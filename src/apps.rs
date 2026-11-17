//! App index: everything the Start menu's "All apps" list shows (classic programs
//! and Store apps), read from the shell's AppsFolder on a background thread, plus the
//! shortcuts pinned to the taskbar and those on the desktop (user and public).

use crate::commands::{Command, COMMANDS, UNINSTALL_COMMAND, UPDATE_COMMAND};
use crate::custom::Custom;
use crate::i18n::{t_args, t_en_args};
use crate::search::Searchable;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{Interface, BSTR, HSTRING, PWSTR};
use windows::Win32::Foundation::SIZE;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IDispatch, IPersistFile,
    IServiceProvider, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
    COINIT_DISABLE_OLE1DDE, STGM_READ,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::*;

#[derive(Clone)]
pub struct App {
    pub name: String,
    pub search: Searchable,
    /// AppsFolder parsing name (an AppUserModelID or a file path), or for a taskbar or
    /// desktop shortcut the path of the shortcut file.
    pub target: String,
    /// Taskbar or desktop shortcut: `target` is opened as it is, so the shortcut's
    /// arguments and working folder apply.
    pub shortcut: bool,
    /// Premultiplied 32-bit HBITMAP, 0 while not loaded yet (always 0 for commands).
    pub icon: isize,
    /// Extra names the search also matches (e.g. the English name of a command).
    pub aliases: Vec<Searchable>,
    /// Set for system commands; `None` for real apps.
    pub command: Option<&'static Command>,
    /// Set for the user's own commands (searched with "?", see custom.rs).
    pub custom: Option<Arc<Custom>>,
    /// Set for the calculator row: Enter copies the result (see calc.rs).
    pub answer: Option<crate::calc::Answer>,
    /// Set for file search results: `Some(true)` for a folder (see filesearch.rs).
    pub file: Option<bool>,
}

impl App {
    /// Glyph icon of commands, custom commands and the calculator (apps have a bitmap
    /// icon).
    pub fn glyph(&self) -> Option<char> {
        self.command
            .map(|c| c.glyph)
            .or_else(|| self.custom.as_ref().map(|c| c.glyph()))
            .or_else(|| self.answer.as_ref().map(|a| a.glyph))
    }

    /// A row for a file search result.
    pub fn for_file(hit: &crate::filesearch::Hit) -> App {
        App {
            name: hit.name.clone(),
            search: Searchable::new(""),
            target: hit.path.clone(),
            shortcut: false,
            icon: hit.icon,
            aliases: Vec::new(),
            command: None,
            custom: None,
            answer: None,
            file: Some(hit.folder),
        }
    }

    /// The calculator row for a result.
    pub fn for_answer(answer: crate::calc::Answer) -> App {
        App {
            name: answer.display.clone(),
            search: Searchable::new(""),
            target: "calc:".into(),
            shortcut: false,
            icon: 0,
            aliases: Vec::new(),
            command: None,
            custom: None,
            answer: Some(answer),
            file: None,
        }
    }
}

static INDEX: Mutex<Option<Arc<Vec<App>>>> = Mutex::new(None);
static LAST_SCAN: Mutex<Option<Instant>> = Mutex::new(None);
static LOADING: AtomicBool = AtomicBool::new(false);
/// Version of a downloaded update waiting to be applied.
static UPDATE_VERSION: Mutex<Option<String>> = Mutex::new(None);
/// Installed through Velopack (not a dev build): "Uninstall WinLaunch" is offered.
static INSTALLED: AtomicBool = AtomicBool::new(false);
const MAX_AGE: Duration = Duration::from_secs(60);

/// Current index (empty until the first scan has finished).
pub fn snapshot() -> Arc<Vec<App>> {
    INDEX.lock().unwrap().clone().unwrap_or_default()
}

/// Rescans in the background unless the last scan is recent or one is running.
pub fn refresh_if_stale(notify: usize, msg: u32, icon_px: i32) {
    let stale = LAST_SCAN
        .lock()
        .unwrap()
        .is_none_or(|t| t.elapsed() > MAX_AGE);
    if stale {
        refresh(notify, msg, icon_px);
    }
}

/// Scans the app list on a background thread. Posts `msg` to the window `notify`
/// once names are available and again once icons are loaded.
pub fn refresh(notify: usize, msg: u32, icon_px: i32) {
    if LOADING.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        unsafe {
            crate::usage::load();
            crate::i18n::init();
            let custom_changed = crate::custom::reload_if_changed();
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            let _ = scan(notify, msg, icon_px);
            if custom_changed || !CUSTOM_ICONS_LOADED.swap(true, Ordering::AcqRel) {
                load_custom_icons(icon_px, notify, msg);
            }
            CoUninitialize();
        }
        *LAST_SCAN.lock().unwrap() = Some(Instant::now());
        LOADING.store(false, Ordering::Release);
    });
}

fn post(notify: usize, msg: u32) {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(notify as _, msg, 0, 0);
    }
}

unsafe fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    let p: PWSTR = item.GetDisplayName(kind).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s
}

/// Uninstall entries: recognized by the program they start ("unins000.exe",
/// "Uninstall.exe", msiexec) or by "uninstall" as a whole word of the name. A name
/// that only contains it, like "Geek Uninstaller", is a real app.
fn is_uninstaller(name: &str, target: &str) -> bool {
    let file = target
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(target)
        .to_lowercase();
    let stem = file.strip_suffix(".exe").unwrap_or(&file);
    if stem.starts_with("unins")
        || stem.contains("uninstall")
        || stem.contains("deinstall")
        || stem == "msiexec"
    {
        return true;
    }
    const WORDS: &[&str] = &[
        "uninstall",
        "uninstallation",
        "deinstall",
        "deinstallieren",
        "deinstallation",
    ];
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| WORDS.contains(&word))
}

fn is_wanted(name: &str, target: &str) -> bool {
    if name.trim().is_empty() || is_uninstaller(name, target) {
        return false;
    }
    // Start-menu shortcuts to documents and web links are not apps.
    let t = target.to_lowercase();
    if t.starts_with("http://") || t.starts_with("https://") {
        return false;
    }
    const DOCS: &[&str] = &[
        ".url", ".txt", ".pdf", ".htm", ".html", ".chm", ".rtf", ".md", ".ini", ".log",
    ];
    !DOCS.iter().any(|ext| t.ends_with(ext))
}

/// Icons of apps that disappeared in a rescan. They are freed one rescan later: until
/// then the UI may still hold (and paint) the previous index. A hidden window always
/// takes the new index before it paints again (`show` runs the search first).
static RETIRED_ICONS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// What the icon cache file holds: icon size and per target (stamp, cached_at). `None`
/// until the first scan has read the file.
type CacheIndex = HashMap<String, (u64, u64)>;
static ICON_CACHE: Mutex<Option<(i32, CacheIndex)>> = Mutex::new(None);

unsafe fn scan(notify: usize, msg: u32, icon_px: i32) -> windows::core::Result<()> {
    for icon in std::mem::take(&mut *RETIRED_ICONS.lock().unwrap()) {
        windows_sys::Win32::Graphics::Gdi::DeleteObject(icon as _);
    }
    // Keep icons from the previous scan, so a rescan only loads new ones.
    // Only apps and shortcuts: the icons of custom commands belong to
    // `load_custom_icons`. Taken in here, they were retired below and freed one scan
    // later while the custom commands still showed them (blank icons).
    // Per target a list: two entries (different names) may share a target, and each
    // must get an icon of its own, or one bitmap would be shared and the other leak.
    let mut old_icons: HashMap<String, Vec<isize>> = HashMap::new();
    for a in snapshot()
        .iter()
        .filter(|a| a.icon != 0 && a.custom.is_none() && a.command.is_none())
    {
        old_icons.entry(a.target.clone()).or_default().push(a.icon);
    }
    let all_old: Vec<isize> = old_icons.values().flatten().copied().collect();
    let mut take_old = |target: &str| old_icons.get_mut(target).and_then(Vec::pop).unwrap_or(0);

    let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
    let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;

    let mut apps = Vec::new();
    let mut shell_items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    loop {
        let mut batch = [None];
        let mut fetched = 0u32;
        if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
            break;
        }
        let Some(item) = batch[0].take() else { break };
        let (Some(name), Some(target)) = (
            display_name(&item, SIGDN_NORMALDISPLAY),
            display_name(&item, SIGDN_PARENTRELATIVEPARSING),
        ) else {
            continue;
        };
        if !is_wanted(&name, &target) || !seen.insert((name.clone(), target.clone())) {
            continue;
        }
        let icon = take_old(&target);
        apps.push(App {
            search: Searchable::new(&name),
            name,
            target,
            shortcut: false,
            icon,
            aliases: Vec::new(),
            command: None,
            custom: None,
            answer: None,
            file: None,
        });
        shell_items.push(item);
    }
    for (mut app, item) in pinned_and_desktop_shortcuts(&apps) {
        app.icon = take_old(&app.target);
        apps.push(app);
        shell_items.push(item);
    }

    // First scan after startup: icons from the disk cache, if still current.
    let stamps: Vec<u64> = apps
        .iter()
        .map(|a| crate::icon_cache::stamp(&a.target))
        .collect();
    let now = crate::icon_cache::now();
    let first_scan = ICON_CACHE.lock().unwrap().is_none();
    if first_scan {
        let disk = if icon_px > 0 {
            crate::icon_cache::load(icon_px)
        } else {
            HashMap::new()
        };
        for (app, &stamp) in apps.iter_mut().zip(&stamps) {
            if let Some(cached) = disk.get(&app.target).filter(|c| c.is_current(stamp, now)) {
                app.icon = crate::icon_cache::to_bitmap(cached);
            }
        }
        let index = disk
            .into_iter()
            .map(|(k, c)| (k, (c.stamp, c.cached_at)))
            .collect();
        *ICON_CACHE.lock().unwrap() = Some((icon_px, index));
    }

    // Publish names first: search works before the (slower) icons are in.
    // Only real apps have shell items and bitmap icons; commands draw a font glyph.
    let missing: Vec<usize> = (0..apps.len()).filter(|&i| apps[i].icon == 0).collect();
    publish(&apps);
    post(notify, msg);
    // Old icons no app uses any more (uninstalled apps, removed shortcuts).
    let kept: HashSet<isize> = apps.iter().map(|a| a.icon).collect();
    RETIRED_ICONS
        .lock()
        .unwrap()
        .extend(all_old.into_iter().filter(|icon| !kept.contains(icon)));

    if icon_px <= 0 {
        return Ok(());
    }
    let size = SIZE {
        cx: icon_px,
        cy: icon_px,
    };
    let mut fresh = HashSet::new();
    for &i in &missing {
        if let Ok(factory) = shell_items[i].cast::<IShellItemImageFactory>() {
            if let Ok(bmp) = factory.GetImage(size, SIIGBF_ICONONLY) {
                crate::icon_cache::premultiply_bitmap(bmp.0 as isize);
                apps[i].icon = bmp.0 as isize;
                fresh.insert(i);
            }
        }
    }
    if !missing.is_empty() {
        publish(&apps);
        post(notify, msg);
    }
    save_icon_cache(&apps, &stamps, &fresh, icon_px, now);
    Ok(())
}

/// Writes the icon cache if it no longer matches the apps (new, changed, removed or
/// reloaded icons, other icon size). `fresh`: apps whose icon was just loaded from the
/// shell (their cache age starts now).
unsafe fn save_icon_cache(
    apps: &[App],
    stamps: &[u64],
    fresh: &HashSet<usize>,
    icon_px: i32,
    now: u64,
) {
    let mut cache = ICON_CACHE.lock().unwrap();
    let previous = cache
        .as_ref()
        .filter(|(px, _)| *px == icon_px)
        .map(|(_, index)| index);
    let index: CacheIndex = apps
        .iter()
        .enumerate()
        .filter(|(_, a)| a.icon != 0)
        .map(|(i, a)| {
            let kept = previous
                .and_then(|p| p.get(&a.target))
                .filter(|(stamp, _)| *stamp == stamps[i] && !fresh.contains(&i));
            let cached_at = kept.map_or(now, |(_, at)| *at);
            (a.target.clone(), (stamps[i], cached_at))
        })
        .collect();
    if previous == Some(&index) {
        return;
    }
    let mut entries = Vec::new();
    for a in apps.iter().filter(|a| a.icon != 0) {
        let (stamp, cached_at) = index[&a.target];
        if let Some((width, height, pixels)) = crate::icon_cache::pixels_of(a.icon) {
            let cached = crate::icon_cache::Cached {
                stamp,
                cached_at,
                width,
                height,
                pixels,
            };
            entries.push((a.target.as_str(), cached));
        }
    }
    let refs: Vec<(&str, &crate::icon_cache::Cached)> =
        entries.iter().map(|(k, c)| (*k, c)).collect();
    crate::icon_cache::save(icon_px, &refs);
    *cache = Some((icon_px, index));
}

/// File types that count as shortcuts (`.url`: e.g. Steam games).
const SHORTCUT_EXTENSIONS: &[&str] = &["lnk", "url", "appref-ms"];

/// Where Windows keeps the shortcuts pinned to the taskbar, below the Quick Launch
/// folder (`%APPDATA%\Microsoft\Internet Explorer\Quick Launch`).
const TASKBAR_PINS: &str = r"User Pinned\TaskBar";

/// Shortcuts pinned to the taskbar, then those on the user's and the public desktop.
/// Most were put there by an installer or duplicate a Start menu entry: a shortcut
/// named like an app is only taken when it passes arguments (e.g. a slicer opening a
/// certain profile folder).
unsafe fn pinned_and_desktop_shortcuts(apps: &[App]) -> Vec<(App, IShellItem)> {
    let app_names: HashSet<String> = apps.iter().map(|a| a.name.to_lowercase()).collect();
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    let folders = [
        known_folder_path(&FOLDERID_QuickLaunch).map(|q| format!(r"{q}\{TASKBAR_PINS}")),
        known_folder_path(&FOLDERID_Desktop),
        known_folder_path(&FOLDERID_PublicDesktop),
    ];
    for dir in folders.into_iter().flatten() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for path in entries.flatten().map(|e| e.path()) {
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_lowercase)
                .unwrap_or_default();
            if !SHORTCUT_EXTENSIONS.contains(&extension.as_str()) {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let key = name.to_lowercase();
            let target = path.to_string_lossy().into_owned();
            // .url and .appref-ms have no program path to check.
            let (program, has_arguments) = if extension == "lnk" {
                link_details(&target).unwrap_or_default()
            } else {
                (String::new(), false)
            };
            let duplicate = app_names.contains(&key) && !has_arguments;
            if duplicate
                || name.trim().is_empty()
                || is_uninstaller(name, &program)
                || !seen.insert(key)
            {
                continue;
            }
            let Ok(item) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(&target), None)
            else {
                continue;
            };
            let app = App {
                search: Searchable::new(name),
                name: name.to_string(),
                target,
                shortcut: true,
                icon: 0,
                aliases: Vec::new(),
                command: None,
                custom: None,
                answer: None,
                file: None,
            };
            found.push((app, item));
        }
    }
    found
}

unsafe fn known_folder_path(folder: &windows::core::GUID) -> Option<String> {
    let p = SHGetKnownFolderPath(folder, KF_FLAG_DEFAULT, None).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s
}

/// The program a `.lnk` file starts and whether it passes command-line arguments.
unsafe fn link_details(lnk: &str) -> Option<(String, bool)> {
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
    link.cast::<IPersistFile>()
        .ok()?
        .Load(&HSTRING::from(lnk), STGM_READ)
        .ok()?;
    let mut path = [0u16; 1024];
    // SLGP_RAWPATH: the path as stored, without searching for a moved target (which
    // could hang on a disconnected network drive).
    let program = match link.GetPath(&mut path, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32) {
        Ok(()) => {
            let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
            String::from_utf16_lossy(&path[..len])
        }
        Err(_) => String::new(),
    };
    let mut args = [0u16; 1024];
    let has_arguments = link.GetArguments(&mut args).is_ok() && args[0] != 0;
    Some((program, has_arguments))
}

/// Version of the update that is downloaded and ready, if any.
pub fn update_version() -> Option<String> {
    UPDATE_VERSION.lock().unwrap().clone()
}

/// Called by the updater: adds (or removes) the "update WinLaunch" command.
pub fn set_update_version(version: Option<String>) {
    *UPDATE_VERSION.lock().unwrap() = version;
    republish();
}

/// Called by the updater once it knows WinLaunch is installed (not a dev build).
pub fn set_installed() {
    INSTALLED.store(true, Ordering::Release);
    republish();
}

/// Icons of custom commands by target, loaded in the background (0: none yet).
static CUSTOM_ICONS: Mutex<Option<HashMap<String, isize>>> = Mutex::new(None);
/// The custom icons were loaded at least once (the first app scan does it).
static CUSTOM_ICONS_LOADED: AtomicBool = AtomicBool::new(false);

/// Rereads the custom commands file if it changed and updates the index: at once with
/// glyphs, then again once their icons are loaded on a background thread.
pub fn reload_custom_commands(notify: usize, msg: u32, icon_px: i32) {
    if !crate::custom::reload_if_changed() {
        return;
    }
    republish();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        load_custom_icons(icon_px, notify, msg);
        CoUninitialize();
    });
}

/// Loads the icons of all custom commands (COM thread), puts them into the index and
/// tells the window. Replaced icons are retired like those of removed apps, but only
/// once the index no longer uses them: retired earlier, a scan starting meanwhile on
/// another thread would free them while they are still shown.
unsafe fn load_custom_icons(icon_px: i32, notify: usize, msg: u32) {
    // Serializes loads (startup scan and a file change may overlap).
    static LOADING_CUSTOM: Mutex<()> = Mutex::new(());
    let _loading = LOADING_CUSTOM.lock().unwrap();
    let mut icons = HashMap::new();
    let mut favicons_to_fetch = Vec::new();
    if icon_px > 0 {
        for c in crate::custom::commands().iter() {
            let url = c.url().filter(|_| c.icon.is_none());
            if let Some(url) = url {
                if crate::favicon::needs_fetch(url) {
                    favicons_to_fetch.push(url.to_string());
                }
            }
            // Explicit icon, then the site's favicon, then the automatic icon.
            let favicon = url.and_then(crate::favicon::cached);
            let path = match (favicon, c.icon_source()) {
                (_, Some(crate::custom::IconSource::File(path))) if c.icon.is_some() => Some(path),
                (Some(favicon), _) => Some(favicon),
                (None, Some(crate::custom::IconSource::File(path))) => Some(path),
                (None, Some(crate::custom::IconSource::Browser)) => default_browser(),
                (None, None) => None,
            };
            if let Some(icon) = path.and_then(|p| icon_of_path(&p, icon_px)) {
                icons.insert(c.target(), icon);
            }
        }
    }
    let old = CUSTOM_ICONS.lock().unwrap().replace(icons);
    republish();
    post(notify, msg);
    RETIRED_ICONS
        .lock()
        .unwrap()
        .extend(old.into_iter().flat_map(|m| m.into_values()));
    fetch_favicons(favicons_to_fetch, icon_px, notify, msg);
}

/// Downloads missing or old favicons on a background thread, then shows them.
fn fetch_favicons(urls: Vec<String>, icon_px: i32, notify: usize, msg: u32) {
    static FETCHING: AtomicBool = AtomicBool::new(false);
    if urls.is_empty() || FETCHING.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        let mut fetched = false;
        let mut origins_done = HashSet::new();
        for url in &urls {
            // One download per site, even if several commands use it.
            let origin = crate::net::split_url(url).map(|(s, h, p, _)| (s, h, p));
            if origins_done.insert(origin) {
                fetched |= crate::favicon::fetch(url);
            }
        }
        FETCHING.store(false, Ordering::Release);
        if fetched {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
                load_custom_icons(icon_px, notify, msg);
                CoUninitialize();
            }
        }
    });
}

/// The icon Explorer shows for a file or folder, as a premultiplied HBITMAP.
unsafe fn icon_of_path(path: &str, icon_px: i32) -> Option<isize> {
    let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
    let factory = item.cast::<IShellItemImageFactory>().ok()?;
    let size = SIZE {
        cx: icon_px,
        cy: icon_px,
    };
    let bmp = factory.GetImage(size, SIIGBF_ICONONLY).ok()?;
    crate::icon_cache::premultiply_bitmap(bmp.0 as isize);
    Some(bmp.0 as isize)
}

/// Program of the default web browser (the one that opens https links).
unsafe fn default_browser() -> Option<String> {
    use windows_sys::Win32::UI::Shell::{
        AssocQueryStringW, ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE,
    };
    let protocol = crate::wide("https");
    let verb = crate::wide("open");
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let hr = AssocQueryStringW(
        ASSOCF_IS_PROTOCOL,
        ASSOCSTR_EXECUTABLE,
        protocol.as_ptr(),
        verb.as_ptr(),
        buf.as_mut_ptr(),
        &mut len,
    );
    if hr != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

/// The user's own commands as index entries; the keyword is a search alias.
fn custom_items() -> Vec<App> {
    let icons = CUSTOM_ICONS.lock().unwrap();
    crate::custom::commands()
        .iter()
        .map(|c| App {
            search: Searchable::new(&c.name),
            name: c.name.clone(),
            target: c.target(),
            shortcut: false,
            icon: icons
                .as_ref()
                .and_then(|m| m.get(&c.target()).copied())
                .unwrap_or(0),
            aliases: c.keyword.iter().map(|k| Searchable::new(k)).collect(),
            command: None,
            custom: Some(Arc::new(c.clone())),
            answer: None,
            file: None,
        })
        .collect()
}

/// System commands as index entries, named in the display language. The English name
/// is a search alias where it differs.
fn command_items() -> Vec<App> {
    let update = update_version();
    let version = update.clone().unwrap_or_default();
    COMMANDS
        .iter()
        .chain(update.is_some().then_some(&UPDATE_COMMAND))
        .chain(
            INSTALLED
                .load(Ordering::Acquire)
                .then_some(&UNINSTALL_COMMAND),
        )
        .map(|cmd| {
            let id = cmd.message_id();
            let args = [("version", version.as_str())];
            let (name, english) = (t_args(&id, &args), t_en_args(&id, &args));
            let aliases = if english == name {
                Vec::new()
            } else {
                vec![Searchable::new(&english)]
            };
            App {
                search: Searchable::new(&name),
                name,
                target: cmd.target(),
                shortcut: false,
                icon: 0,
                aliases,
                command: Some(cmd),
                custom: None,
                answer: None,
                file: None,
            }
        })
        .collect()
}

/// `--list-apps`: scans synchronously (no icons) and prints what the search sees.
/// Diagnostic only; stdout is visible when redirected, e.g. `winlaunch --list-apps | more`.
/// Icon size 0: no icons, and the icon cache of the running launcher stays untouched.
pub fn print_list() {
    use std::io::Write;
    let start = Instant::now();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let _ = scan(0, 0, 0);
        CoUninitialize();
    }
    let elapsed = start.elapsed();
    let index = snapshot();
    let mut out = std::io::stdout().lock();
    for app in index.iter() {
        let kind = if app.shortcut { "\t(shortcut)" } else { "" };
        let _ = writeln!(out, "{}\t{}{kind}", app.name, app.target);
    }
    let icons = index.iter().filter(|a| a.icon != 0).count();
    let commands = index.iter().filter(|a| a.command.is_some()).count();
    let _ = writeln!(
        out,
        "# {} apps, {} commands, {} icons, scan took {:.1} ms",
        index.len() - commands,
        commands,
        icons,
        elapsed.as_secs_f64() * 1000.0
    );
}

/// Makes `apps` the searchable index. The commands are appended fresh on every publish:
/// the app scan and the updater publish independently, and whichever comes last must
/// not drop what the other added (e.g. the "update WinLaunch" command).
///
/// Built while holding the index lock, like `republish`: otherwise a publish could
/// overwrite one that happened meanwhile with older data.
fn publish(apps: &[App]) {
    let mut index = INDEX.lock().unwrap();
    *index = Some(Arc::new(build_index(apps)));
}

/// Rebuilds the index from its current apps, after the commands (update ready,
/// installed) or the custom commands changed. Read and written under one lock: built
/// from a snapshot taken earlier, it could put back what a scan had just replaced (lost
/// icons, or icons of removed apps that are already retired and then freed twice).
fn republish() {
    let mut index = INDEX.lock().unwrap();
    let apps = index.clone().unwrap_or_default();
    *index = Some(Arc::new(build_index(&apps)));
}

/// The apps of `apps` plus the current system and custom commands.
fn build_index(apps: &[App]) -> Vec<App> {
    let mut index: Vec<App> = apps
        .iter()
        .filter(|a| a.command.is_none() && a.custom.is_none())
        .cloned()
        .collect();
    index.extend(command_items());
    index.extend(custom_items());
    index
}

/// Starts an app or shortcut on a background thread. When WinLaunch runs
/// elevated (autostart task), it is started through Explorer so it does not inherit
/// admin rights.
pub fn launch(app: &App, elevated: bool) {
    if app.shortcut {
        open(app.target.clone(), elevated);
    } else {
        open(format!("shell:AppsFolder\\{}", app.target), elevated);
    }
}

/// Opens a file, shell item or URI (e.g. `ms-settings:display`) on a background
/// thread, unelevated like `launch`.
pub fn open(file: String, elevated: bool) {
    run(file, String::new(), None, elevated);
}

/// Starts a program (or opens a file) with arguments and an optional working folder
/// on a background thread, unelevated like `launch`.
///
/// When WinLaunch runs elevated and Explorer cannot start it (e.g. Explorer is not
/// running), nothing is started: falling back to our own ShellExecute would start it
/// with admin rights, and targets such as desktop shortcuts or commands.toml can be
/// changed by any unelevated program.
pub fn run(file: String, args: String, folder: Option<String>, elevated: bool) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let folder = folder.unwrap_or_default();
        if !elevated {
            shell_execute(&file, &args, &folder);
        } else if let Err(e) = launch_via_explorer(&file, &args, &folder) {
            crate::debug_log::log(&format!("not started (Explorer unavailable): {file}: {e}"));
        }
        CoUninitialize();
    });
}

unsafe fn shell_execute(file: &str, args: &str, folder: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let optional = |s: &str| (!s.is_empty()).then(|| crate::wide(s));
    let (file, verb) = (crate::wide(file), crate::wide("open"));
    let (args, folder) = (optional(args), optional(folder));
    ShellExecuteW(
        std::ptr::null_mut(),
        verb.as_ptr(),
        file.as_ptr(),
        args.as_ref().map_or(std::ptr::null(), |a| a.as_ptr()),
        folder.as_ref().map_or(std::ptr::null(), |f| f.as_ptr()),
        SW_SHOWNORMAL,
    );
}

/// Asks the desktop's Explorer to run the file (Raymond Chen's
/// "launch an unelevated process from an elevated one").
unsafe fn launch_via_explorer(file: &str, args: &str, folder: &str) -> windows::core::Result<()> {
    let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
    let desktop = VARIANT::from(0i32); // CSIDL_DESKTOP
    let empty = VARIANT::default();
    let mut hwnd = 0i32;
    let disp: IDispatch =
        windows.FindWindowSW(&desktop, &empty, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
    let provider: IServiceProvider = disp.cast()?;
    let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
    let view: IShellView = browser.QueryActiveShellView()?;
    let background: IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
    let folder_view: IShellFolderViewDual = background.cast()?;
    let shell: IShellDispatch2 = folder_view.Application()?.cast()?;
    shell.ShellExecute(
        &BSTR::from(file),
        &VARIANT::from(BSTR::from(args)),
        &VARIANT::from(BSTR::from(folder)),
        &VARIANT::from(BSTR::from("open")),
        &VARIANT::from(1i32), // SW_SHOWNORMAL
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstallers_by_program_or_whole_word() {
        // By the program they start
        assert!(is_uninstaller(
            "OrcaSlicer",
            r"{6D809377}\OrcaSlicer\Uninstall.exe"
        ));
        assert!(is_uninstaller("Remove Foo", r"C:\Foo\unins000.exe"));
        assert!(is_uninstaller("Foo", r"C:\Windows\System32\msiexec.exe"));
        // By a whole word of the name
        assert!(is_uninstaller("Uninstall OrcaSlicer", r"C:\Orca\orca.exe"));
        assert!(is_uninstaller("OrcaSlicer deinstallieren", ""));
        // Real apps
        assert!(!is_uninstaller("Geek Uninstaller", r"C:\Tools\geek.exe"));
        assert!(!is_uninstaller(
            "Rechner",
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
        ));
        assert!(!is_uninstaller("Unison", r"C:\Unison\unison.exe"));
    }
}
