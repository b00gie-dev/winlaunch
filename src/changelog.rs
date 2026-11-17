//! What's new: the release notes from CHANGELOG.md.
//!
//! * Before an update: the release workflow passes the version's CHANGELOG section to
//!   Velopack as release notes; they arrive with the update check (`NotesMarkdown`) and
//!   "/update" lists them under the update command.
//! * After an update: the binary carries its own CHANGELOG.md. The first time the
//!   launcher opens with an empty field after the version changed, it shows that
//!   version's notes instead of the most-used apps, once.
//!
//! An entry may start with its kind in bold, which picks its icon and is not shown:
//! `- **New:** …`, `- **Improved:** …`, `- **Fix:** …`, `- **Security:** …`.

use std::path::PathBuf;

/// Kind of a changelog entry, from its prefix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    New,
    Improved,
    Fix,
    Security,
    /// No (known) prefix.
    Other,
}

impl Kind {
    fn from_word(word: &str) -> Option<Kind> {
        Some(match word.to_lowercase().as_str() {
            "new" | "feature" | "added" => Kind::New,
            "improved" | "improvement" | "changed" => Kind::Improved,
            "fix" | "fixed" | "bug" | "bugfix" => Kind::Fix,
            "security" => Kind::Security,
            _ => return None,
        })
    }

    /// Icon: a glyph of "Segoe MDL2 Assets".
    pub fn glyph(self) -> char {
        match self {
            Kind::New => '\u{E734}',      // FavoriteStar
            Kind::Improved => '\u{E74A}', // Up
            Kind::Fix => '\u{EBE8}',      // Bug
            Kind::Security => '\u{E72E}', // Lock
            Kind::Other => '\u{E7C3}',    // Page
        }
    }
}

/// One changelog entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub kind: Kind,
    pub text: String,
}

/// Splits "**New:** text", "**New**: text" or "New: text" into kind and text.
fn note(item: &str) -> Note {
    let plain = item.strip_prefix("**").unwrap_or(item);
    if let Some((word, rest)) = plain.split_once(':') {
        if let Some(kind) = Kind::from_word(word.trim().trim_end_matches("**")) {
            let rest = rest.trim_start();
            let text = rest.strip_prefix("**").unwrap_or(rest);
            return Note {
                kind,
                text: clean(text),
            };
        }
    }
    Note {
        kind: Kind::Other,
        text: clean(item),
    }
}

/// Without simple markdown markup.
fn clean(text: &str) -> String {
    text.replace("**", "").replace('`', "").trim().to_string()
}

/// This version's changelog, built into the binary.
const CHANGELOG: &str = include_str!("../CHANGELOG.md");

/// Entries of a markdown list ("- item" / "* item"), with simple markup removed.
pub fn bullets(markdown: &str) -> Vec<Note> {
    markdown
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            l.strip_prefix("- ").or_else(|| l.strip_prefix("* "))
        })
        .map(note)
        .filter(|n| !n.text.is_empty())
        .collect()
}

/// The entries of the `## version` section of a changelog.
pub fn section(changelog: &str, version: &str) -> Vec<Note> {
    let heading = format!("## {version}");
    let mut lines = changelog.lines().skip_while(|l| l.trim() != heading);
    if lines.next().is_none() {
        return Vec::new();
    }
    let body: Vec<&str> = lines.take_while(|l| !l.starts_with("## ")).collect();
    bullets(&body.join("\n"))
}

fn seen_path() -> Option<PathBuf> {
    Some(crate::local_data_dir()?.join("last-version.txt"))
}

/// Notes to show once after an update: `(version, items)`, or `None` if this version's
/// notes were already shown. A fresh install (no launch history yet) shows nothing.
pub fn whats_new() -> Option<(String, Vec<Note>)> {
    let (version, dev) = crate::update::installed_version();
    if dev {
        return None;
    }
    let path = seen_path()?;
    match std::fs::read_to_string(&path) {
        Ok(seen) if seen.trim() == version => return None,
        Ok(_) => {}
        Err(_) => {
            // No marker yet: an update from a version before this feature (there is a
            // launch history) or a fresh install (there is none).
            let has_history = path.with_file_name("usage.tsv").exists();
            if !has_history {
                mark_seen();
                return None;
            }
        }
    }
    let items = section(CHANGELOG, &version);
    if items.is_empty() {
        mark_seen();
        return None;
    }
    Some((version, items))
}

/// Remembers that this version's notes were shown.
pub fn mark_seen() {
    let (version, dev) = crate::update::installed_version();
    if dev {
        return;
    }
    if let Some(path) = seen_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog\n\nIntro.\n\n## 1.1.0\n\n- New **thing**\n- `cmd` works\n\n## 1.0.0\n\n- First\n";

    fn texts(notes: Vec<Note>) -> Vec<String> {
        notes.into_iter().map(|n| n.text).collect()
    }

    #[test]
    fn reads_a_section() {
        assert_eq!(texts(section(LOG, "1.1.0")), ["New thing", "cmd works"]);
        assert_eq!(texts(section(LOG, "1.0.0")), ["First"]);
        assert!(section(LOG, "2.0.0").is_empty());
    }

    #[test]
    fn reads_release_notes_from_the_feed() {
        assert_eq!(
            texts(bullets("- a\r\n* b\n\nnot a bullet\n-  c ")),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn kinds_from_prefixes() {
        let notes = bullets(
            "- **New:** Calculator\n- **Fix**: Icons\n- Improved: Faster\n- **Security:** Safe\n- **Bug:** x\n- Ratio: 1:2\n- Plain",
        );
        let kinds: Vec<(Kind, &str)> = notes.iter().map(|n| (n.kind, n.text.as_str())).collect();
        assert_eq!(
            kinds,
            [
                (Kind::New, "Calculator"),
                (Kind::Fix, "Icons"),
                (Kind::Improved, "Faster"),
                (Kind::Security, "Safe"),
                (Kind::Fix, "x"),
                (Kind::Other, "Ratio: 1:2"),
                (Kind::Other, "Plain"),
            ]
        );
    }

    #[test]
    fn changelog_has_this_version() {
        // A version bump without notes would fail the release; catch it in CI already.
        let version = env!("CARGO_PKG_VERSION");
        assert!(
            !section(CHANGELOG, version).is_empty(),
            "CHANGELOG.md needs a '## {version}' section"
        );
    }
}
