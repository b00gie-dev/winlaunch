//! The user's own commands, searched with a leading "?". They live in a small TOML
//! file, `%APPDATA%\WinLaunch\commands.toml`, which survives updates and reinstalls.
//! The command "Edit custom commands" creates it with examples; changes are picked up
//! the next time the launcher opens.
//!
//! ```toml
//! [[command]]
//! name = "Google"
//! keyword = "g"                     # "?g rust gdi" searches for "rust gdi"
//! open = "https://www.google.com/search?q={query}"
//!
//! [[command]]
//! name = "OrcaSlicer (projects)"
//! run = '%ProgramFiles%\OrcaSlicer\orca-slicer.exe'
//! args = '--datadir "D:\3D"'
//! folder = 'D:\3D'                  # optional working folder
//! icon = 'C:\Icons\orca.ico'        # optional: file whose icon is shown
//! ```
//!
//! Icons: a program shows its own icon, a folder or file the one Explorer shows, a web
//! page the default browser's. Search commands keep a magnifier glyph. `icon` (any
//! file: .exe, .ico, ...) overrides all of these.

use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Placeholder for the text typed after the keyword.
const QUERY: &str = "{query}";

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// URL, folder or file, opened like a double-click.
    Open(String),
    /// Program with arguments and an optional working folder.
    Run {
        program: String,
        args: String,
        folder: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Custom {
    pub name: String,
    /// Short word that starts the command, e.g. "g" in "?g rust".
    pub keyword: Option<String>,
    pub action: Action,
    /// File whose icon is shown instead of the automatic one.
    pub icon: Option<String>,
}

/// Where a custom command's icon comes from.
#[derive(Debug, PartialEq)]
pub enum IconSource {
    /// The icon Explorer shows for this file or folder (a program: its own icon).
    File(String),
    /// The icon of the default web browser.
    Browser,
}

impl Custom {
    /// Stable key in the launch history.
    pub fn target(&self) -> String {
        format!("custom:{}", self.name)
    }

    /// The command uses the text typed after its keyword (`{query}`).
    pub fn takes_query(&self) -> bool {
        match &self.action {
            Action::Open(target) => target.contains(QUERY),
            Action::Run { program, args, .. } => program.contains(QUERY) || args.contains(QUERY),
        }
    }

    /// For "?g rust gdi" (`text` = "g rust gdi"): `Some("rust gdi")` if this command
    /// takes a query and "g" is its keyword or its whole name.
    pub fn query_in<'a>(&self, text: &'a str) -> Option<&'a str> {
        if !self.takes_query() {
            return None;
        }
        let text = text.trim_start();
        let (word, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let matches = |s: &str| !word.is_empty() && s.eq_ignore_ascii_case(word);
        (self.keyword.as_deref().is_some_and(matches) || matches(&self.name)).then(|| rest.trim())
    }

    /// The web address the command opens, if any (favicons come from its site).
    pub fn url(&self) -> Option<&str> {
        match &self.action {
            Action::Open(target) if target.contains("://") => Some(target),
            _ => None,
        }
    }

    /// Where the bitmap icon comes from; `None`: only the glyph (search commands, and
    /// paths that do not exist). A web page's favicon, if loaded, comes first (see
    /// favicon.rs); an explicit `icon` before everything.
    pub fn icon_source(&self) -> Option<IconSource> {
        if let Some(icon) = &self.icon {
            return Some(IconSource::File(expand_env(icon)));
        }
        if self.takes_query() {
            return None; // the magnifier says more than a browser icon
        }
        match &self.action {
            Action::Open(target) if target.contains("://") => Some(IconSource::Browser),
            Action::Open(target) => {
                let path = expand_env(target);
                std::path::Path::new(&path)
                    .exists()
                    .then_some(IconSource::File(path))
            }
            Action::Run { program, .. } => find_program(&expand_env(program)).map(IconSource::File),
        }
    }

    /// Fallback icon while the bitmap loads or if there is none: a glyph of
    /// "Segoe MDL2 Assets".
    pub fn glyph(&self) -> char {
        match &self.action {
            _ if self.takes_query() => '\u{E721}', // Search
            Action::Open(target) if target.contains("://") => '\u{E774}', // Globe
            Action::Open(_) => '\u{E8B7}',         // Folder
            Action::Run { .. } => '\u{E756}',      // CommandPrompt
        }
    }

    /// Runs the command with `query` filled in, unelevated like apps.
    pub fn execute(&self, query: &str, elevated: bool) {
        match &self.action {
            Action::Open(target) => {
                let target = expand_env(target);
                let target = if target.contains("://") {
                    target.replace(QUERY, &url_encode(query))
                } else {
                    target.replace(QUERY, query)
                };
                crate::apps::open(target, elevated);
            }
            Action::Run {
                program,
                args,
                folder,
            } => crate::apps::run(
                expand_env(&program.replace(QUERY, query)),
                expand_env(&args.replace(QUERY, query)),
                folder.as_deref().map(expand_env),
                elevated,
            ),
        }
    }
}

/// What the file contains, as read.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default, rename = "command")]
    commands: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    keyword: Option<String>,
    open: Option<String>,
    run: Option<String>,
    args: Option<String>,
    folder: Option<String>,
    icon: Option<String>,
}

/// Parses the file. Errors name the line (TOML errors) or the command.
pub fn parse(text: &str) -> Result<Vec<Custom>, String> {
    let file: File = toml::from_str(text).map_err(|e| {
        // First line of the message, e.g. "TOML parse error at line 3, column 8".
        e.to_string().lines().next().unwrap_or_default().to_string()
    })?;
    file.commands
        .into_iter()
        .map(|e| {
            let name = e.name.trim().to_string();
            if name.is_empty() {
                return Err("\"name\" is empty".to_string());
            }
            let action = match (e.open, e.run) {
                (Some(open), None) if e.args.is_none() && e.folder.is_none() => Action::Open(open),
                (None, Some(program)) => Action::Run {
                    program,
                    args: e.args.unwrap_or_default(),
                    folder: e.folder,
                },
                (Some(_), None) => {
                    return Err(format!(
                        "{name}: \"args\"/\"folder\" only work with \"run\""
                    ))
                }
                _ => return Err(format!("{name}: needs either \"open\" or \"run\"")),
            };
            let keyword = e
                .keyword
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty());
            let icon = e
                .icon
                .map(|i| i.trim().to_string())
                .filter(|i| !i.is_empty());
            Ok(Custom {
                name,
                keyword,
                action,
                icon,
            })
        })
        .collect()
}

/// Last read state of the file.
struct Loaded {
    modified: Option<SystemTime>,
    commands: Arc<Vec<Custom>>,
    error: Option<String>,
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);

/// `%APPDATA%\WinLaunch\commands.toml` (`WINLAUNCH_DATA_DIR` overrides the folder).
pub fn path() -> Option<PathBuf> {
    let dir = match crate::test_env("WINLAUNCH_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("APPDATA")?).join("WinLaunch"),
    };
    Some(dir.join("commands.toml"))
}

/// Rereads the file if it changed since the last read. Returns true if it did.
pub fn reload_if_changed() -> bool {
    let path = path();
    let modified = path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let mut loaded = LOADED.lock().unwrap();
    if loaded.as_ref().is_some_and(|l| l.modified == modified) {
        return false;
    }
    let text = path.and_then(|p| std::fs::read_to_string(p).ok());
    let (commands, error) = match text.map(|t| parse(&t)) {
        None => (Vec::new(), None), // no file yet
        Some(Ok(commands)) => (commands, None),
        Some(Err(e)) => (Vec::new(), Some(e)),
    };
    *loaded = Some(Loaded {
        modified,
        commands: Arc::new(commands),
        error,
    });
    true
}

/// The commands as last read (empty before the first read or after an error).
pub fn commands() -> Arc<Vec<Custom>> {
    LOADED
        .lock()
        .unwrap()
        .as_ref()
        .map(|l| l.commands.clone())
        .unwrap_or_default()
}

/// Why the file could not be read, if it could not.
pub fn error() -> Option<String> {
    LOADED
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|l| l.error.clone())
}

/// "Edit custom commands": creates the file with examples if needed and opens it.
pub fn edit(elevated: bool) {
    let Some(path) = path() else { return };
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let template = if crate::i18n::language() == "de" {
            include_str!("../templates/commands.de.toml")
        } else {
            include_str!("../templates/commands.en.toml")
        };
        let _ = std::fs::write(&path, template);
    }
    crate::apps::open(path.to_string_lossy().into_owned(), elevated);
}

/// Full path of a program: as given if it exists, else looked up in PATH like the
/// command line does ("cmd.exe" or "cmd" -> C:\Windows\System32\cmd.exe).
fn find_program(program: &str) -> Option<String> {
    let path = std::path::Path::new(program);
    if path.is_file() {
        return Some(program.to_string());
    }
    if path.components().count() != 1 {
        return None; // a path that does not exist; PATH is only for bare names
    }
    let names = if path.extension().is_some() {
        vec![program.to_string()]
    } else {
        vec![program.to_string(), format!("{program}.exe")]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Replaces `%NAME%` with environment variables (Windows' own rules).
fn expand_env(s: &str) -> String {
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    let src = crate::wide(s);
    unsafe {
        let needed = ExpandEnvironmentStringsW(src.as_ptr(), std::ptr::null_mut(), 0);
        if needed == 0 {
            return s.to_string();
        }
        let mut buf = vec![0u16; needed as usize];
        let n = ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), needed);
        if n == 0 || n > needed {
            return s.to_string();
        }
        String::from_utf16_lossy(&buf[..n as usize - 1])
    }
}

/// Percent-encodes everything but unreserved characters (RFC 3986), for URL queries.
fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(toml: &str) -> Custom {
        parse(toml).unwrap().pop().unwrap()
    }

    #[test]
    fn parses_open_and_run() {
        let list = parse(
            r#"
            [[command]]
            name = "Home Assistant"
            open = "http://ha.local:8123"

            [[command]]
            name = "Orca"
            run = 'C:\Orca\orca.exe'
            args = '--datadir "D:\3D"'
            "#,
        )
        .unwrap();
        assert_eq!(list[0].action, Action::Open("http://ha.local:8123".into()));
        assert_eq!(
            list[1].action,
            Action::Run {
                program: r"C:\Orca\orca.exe".into(),
                args: r#"--datadir "D:\3D""#.into(),
                folder: None,
            }
        );
    }

    #[test]
    fn rejects_unclear_entries() {
        assert!(parse("[[command]]\nname = \"x\"").is_err());
        assert!(parse("[[command]]\nname = \"x\"\nopen = \"a\"\nrun = \"b\"").is_err());
        assert!(parse("[[command]]\nname = \"x\"\nopen = \"a\"\nargs = \"-v\"").is_err());
        assert!(parse("[[command]]\nname = \"x\"\nopen = \"a\"\ntypo = 1").is_err());
        let err = parse("[[command]\nname = 1").unwrap_err();
        assert!(err.contains("line"), "{err}");
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn query_follows_keyword_or_name() {
        let google = one(
            "[[command]]\nname = \"Google\"\nkeyword = \"g\"\nopen = \"https://g.com/?q={query}\"",
        );
        assert_eq!(google.query_in("g rust gdi"), Some("rust gdi"));
        assert_eq!(google.query_in("G"), Some(""));
        assert_eq!(google.query_in("google  rust "), Some("rust"));
        assert_eq!(google.query_in("go rust"), None);
        let site = one("[[command]]\nname = \"Site\"\nkeyword = \"s\"\nopen = \"https://x.com\"");
        assert_eq!(site.query_in("s rust"), None, "no {{query}}, no query");
    }

    #[test]
    fn icon_sources() {
        let site = one("[[command]]\nname = \"HA\"\nopen = \"http://ha.local\"");
        assert_eq!(site.icon_source(), Some(IconSource::Browser));
        let search = one("[[command]]\nname = \"G\"\nopen = \"https://g.com/?q={query}\"");
        assert_eq!(
            search.icon_source(),
            None,
            "search commands keep the magnifier"
        );
        let cmd = one("[[command]]\nname = \"Cmd\"\nrun = \"cmd\"");
        match cmd.icon_source() {
            Some(IconSource::File(p)) => assert!(p.to_lowercase().ends_with("cmd.exe"), "{p}"),
            other => panic!("{other:?}"),
        }
        let gone = one("[[command]]\nname = \"X\"\nrun = 'C:\\nowhere\\x.exe'");
        assert_eq!(gone.icon_source(), None);
        let own = one(
            "[[command]]\nname = \"G\"\nopen = \"https://g.com/?q={query}\"\nicon = 'C:\\g.ico'",
        );
        assert_eq!(
            own.icon_source(),
            Some(IconSource::File(r"C:\g.ico".into()))
        );
    }

    #[test]
    fn url_encoding_keeps_unreserved_only() {
        assert_eq!(url_encode("rust gdi"), "rust%20gdi");
        assert_eq!(url_encode("a&b=ü"), "a%26b%3D%C3%BC");
        assert_eq!(url_encode("a-b_c.d~"), "a-b_c.d~");
    }

    #[test]
    fn templates_parse() {
        for t in [
            include_str!("../templates/commands.de.toml"),
            include_str!("../templates/commands.en.toml"),
        ] {
            let list = parse(t).unwrap();
            assert!(!list.is_empty());
        }
    }
}
