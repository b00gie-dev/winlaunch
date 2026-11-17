//! Favicons for custom commands that open a web page (also search commands): the
//! site's own icon instead of the browser's or the magnifier.
//!
//! Fetched directly from the site – no third-party favicon service, which would learn
//! every configured domain: first `/favicon.ico`, else the `<link rel="icon">` of the
//! start page. Saved as `.ico` in `%LOCALAPPDATA%\WinLaunch\favicons\` (a PNG is wrapped
//! into an ICO, which Windows Vista and later display), refreshed after 30 days. After
//! a failure the next attempt is a day later. Downloads run only on background threads.

use std::path::PathBuf;
use std::time::Duration;

const REFRESH_AFTER: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// `https://host:port` of a URL, the part favicons belong to.
fn origin(url: &str) -> Option<String> {
    let (secure, host, port, _) = crate::net::split_url(url)?;
    let scheme = if secure { "https" } else { "http" };
    let default = if secure { 443 } else { 80 };
    Some(if port == default {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    })
}

fn dir() -> Option<PathBuf> {
    Some(crate::local_data_dir()?.join("favicons"))
}

/// File name for an origin: "https_www.google.com_443"-like, safe for Windows.
fn file_stem(origin: &str) -> String {
    origin
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn icon_path(origin: &str) -> Option<PathBuf> {
    Some(dir()?.join(format!("{}.ico", file_stem(origin))))
}

fn failed_marker(origin: &str) -> Option<PathBuf> {
    Some(dir()?.join(format!("{}.failed", file_stem(origin))))
}

fn age(path: &PathBuf) -> Option<Duration> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()
}

/// The saved favicon for the site of `url`, if there is one.
pub fn cached(url: &str) -> Option<String> {
    let path = icon_path(&origin(url)?)?;
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

/// True if the favicon of `url`'s site is missing or old and may be fetched now.
pub fn needs_fetch(url: &str) -> bool {
    let Some(origin) = origin(url) else {
        return false;
    };
    let fresh = icon_path(&origin)
        .and_then(|p| age(&p))
        .is_some_and(|a| a < REFRESH_AFTER);
    let failed_recently = failed_marker(&origin)
        .and_then(|p| age(&p))
        .is_some_and(|a| a < RETRY_AFTER);
    !fresh && !failed_recently
}

/// Downloads and saves the favicon of `url`'s site (blocking). True on success.
pub fn fetch(url: &str) -> bool {
    let Some(origin) = origin(url) else {
        return false;
    };
    let (Some(path), Some(marker), Some(dir)) = (icon_path(&origin), failed_marker(&origin), dir())
    else {
        return false;
    };
    let _ = std::fs::create_dir_all(&dir);
    match download(&origin) {
        Some(ico) => {
            let tmp = path.with_extension("ico.tmp");
            let ok = std::fs::write(&tmp, ico).is_ok() && std::fs::rename(&tmp, &path).is_ok();
            let _ = std::fs::remove_file(&marker);
            ok
        }
        None => {
            crate::debug_log::log(&format!("no favicon for {origin}"));
            let _ = std::fs::write(&marker, b"");
            false
        }
    }
}

fn download(origin: &str) -> Option<Vec<u8>> {
    if let Some(ico) = crate::net::get_url(&format!("{origin}/favicon.ico"))
        .ok()
        .and_then(|b| as_ico(&b))
    {
        return Some(ico);
    }
    let page = crate::net::get_url(&format!("{origin}/")).ok()?;
    let html = String::from_utf8_lossy(&page);
    icon_links(&html)
        .into_iter()
        .filter_map(|href| resolve(origin, &href))
        .find_map(|url| crate::net::get_url(&url).ok().and_then(|b| as_ico(&b)))
}

/// The data as an ICO file: ICO stays, PNG is wrapped. Anything else (HTML error
/// pages, SVG) is rejected.
pub fn as_ico(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() > 6 && data[..4] == [0, 0, 1, 0] {
        return Some(data.to_vec());
    }
    if data.len() >= 24 && data[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
        let dim = |v: u32| if v >= 256 { 0u8 } else { v as u8 }; // 0 means 256
        let mut ico = vec![0, 0, 1, 0, 1, 0]; // header: type icon, 1 image
        ico.extend_from_slice(&[dim(width), dim(height), 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes()); // planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes()); // offset: 6 + 16
        ico.extend_from_slice(data);
        return Some(ico);
    }
    None
}

/// `href`s of `<link rel="…icon…">` tags, apple-touch-icon (usually a large PNG) first.
fn icon_links(html: &str) -> Vec<String> {
    // ASCII only: offsets found in `lower` are used on `html`, so both must have the
    // same byte length (full lowercasing changes it for some letters, e.g. Turkish İ).
    let lower = html.to_ascii_lowercase();
    let mut links: Vec<(bool, String)> = Vec::new();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<link") {
        let start = pos + i;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &html[start..end];
        let tag_lower = &lower[start..end];
        pos = end;
        let Some(rel) = attribute(tag_lower, "rel") else {
            continue;
        };
        if !rel.split_whitespace().any(|r| r.contains("icon")) || rel.contains("mask") {
            continue;
        }
        if let Some(href) = attribute(tag, "href") {
            let href = href.replace("&amp;", "&");
            if !href.to_lowercase().ends_with(".svg") {
                links.push((rel.contains("apple-touch-icon"), href));
            }
        }
    }
    links.sort_by_key(|(apple, _)| !apple);
    links.into_iter().map(|(_, h)| h).collect()
}

/// Value of an HTML attribute (quoted with ' or ", or unquoted).
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase(); // same byte offsets as `tag`, see icon_links
    let mut from = 0;
    loop {
        let i = from + lower[from..].find(name)?;
        from = i + name.len();
        // Whole attribute name, followed by "=".
        let before_ok = i == 0 || lower.as_bytes()[i - 1].is_ascii_whitespace();
        let rest = tag[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let value = rest[1..].trim_start();
        return Some(match value.chars().next()? {
            q @ ('"' | '\'') => value[1..].split(q).next()?.to_string(),
            _ => value
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()?
                .to_string(),
        });
    }
}

/// Absolute URL of a link found on `origin`'s start page.
fn resolve(origin: &str, href: &str) -> Option<String> {
    if href.starts_with("http://") || href.starts_with("https://") {
        Some(href.to_string())
    } else if let Some(rest) = href.strip_prefix("//") {
        let scheme = origin.split("://").next()?;
        Some(format!("{scheme}://{rest}"))
    } else if href.starts_with('/') {
        Some(format!("{origin}{href}"))
    } else if href.is_empty() || href.starts_with("data:") {
        None
    } else {
        Some(format!("{origin}/{href}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(
            origin("https://www.google.com/search?q={query}").as_deref(),
            Some("https://www.google.com")
        );
        assert_eq!(
            origin("http://homeassistant.local:8123").as_deref(),
            Some("http://homeassistant.local:8123")
        );
        assert_eq!(origin("C:\\Downloads"), None);
        assert_eq!(file_stem("http://ha.local:8123"), "http___ha.local_8123");
    }

    #[test]
    fn keeps_ico_wraps_png_rejects_rest() {
        let ico = [0u8, 0, 1, 0, 1, 0, 16, 16];
        assert_eq!(as_ico(&ico), Some(ico.to_vec()));
        let mut png = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
        ];
        png.extend_from_slice(&180u32.to_be_bytes());
        png.extend_from_slice(&180u32.to_be_bytes());
        let wrapped = as_ico(&png).unwrap();
        assert_eq!(&wrapped[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!(wrapped[6], 180);
        assert_eq!(&wrapped[22..], &png[..]);
        assert_eq!(as_ico(b"<!doctype html><html>"), None);
        assert_eq!(as_ico(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None);
    }

    #[test]
    fn finds_icon_links() {
        let html = r#"<html><head>
            <link rel="stylesheet" href="/a.css">
            <link rel="icon" type="image/png" href="/static/icons/favicon.png">
            <link rel='mask-icon' href='/m.svg'>
            <link rel="icon" href="/logo.svg">
            <LINK REL="apple-touch-icon" HREF="/apple.png?v=1&amp;x=2">
        </head></html>"#;
        assert_eq!(
            icon_links(html),
            ["/apple.png?v=1&x=2", "/static/icons/favicon.png"]
        );
        // Letters whose lowercase form is longer or shorter (İ, Kelvin sign) must not
        // shift the offsets (this used to panic).
        let html = "<title>İstanbul K</title><link rel=\"icon\" title=\"İİ\" href=\"/f.png\">";
        assert_eq!(icon_links(html), ["/f.png"]);
    }

    /// Real downloads (network), into a temporary folder: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn fetches_real_favicons() {
        let dir = std::env::temp_dir().join("winlaunch-favicon-test");
        std::env::set_var("WINLAUNCH_DATA_DIR", &dir);
        for url in [
            "https://www.google.com/search?q={query}",
            "https://de.wikipedia.org/w/index.php?search={query}",
            "https://github.com",
        ] {
            assert!(fetch(url), "{url}");
            let path = cached(url).expect("saved");
            let data = std::fs::read(&path).unwrap();
            assert_eq!(&data[..4], &[0, 0, 1, 0], "{path}");
            println!("{url} -> {path} ({} bytes)", data.len());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resolves_links() {
        let o = "https://x.org";
        assert_eq!(resolve(o, "/i.png").as_deref(), Some("https://x.org/i.png"));
        assert_eq!(resolve(o, "i.png").as_deref(), Some("https://x.org/i.png"));
        assert_eq!(
            resolve(o, "//cdn.x.org/i.png").as_deref(),
            Some("https://cdn.x.org/i.png")
        );
        assert_eq!(
            resolve(o, "https://y.org/i.ico").as_deref(),
            Some("https://y.org/i.ico")
        );
        assert_eq!(resolve(o, "data:image/png;base64,AAA"), None);
    }
}
