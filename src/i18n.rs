//! Localization with Project Fluent.
//!
//! * Translations live in `locales/<lang>.ftl` and are embedded into the EXE.
//! * The language follows the Windows display language (`GetUserPreferredUILanguages`),
//!   not the regional format. Resolution per preferred language: exact tag, then base
//!   language (`de-AT` -> `de`); English is always the last fallback, also for single
//!   messages missing in a translation.
//! * Loaded lazily on first use (warmed up on the background index thread), so it never
//!   costs startup time.
//! * `WINLAUNCH_LANG` overrides the language (comma-separated list, e.g. `de` or `en`).
//!   `qps-ploc` (the Windows pseudo-locale) shows accented, ~40 % longer English text to
//!   find hard-coded strings and layouts that are too tight.

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use std::borrow::Cow;
use std::sync::OnceLock;
use unic_langid::LanguageIdentifier;

/// Available translations. English is the fallback and must contain every message.
const LOCALES: &[(&str, &str)] = &[
    ("en", include_str!("../locales/en.ftl")),
    ("de", include_str!("../locales/de.ftl")),
];
const FALLBACK: &str = "en";
const PSEUDO: &str = "qps-ploc";

struct Catalog {
    /// Preferred language first, English last.
    bundles: Vec<FluentBundle<FluentResource>>,
    /// Language code of each bundle.
    languages: Vec<&'static str>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();

/// Loads the translations now (call from a background thread to warm up).
pub fn init() {
    catalog();
}

/// Localized text for a message id. Unknown ids come back unchanged, so a missing
/// message is visible instead of silently empty.
pub fn t(id: &str) -> String {
    catalog().format(id, None)
}

/// The English (fallback) text for a message id, whatever the display language is.
/// Used for search aliases, so English names still find localized commands.
#[cfg(test)]
pub fn t_en(id: &str) -> String {
    t_en_args(id, &[])
}

/// English text with named arguments (see `t_args`).
pub fn t_en_args(id: &str, args: &[(&str, &str)]) -> String {
    let mut fluent_args = FluentArgs::new();
    for (name, value) in args {
        fluent_args.set(*name, FluentValue::from(*value));
    }
    // English is always the last bundle of the chain.
    catalog()
        .bundles
        .last()
        .and_then(|bundle| format_in(bundle, id, Some(&fluent_args)))
        .unwrap_or_else(|| id.to_string())
}

/// Localized text with named arguments, e.g. `t_args("install-failed", &[("details", &e)])`.
pub fn t_args(id: &str, args: &[(&str, &str)]) -> String {
    let mut fluent_args = FluentArgs::new();
    for (name, value) in args {
        fluent_args.set(*name, FluentValue::from(*value));
    }
    catalog().format(id, Some(&fluent_args))
}

/// Code of the language WinLaunch shows, e.g. "de" (English: "en").
pub fn language() -> &'static str {
    catalog().languages.first().copied().unwrap_or(FALLBACK)
}

fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| Catalog::new(&preferred_languages()))
}

impl Catalog {
    fn new(preferred: &[String]) -> Self {
        let pseudo = preferred
            .first()
            .is_some_and(|l| l.eq_ignore_ascii_case(PSEUDO));
        let available: Vec<&str> = LOCALES.iter().map(|(code, _)| *code).collect();
        let chain = if pseudo {
            vec![FALLBACK]
        } else {
            resolve(preferred, &available)
        };
        Catalog {
            bundles: chain.iter().map(|code| bundle(code, pseudo)).collect(),
            languages: chain,
        }
    }

    fn format(&self, id: &str, args: Option<&FluentArgs>) -> String {
        self.bundles
            .iter()
            .find_map(|bundle| format_in(bundle, id, args))
            .unwrap_or_else(|| id.to_string())
    }
}

/// The message in one language, or None if that language does not have it.
fn format_in(
    bundle: &FluentBundle<FluentResource>,
    id: &str,
    args: Option<&FluentArgs>,
) -> Option<String> {
    let pattern = bundle.get_message(id)?.value()?;
    let mut errors = Vec::new();
    Some(
        bundle
            .format_pattern(pattern, args, &mut errors)
            .into_owned(),
    )
}

fn bundle(code: &str, pseudo: bool) -> FluentBundle<FluentResource> {
    let source = LOCALES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, s)| *s)
        .unwrap_or_default();
    let langid: LanguageIdentifier = code.parse().unwrap_or_default();
    let mut bundle = FluentBundle::new_concurrent(vec![langid]);
    // No Unicode bidi isolation marks around arguments: all languages are left-to-right
    // so far, and the marks would show up in message boxes copied as text.
    bundle.set_use_isolating(false);
    if pseudo {
        bundle.set_transform(Some(pseudo_text));
    }
    // Syntax errors keep the valid part; the unit tests make sure there are none.
    let resource = FluentResource::try_new(source.to_string()).unwrap_or_else(|(r, _)| r);
    let _ = bundle.add_resource(resource);
    bundle
}

/// Language chain for the preferred display languages: per preference the exact tag,
/// then the base language; English always last. Case-insensitive.
fn resolve(preferred: &[String], available: &[&'static str]) -> Vec<&'static str> {
    let mut chain: Vec<&'static str> = Vec::new();
    let mut push = |tag: &str| {
        if let Some(code) = available.iter().find(|c| c.eq_ignore_ascii_case(tag)) {
            if !chain.contains(code) {
                chain.push(code);
            }
        }
    };
    for lang in preferred {
        push(lang);
        if let Some((base, _)) = lang.split_once('-') {
            push(base);
        }
    }
    push(FALLBACK);
    chain
}

/// Preferred display languages: `WINLAUNCH_LANG` if set, otherwise from Windows.
fn preferred_languages() -> Vec<String> {
    if let Ok(list) = std::env::var("WINLAUNCH_LANG") {
        return list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    windows_ui_languages()
}

/// `GetUserPreferredUILanguages`: the user's display languages, most preferred first.
fn windows_ui_languages() -> Vec<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    unsafe {
        let (mut count, mut len) = (0u32, 0u32);
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            std::ptr::null_mut(),
            &mut len,
        ) == 0
        {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len)
            == 0
        {
            return Vec::new();
        }
        // Double-null-terminated list: "de-DE\0en-US\0\0"
        buf.split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}

/// Pseudo-localization: accented letters, padded to ~140 % of the original length
/// (typical growth of translations from English).
fn pseudo_text(text: &str) -> Cow<'_, str> {
    if text.trim().is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        let accented = match c {
            'a' => 'á',
            'e' => 'é',
            'i' => 'í',
            'o' => 'ö',
            'u' => 'ü',
            'A' => 'Å',
            'E' => 'É',
            'I' => 'Î',
            'O' => 'Ø',
            'U' => 'Û',
            'c' => 'ç',
            'n' => 'ñ',
            's' => 'š',
            'y' => 'ý',
            'z' => 'ž',
            'C' => 'Ç',
            'N' => 'Ñ',
            'S' => 'Š',
            'W' => 'Ŵ',
            'L' => 'Ľ',
            other => other,
        };
        out.push(accented);
    }
    let padding = (text.chars().count() * 2).div_ceil(5); // +40 %
    out.push(' ');
    out.extend(std::iter::repeat_n('~', padding.saturating_sub(1).max(1)));
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluent_syntax::ast;
    use std::collections::BTreeSet;

    fn langs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Message ids and the variables each one uses, for one .ftl source.
    fn messages(source: &str) -> BTreeSet<(String, BTreeSet<String>)> {
        let resource = fluent_syntax::parser::parse(source)
            .unwrap_or_else(|(_, errors)| panic!("syntax errors: {errors:?}"));
        resource
            .body
            .iter()
            .filter_map(|entry| match entry {
                ast::Entry::Message(m) => {
                    let mut vars = BTreeSet::new();
                    if let Some(value) = &m.value {
                        for element in &value.elements {
                            if let ast::PatternElement::Placeable {
                                expression:
                                    ast::Expression::Inline(ast::InlineExpression::VariableReference {
                                        id,
                                    }),
                            } = element
                            {
                                vars.insert(id.name.to_string());
                            }
                        }
                    }
                    Some((m.id.name.to_string(), vars))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_locale_parses_and_matches_english() {
        let english = messages(LOCALES[0].1);
        assert!(!english.is_empty());
        for (code, source) in LOCALES {
            assert_eq!(
                messages(source),
                english,
                "{code}.ftl must have the same messages and variables as en.ftl"
            );
        }
    }

    #[test]
    fn resolves_exact_then_base_then_english() {
        let available = ["en", "de"];
        assert_eq!(resolve(&langs(&["de-AT"]), &available), vec!["de", "en"]);
        assert_eq!(
            resolve(&langs(&["fr-FR", "de-DE"]), &available),
            vec!["de", "en"]
        );
        assert_eq!(
            resolve(&langs(&["en-US", "de-DE"]), &available),
            vec!["en", "de"]
        );
        assert_eq!(resolve(&langs(&["DE"]), &available), vec!["de", "en"]);
        assert_eq!(resolve(&[], &available), vec!["en"]);
        let regional = ["en", "pt", "pt-BR"];
        assert_eq!(
            resolve(&langs(&["pt-BR"]), &regional),
            vec!["pt-BR", "pt", "en"]
        );
    }

    #[test]
    fn formats_in_chosen_language_with_arguments() {
        let de = Catalog::new(&langs(&["de-DE"]));
        assert_eq!(de.format("result-kind-application", None), "Anwendung");
        let mut args = FluentArgs::new();
        args.set("details", FluentValue::from("Zugriff verweigert"));
        let text = de.format("install-failed", Some(&args));
        assert!(text.starts_with("Autostart konnte nicht eingerichtet werden:"));
        assert!(text.ends_with("Zugriff verweigert"));
        assert!(
            text.contains("\n\n"),
            "blank line in multiline message is kept"
        );

        let en = Catalog::new(&langs(&["fr-FR"]));
        assert_eq!(en.format("result-kind-application", None), "Application");
    }

    #[test]
    fn unknown_message_id_is_returned_unchanged() {
        assert_eq!(
            Catalog::new(&[]).format("no-such-message", None),
            "no-such-message"
        );
    }

    #[test]
    fn pseudo_locale_is_accented_and_longer() {
        let pseudo = Catalog::new(&langs(&["qps-ploc"]));
        let text = pseudo.format("search-placeholder", None);
        let english = Catalog::new(&langs(&["en"])).format("search-placeholder", None);
        assert_ne!(text, english);
        assert!(text.chars().count() as f64 >= english.chars().count() as f64 * 1.3);
        assert!(text.contains('á'));
    }
}
