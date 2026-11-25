//! The launcher window: a hidden popup with a text field, toggled by Win+Space.
//! Typing shows matching apps below the field; Up/Down select, Enter launches.

use crate::commands::{self, Kind};
use crate::i18n::{t, t_args};
use crate::search::{
    is_command_query, is_custom_query, is_file_query, rank_items, strip_custom_prefix,
    strip_file_prefix, Query,
};
use crate::SHOW_MSG_NAME;
use crate::{apps, open_event, update, usage, wide, CLASS_NAME, MUTEX_NAME, QUIT_MSG_NAME};
use crate::{control, UNINSTALL_MSG_NAME};
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_TOGGLE: u32 = WM_APP + 1;
/// Posted by the app index thread when names or icons are available.
const WM_INDEX_UPDATED: u32 = WM_APP + 2;
/// Posted by the file search thread when results are ready.
const WM_FILES_READY: u32 = WM_APP + 3;
/// Timer that waits for the Start menu to close before the launcher shows.
const TIMER_AFTER_START_MENU: usize = 1;
/// Check every 30 ms, give up waiting after 15 checks (~0.5 s) and show anyway.
const START_MENU_POLL_MS: u32 = 30;
const START_MENU_MAX_POLLS: u32 = 15;
const EM_SETSEL: u32 = 0x00B1;
const EN_CHANGE: u32 = 0x0300;
const EDIT_ID: usize = 1;
const MARKER: usize = 0x4C41_554E; // tags our own injected input
const VK_DUMMY: u16 = 0xE8; // unassigned VK, stops Win-up from opening Start
const MAX_ROWS: usize = 8;
/// Entries from here on in `State::results` are file search results (`State::files`).
const FILE_BASE: usize = usize::MAX / 2;
/// Entries from here on (below `FILE_BASE`) are release note rows (`State::notes`).
const NOTE_BASE: usize = usize::MAX / 4;
/// Entry in `State::results` that stands for the calculator row (`State::answer`).
const ANSWER: usize = usize::MAX;
/// Most-used apps shown while the field is empty.
const MAX_SUGGESTIONS: usize = 6;

// Layout in 96-DPI pixels, scaled once at startup.
const BASE_W: i32 = 680;
const BASE_INPUT_H: i32 = 60;
const BASE_ROW_H: i32 = 44;
const BASE_LIST_PAD: i32 = 6;
const BASE_HEADER_H: i32 = 26;
const BASE_ICON: i32 = 24;

const BG: u32 = rgb(0x20, 0x20, 0x20);
const SEPARATOR: u32 = rgb(0x33, 0x33, 0x33);
const SELECTION: u32 = rgb(0x34, 0x34, 0x34);
const TEXT: u32 = rgb(0xF0, 0xF0, 0xF0);
const SUBTLE: u32 = rgb(0x8A, 0x8A, 0x8A);
const PLACEHOLDER_TEXT: u32 = rgb(0x6E, 0x6E, 0x6E);
/// Light blue like Windows 11's dark-mode accent, for the update hint.
const ACCENT: u32 = rgb(0x60, 0xCD, 0xFF);
/// Height of the footer with version and update status (96 DPI).
const BASE_FOOTER_H: i32 = 28;

static mut MAIN: HWND = null_mut();
static mut EDIT: HWND = null_mut();
/// Original window procedure of the EDIT control (we subclass it for the placeholder).
static mut EDIT_PROC: WNDPROC = None;
static mut SHOWN_EVENT: HANDLE = null_mut();
static mut QUIT_MSG: u32 = 0;
static mut SHOW_MSG: u32 = 0;
static mut UNINSTALL_MSG: u32 = 0;
/// Checks left while waiting for the Start menu to close (UI thread only).
static mut START_MENU_POLLS: u32 = 0;
/// Space went down as part of Win+Space; its key-up is swallowed too (hook thread only).
static mut SPACE_HELD: bool = false;
/// The hook is reinstalled this often (see `hook_thread`).
const HOOK_REFRESH_MS: u32 = 5 * 60 * 1000;

/// Sizes, fonts and brushes; set once in `run`, read-only afterwards.
#[derive(Clone, Copy)]
struct Style {
    w: i32,
    input_h: i32,
    row_h: i32,
    list_pad: i32,
    header_h: i32,
    icon: i32,
    radius: i32,
    margin: i32,
    bg: HBRUSH,
    separator: HBRUSH,
    selection: HBRUSH,
    name_font: HFONT,
    label_font: HFONT,
    glyph_font: HFONT,
    footer_h: i32,
}

/// Mutable UI state, main thread only. Keep borrows short: never hold one across a
/// Win32 call that can send messages back into `wnd_proc`.
struct State {
    style: Style,
    index: Arc<Vec<apps::App>>,
    results: Vec<usize>,
    /// Title above the list: most-used apps (empty field) or all commands ("/").
    header: Option<String>,
    selected: usize,
    /// First visible row; lists longer than MAX_ROWS scroll.
    scroll: usize,
    /// "?" mode: the text after the "?" (custom commands take their query from it).
    custom_text: Option<String>,
    /// Calculator row, listed in `results` as `ANSWER`.
    answer: Option<apps::App>,
    /// File search results, listed in `results` as `FILE_BASE + i`.
    files: Vec<apps::App>,
    /// Release note rows, listed in `results` as `NOTE_BASE + i`.
    notes: Vec<apps::App>,
    /// The list shows "what's new" after an update; marked as seen when it hides.
    showing_whats_new: bool,
    /// Number of the newest file search; older results are dropped.
    file_generation: u64,
    last_mouse: (i32, i32),
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// The index entry (or calculator row) a result refers to.
fn entry(st: &State, i: usize) -> &apps::App {
    match &st.answer {
        Some(answer) if i == ANSWER => answer,
        _ if i >= FILE_BASE && i != ANSWER => &st.files[i - FILE_BASE],
        _ if (NOTE_BASE..FILE_BASE).contains(&i) => &st.notes[i - NOTE_BASE],
        _ => &st.index[i],
    }
}

/// Frees the icons of the shown file results and forgets them.
fn clear_files(st: &mut State) {
    for file in st.files.drain(..) {
        if file.icon != 0 {
            unsafe { DeleteObject(file.icon as _) };
        }
    }
}

/// "%" mode: starts a search for the text and keeps showing the previous results
/// until the new ones arrive (WM_FILES_READY).
unsafe fn update_file_results(text: &str) {
    let (generation, icon) = with_state(|st| {
        st.file_generation += 1;
        st.header = None;
        st.answer = None;
        st.custom_text = None;
        st.results = (0..st.files.len()).map(|i| FILE_BASE + i).collect();
        st.selected = 0;
        st.scroll = 0;
        (st.file_generation, st.style.icon)
    });
    crate::filesearch::search(
        generation,
        strip_file_prefix(text),
        icon,
        MAIN as usize,
        WM_FILES_READY,
    );
    resize();
    InvalidateRect(MAIN, null(), 0);
    InvalidateRect(EDIT, null(), 1);
}

/// WM_FILES_READY: shows the newest file results if they belong to the current text.
unsafe fn show_file_results() {
    let Some(results) = crate::filesearch::take_results() else {
        return;
    };
    let current =
        is_file_query(&edit_text()) && with_state(|st| st.file_generation == results.generation);
    if !current {
        crate::filesearch::free_icons(&results.hits);
        return;
    }
    with_state(|st| {
        clear_files(st);
        st.files = results.hits.iter().map(apps::App::for_file).collect();
        st.results = (0..st.files.len()).map(|i| FILE_BASE + i).collect();
        st.selected = 0;
        st.scroll = 0;
    });
    resize();
    InvalidateRect(MAIN, null(), 0);
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(s.borrow_mut().as_mut().expect("UI state not initialised")))
}

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}

unsafe extern "system" fn kb_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(lp as *const KBDLLHOOKSTRUCT);
        if k.dwExtraInfo != MARKER && k.vkCode as u16 == VK_SPACE {
            let down = wp == WM_KEYDOWN as usize || wp == WM_SYSKEYDOWN as usize;
            // Ask Windows for the real state of the Win keys instead of tracking their
            // events: a release on the secure desktop (Win+L, UAC prompt) never reaches
            // this hook, and a tracked Win key would then stay "held" and turn every
            // later Space into Win+Space.
            let win = GetAsyncKeyState(VK_LWIN as i32) < 0 || GetAsyncKeyState(VK_RWIN as i32) < 0;
            if win || SPACE_HELD {
                if down && !SPACE_HELD {
                    // Fake keystroke while Win is held: Windows won't treat the
                    // later Win-up as "Win pressed alone" and won't open Start.
                    let mut inp: [INPUT; 2] = zeroed();
                    for (i, flags) in [(0usize, 0u32), (1, KEYEVENTF_KEYUP)] {
                        inp[i].r#type = INPUT_KEYBOARD;
                        inp[i].Anonymous.ki = KEYBDINPUT {
                            wVk: VK_DUMMY,
                            wScan: 0,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: MARKER,
                        };
                    }
                    SendInput(2, inp.as_ptr(), size_of::<INPUT>() as i32);
                    PostMessageW(MAIN, WM_TOGGLE, 0, 0);
                }
                SPACE_HELD = down;
                return 1; // swallow: no layout switch
            }
        }
    }
    CallNextHookEx(null_mut(), code, wp, lp)
}

/// Runs the keyboard hook on a thread of its own. Windows calls a low-level hook through
/// its thread's message loop: on the UI thread, any slow paint or file access would
/// delay every keystroke system-wide, and a hook that answers too late is silently
/// removed (Win+Space would stop working). Reinstalled every few minutes, so even a
/// removal under heavy load (e.g. right after resume) heals itself.
/// Returns once the hook is installed (or failed to install).
unsafe fn start_hook_thread() {
    let (installed_tx, installed_rx) = std::sync::mpsc::sync_channel::<bool>(1);
    std::thread::spawn(move || {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        let install =
            || SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook), GetModuleHandleW(null()), 0);
        let mut hook = install();
        let _ = installed_tx.send(!hook.is_null());
        if hook.is_null() {
            crate::debug_log::log(&format!("keyboard hook failed: error {}", GetLastError()));
        }
        SetTimer(null_mut(), 0, HOOK_REFRESH_MS, None);
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == WM_TIMER {
                // New hook first, then the old one: no moment without a hook.
                let fresh = install();
                if !fresh.is_null() {
                    if !hook.is_null() {
                        UnhookWindowsHookEx(hook);
                    }
                    hook = fresh;
                }
            }
        }
    });
    let _ = installed_rx.recv();
}

unsafe fn force_foreground(hwnd: HWND) {
    if SetForegroundWindow(hwnd) != 0 && GetForegroundWindow() == hwnd {
        return;
    }
    let fg = GetForegroundWindow();
    let fg_thread = GetWindowThreadProcessId(fg, null_mut());
    let me = GetCurrentThreadId();
    AttachThreadInput(me, fg_thread, 1);
    BringWindowToTop(hwnd);
    SetForegroundWindow(hwnd);
    AttachThreadInput(me, fg_thread, 0);
}

unsafe fn edit_text() -> String {
    let len = GetWindowTextLengthW(EDIT);
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(EDIT, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Normalised query text for the launch history: lowercase, single spaces.
fn usage_key(text: &str) -> String {
    crate::search::strip_command_prefix(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Ranks the index entries in `pool` (indices into `index`) by `query`.
fn rank_pool(
    query: &Query,
    index: &[apps::App],
    pool: &[usize],
    limit: usize,
    boost: impl Fn(usize) -> i32,
) -> Vec<usize> {
    let items = pool.iter().map(|&i| {
        let a = &index[i];
        (a.name.as_str(), &a.search, a.aliases.as_slice())
    });
    rank_items(query, items, limit, |j| boost(pool[j]))
        .into_iter()
        .map(|j| pool[j])
        .collect()
}

/// Results of the "?" mode: custom commands whose keyword starts the text come first
/// (they take the rest as their query), then the others ranked by the whole text.
/// "?" alone lists all in file order. "Edit custom commands" is always reachable.
fn custom_results(index: &[apps::App], text: &str) -> Vec<usize> {
    let edit = crate::commands::edit_custom_commands();
    let pool: Vec<usize> = (0..index.len())
        .filter(|&i| {
            index[i].custom.is_some() || index[i].command.is_some_and(|c| std::ptr::eq(c, edit))
        })
        .collect();
    if text.trim().is_empty() {
        // File order, the edit command last.
        let mut all = pool;
        all.sort_by_key(|&i| index[i].command.is_some());
        return all;
    }
    let mut results: Vec<usize> = pool
        .iter()
        .copied()
        .filter(|&i| {
            index[i]
                .custom
                .as_ref()
                .is_some_and(|c| c.query_in(text).is_some())
        })
        .collect();
    let query = Query::new(text);
    for i in rank_pool(&query, index, &pool, pool.len(), |_| 0) {
        if !results.contains(&i) {
            results.push(i);
        }
    }
    results
}

/// Re-runs the search for the current text and resizes the window to fit.
unsafe fn update_results() {
    let text = edit_text();
    if is_file_query(&text) {
        return update_file_results(&text);
    }
    with_state(clear_files);
    let query = Query::new(&text);
    let index = apps::snapshot();
    let history = usage::snapshot();
    let (key, now) = (usage_key(&text), usage::now());
    let custom_text = is_custom_query(&text).then(|| strip_custom_prefix(&text).to_string());
    let command_mode = is_command_query(&text);
    let (mut results, mut header) = if let Some(custom_text) = &custom_text {
        let results = custom_results(&index, custom_text);
        let header = match crate::custom::error() {
            Some(e) => Some(t_args("custom-commands-error", &[("details", &e)])),
            None => custom_text
                .trim()
                .is_empty()
                .then(|| t("custom-commands-title")),
        };
        (results, header)
    } else if command_mode {
        // "/": only commands and settings; "/" alone lists all of them, used ones first.
        let pool: Vec<usize> = (0..index.len())
            .filter(|&i| index[i].command.is_some())
            .collect();
        let results = if query.is_empty() {
            let mut all = pool;
            // Stable: unused commands keep the table order.
            all.sort_by_key(|&i| -history.boost(&key, &index[i].target, now));
            all
        } else {
            rank_pool(&query, &index, &pool, pool.len(), |i| {
                history.boost(&key, &index[i].target, now)
            })
        };
        let header = query.is_empty().then(|| t("commands-title"));
        (results, header)
    } else if query.is_empty() {
        // Empty field: most-used apps that are still installed.
        let by_target: HashMap<&str, usize> = index
            .iter()
            .enumerate()
            .map(|(i, a)| (a.target.as_str(), i))
            .collect();
        let results = history
            .most_used(now)
            .into_iter()
            .filter_map(|t| by_target.get(t).copied())
            .filter(|&i| index[i].custom.is_none()) // custom commands only with "?"
            .take(MAX_SUGGESTIONS)
            .collect();
        (results, Some(t("suggestions-title")))
    } else {
        // Everything but custom commands (those only with "?").
        let pool: Vec<usize> = (0..index.len())
            .filter(|&i| index[i].custom.is_none())
            .collect();
        let results = rank_pool(&query, &index, &pool, MAX_ROWS, |i| {
            history.boost(&key, &index[i].target, now)
        });
        (results, None)
    };
    // Release notes: once after an update instead of the most-used apps, and in "/"
    // mode under the command that applies a downloaded update.
    let mut notes = Vec::new();
    let mut whats_new = false;
    let note_rows = |items: &[crate::changelog::Note], version: &str| -> Vec<apps::App> {
        let label = t_args("notes-label", &[("version", version)]);
        items
            .iter()
            .map(|item| {
                apps::App::for_answer(crate::calc::Answer {
                    display: item.text.clone(),
                    copy: String::new(), // Enter does nothing on a note
                    label: Some(label.clone()),
                    glyph: item.kind.glyph(),
                })
            })
            .collect()
    };
    if custom_text.is_none() && !command_mode && query.is_empty() {
        if let Some((version, items)) = crate::changelog::whats_new() {
            notes = note_rows(&items, &version);
            results = (0..notes.len()).map(|i| NOTE_BASE + i).collect();
            header = Some(t_args("whats-new-title", &[("version", &version)]));
            whats_new = true;
        }
    } else if command_mode {
        if let Some((version, items)) = update::pending_notes() {
            let update = &crate::commands::UPDATE_COMMAND;
            let position = results
                .iter()
                .position(|&i| index[i].command.is_some_and(|c| std::ptr::eq(c, update)));
            if let Some(position) = position {
                notes = note_rows(&items, &version);
                let rows: Vec<usize> = (0..notes.len()).map(|i| NOTE_BASE + i).collect();
                results.splice(position + 1..position + 1, rows);
            }
        }
    }

    // Calculator, currency or unit converter: the result comes first (normal search only).
    let language = crate::i18n::language();
    let answer = (custom_text.is_none() && !command_mode)
        .then(|| {
            crate::calc::answer(&text, language)
                .or_else(|| {
                    crate::currency::answer(&text, language, MAIN as usize, WM_INDEX_UPDATED)
                })
                .or_else(|| crate::units::answer(&text, language))
        })
        .flatten()
        .map(apps::App::for_answer);
    if answer.is_some() {
        results.insert(0, ANSWER);
    }
    with_state(|st| {
        st.header = header.filter(|_| !results.is_empty());
        st.results = results;
        st.index = index;
        st.selected = 0;
        st.scroll = 0;
        st.custom_text = custom_text;
        st.answer = answer;
        st.notes = notes;
        st.showing_whats_new |= whats_new;
    });
    resize();
    InvalidateRect(MAIN, null(), 0);
    InvalidateRect(EDIT, null(), 1); // placeholder appears/disappears
}

/// Client-area y where the first result row starts.
fn list_top(st: &State) -> i32 {
    let s = st.style;
    let header = if st.header.is_some() { s.header_h } else { 0 };
    s.input_h + 1 + s.list_pad + header
}

/// Number of rows the window shows (the rest is reached by scrolling).
fn visible_rows(st: &State) -> usize {
    st.results.len().min(MAX_ROWS)
}

/// Selects `row` and scrolls just enough to show it.
fn select(st: &mut State, row: usize) {
    st.selected = row.min(st.results.len().saturating_sub(1));
    if st.selected < st.scroll {
        st.scroll = st.selected;
    } else if st.selected >= st.scroll + MAX_ROWS {
        st.scroll = st.selected + 1 - MAX_ROWS;
    }
}

unsafe fn window_height() -> i32 {
    with_state(|st| {
        let content = if st.results.is_empty() {
            st.style.input_h
        } else {
            list_top(st) + visible_rows(st) as i32 * st.style.row_h + st.style.list_pad
        };
        content + st.style.footer_h
    })
}

unsafe fn resize() {
    let w = with_state(|st| st.style.w);
    let h = window_height();
    SetWindowPos(
        MAIN,
        null_mut(),
        0,
        0,
        w,
        h,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

unsafe fn show() {
    let (w, icon) = with_state(|st| (st.style.w, st.style.icon));
    apps::refresh_if_stale(MAIN as usize, WM_INDEX_UPDATED, icon);
    apps::reload_custom_commands(MAIN as usize, WM_INDEX_UPDATED, icon); // edits of commands.toml
    update_results();

    let mut pt: POINT = zeroed();
    GetCursorPos(&mut pt);
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(mon, &mut mi);
    let wa = mi.rcWork;
    let x = wa.left + ((wa.right - wa.left) - w) / 2;
    let y = wa.top + (wa.bottom - wa.top) / 4;
    // Remember where the resting cursor is relative to the window, so the WM_MOUSEMOVE
    // that Windows sends when the window appears under it does not change the selection.
    let mut cursor = pt;
    cursor.x -= x;
    cursor.y -= y;
    with_state(|st| st.last_mouse = (cursor.x, cursor.y));
    SetWindowPos(MAIN, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_SHOWWINDOW);
    force_foreground(MAIN);
    SetFocus(EDIT);
    SendMessageW(EDIT, EM_SETSEL, 0, -1);
    RedrawWindow(MAIN, null(), null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
    if !SHOWN_EVENT.is_null() {
        SetEvent(SHOWN_EVENT);
    }
}

/// True if `hwnd` is the Start menu or Windows search (Windows 10/11): a CoreWindow of
/// StartMenuExperienceHost.exe, SearchHost.exe or SearchApp.exe. While one of them is
/// in front, it keeps the focus and swallows what is typed into the launcher.
unsafe fn is_start_menu(hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    let mut class = [0u16; 64];
    let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
    if String::from_utf16_lossy(&class[..len.max(0) as usize]) != "Windows.UI.Core.CoreWindow" {
        return false;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if process.is_null() {
        return false;
    }
    let mut path = [0u16; 260];
    let mut size = path.len() as u32;
    let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut size) != 0;
    CloseHandle(process);
    if !ok {
        return false;
    }
    let path = String::from_utf16_lossy(&path[..size as usize]).to_lowercase();
    [
        "startmenuexperiencehost.exe",
        "searchhost.exe",
        "searchapp.exe",
    ]
    .iter()
    .any(|exe| path.ends_with(exe))
}

/// Win+Space: shows the launcher. If the Start menu (or search) is open, it is closed
/// first with Escape, and the launcher shows only once it is gone: shown right away,
/// the Start menu would keep the focus, and the Escape could reach the launcher.
unsafe fn show_after_start_menu() {
    if !is_start_menu(GetForegroundWindow()) {
        show();
        return;
    }
    let mut inp: [INPUT; 2] = zeroed();
    for (i, flags) in [(0usize, 0u32), (1, KEYEVENTF_KEYUP)] {
        inp[i].r#type = INPUT_KEYBOARD;
        inp[i].Anonymous.ki = KEYBDINPUT {
            wVk: VK_ESCAPE,
            wScan: 0,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: MARKER,
        };
    }
    SendInput(2, inp.as_ptr(), size_of::<INPUT>() as i32);
    START_MENU_POLLS = START_MENU_MAX_POLLS;
    SetTimer(MAIN, TIMER_AFTER_START_MENU, START_MENU_POLL_MS, None);
}

/// WM_TIMER while waiting for the Start menu to close.
unsafe fn start_menu_poll() {
    START_MENU_POLLS = START_MENU_POLLS.saturating_sub(1);
    if is_start_menu(GetForegroundWindow()) && START_MENU_POLLS > 0 {
        return; // still closing
    }
    KillTimer(MAIN, TIMER_AFTER_START_MENU);
    if IsWindowVisible(MAIN) == 0 {
        show();
    }
}

unsafe fn hide() {
    ShowWindow(MAIN, SW_HIDE);
    update::clear_transient_state();
    if with_state(|st| std::mem::take(&mut st.showing_whats_new)) {
        crate::changelog::mark_seen(); // "what's new" is shown once per version
    }
}

/// Launches the selected (or given) result and closes the launcher.
unsafe fn launch(row: Option<usize>) {
    launch_with(row, false);
}

/// Launches a result; `reveal`: for a file, show it in Explorer instead (Ctrl+Enter).
unsafe fn launch_with(row: Option<usize>, reveal: bool) {
    let picked = with_state(|st| {
        let row = row.unwrap_or(st.selected);
        st.results.get(row).map(|&i| entry(st, i).clone())
    });
    let Some(app) = picked else {
        return;
    };
    if let Some(folder) = app.file {
        AllowSetForegroundWindow(ASFW_ANY);
        let elevated = crate::control::is_elevated();
        if reveal {
            let args = format!("/select,\"{}\"", app.target);
            apps::run(crate::windows_exe("explorer.exe"), args, None, elevated);
        } else {
            let _ = folder; // folders and files both open with their default program
            apps::open(app.target.clone(), elevated);
        }
        hide();
        SetWindowTextW(EDIT, wide("").as_ptr());
        return;
    }
    if let Some(answer) = &app.answer {
        if answer.copy.is_empty() {
            return; // e.g. currency rates still loading
        }
        copy_to_clipboard(&answer.copy);
        hide();
        SetWindowTextW(EDIT, wide("").as_ptr());
        return;
    }
    let command = app.command;
    let text = edit_text();
    if let Some(custom) = &app.custom {
        // Only the command counts in the history, not what was typed after its keyword.
        usage::record("", &app.target);
        let query = custom.query_in(strip_custom_prefix(&text)).unwrap_or("");
        AllowSetForegroundWindow(ASFW_ANY);
        custom.execute(query, crate::control::is_elevated());
        hide();
        SetWindowTextW(EDIT, wide("").as_ptr());
        return;
    }
    usage::record(&usage_key(&text), &app.target);
    if command.is_some_and(|c| c.stays_open()) {
        // "Check for WinLaunch updates": keep the window, the footer shows the result.
        update::check_now(MAIN as usize, WM_INDEX_UPDATED);
        SetWindowTextW(EDIT, wide("").as_ptr());
        return;
    }
    // We are still in the foreground here: let the new app take it over.
    AllowSetForegroundWindow(ASFW_ANY);
    let elevated = crate::control::is_elevated();
    match command {
        Some(cmd) => commands::run(cmd, elevated),
        None => apps::launch(&app, elevated),
    }
    hide();
    SetWindowTextW(EDIT, wide("").as_ptr()); // EN_CHANGE collapses the list
}

/// Puts `text` on the clipboard (Unicode text).
unsafe fn copy_to_clipboard(text: &str) {
    use windows_sys::Win32::System::DataExchange::*;
    use windows_sys::Win32::System::Memory::*;
    const CF_UNICODETEXT: u32 = 13;
    let data = wide(text);
    let bytes = data.len() * 2;
    if OpenClipboard(MAIN) == 0 {
        return;
    }
    EmptyClipboard();
    let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
    if !handle.is_null() {
        let dest = GlobalLock(handle) as *mut u16;
        if dest.is_null() {
            GlobalFree(handle);
        } else {
            std::ptr::copy_nonoverlapping(data.as_ptr(), dest, data.len());
            GlobalUnlock(handle);
            // The clipboard owns the memory from here on.
            if SetClipboardData(CF_UNICODETEXT, handle as _).is_null() {
                GlobalFree(handle);
            }
        }
    }
    CloseClipboard();
}

/// WinLaunch is already running (e.g. started again from the Start menu): bring up the
/// running instance's window instead of silently doing nothing.
unsafe fn show_running_instance() {
    let hwnd = FindWindowW(wide(CLASS_NAME).as_ptr(), null());
    if hwnd.is_null() {
        return;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    // We were just started by the user, so we may hand the foreground over.
    AllowSetForegroundWindow(pid);
    PostMessageW(
        hwnd,
        RegisterWindowMessageW(wide(SHOW_MSG_NAME).as_ptr()),
        0,
        0,
    );
}

/// Ctrl is held without Alt (Ctrl+Alt is AltGr on German keyboards, e.g. for "@").
unsafe fn ctrl_only() -> bool {
    GetKeyState(VK_CONTROL as i32) < 0 && GetKeyState(VK_MENU as i32) >= 0
}

/// Ctrl+Backspace: deletes the selection, or the word before the caret.
unsafe fn delete_word_before_caret() {
    const EM_GETSEL: u32 = 0x00B0;
    const EM_REPLACESEL: u32 = 0x00C2;
    let (mut start, mut end) = (0u32, 0u32);
    SendMessageW(
        EDIT,
        EM_GETSEL,
        &mut start as *mut u32 as WPARAM,
        &mut end as *mut u32 as LPARAM,
    );
    if start == end {
        let len = GetWindowTextLengthW(EDIT);
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(EDIT, buf.as_mut_ptr(), buf.len() as i32);
        buf.truncate(n.max(0) as usize);
        start = crate::search::word_start_before(&buf, end as usize) as u32;
        SendMessageW(EDIT, EM_SETSEL, start as WPARAM, end as LPARAM);
    }
    let empty = wide("");
    // wParam 1: the deletion can be undone with Ctrl+Z
    SendMessageW(EDIT, EM_REPLACESEL, 1, empty.as_ptr() as LPARAM);
}

unsafe fn move_selection(delta: isize) {
    let changed = with_state(|st| {
        let n = st.results.len() as isize;
        if n == 0 {
            return false;
        }
        let row = (st.selected as isize + delta).rem_euclid(n) as usize;
        select(st, row);
        true
    });
    if changed {
        InvalidateRect(MAIN, null(), 0);
    }
}

/// Mouse wheel: scrolls the list by `rows` and keeps the selection in view.
unsafe fn scroll_by(rows: isize) {
    let changed = with_state(|st| {
        let max = st.results.len().saturating_sub(MAX_ROWS) as isize;
        let scroll = (st.scroll as isize + rows).clamp(0, max) as usize;
        if scroll == st.scroll {
            return false;
        }
        st.scroll = scroll;
        st.selected = st.selected.clamp(scroll, scroll + MAX_ROWS - 1);
        true
    });
    if changed {
        InvalidateRect(MAIN, null(), 0);
    }
}

/// Result row under a client-area y coordinate.
fn row_at(st: &State, y: i32) -> Option<usize> {
    let top = list_top(st);
    if y < top {
        return None;
    }
    let row = ((y - top) / st.style.row_h) as usize;
    (row < visible_rows(st)).then_some(st.scroll + row)
}

/// Rectangle of result `row` (an index into `results`; must be visible).
fn row_rect(st: &State, row: usize) -> RECT {
    let s = st.style;
    let top = list_top(st) + (row - st.scroll) as i32 * s.row_h;
    RECT {
        left: s.margin,
        top,
        right: s.w - s.margin,
        bottom: top + s.row_h,
    }
}

/// Thin bar at the bottom: installed version on the left, update status on the right.
unsafe fn paint_footer(dc: HDC, s: &Style, bottom: i32) {
    let top = bottom - s.footer_h;
    let line = RECT {
        left: 0,
        top,
        right: s.w,
        bottom: top + 1,
    };
    FillRect(dc, &line, s.separator);

    let (version, dev) = update::installed_version();
    let left = if dev {
        t_args("footer-version-dev", &[("version", &version)])
    } else {
        t_args("footer-version", &[("version", &version)])
    };
    let (right, color) = match update::state() {
        update::State::Idle => (String::new(), SUBTLE),
        update::State::Checking => (t("update-status-checking"), SUBTLE),
        update::State::UpToDate => (t("update-status-current"), SUBTLE),
        update::State::Ready(v) => (t_args("update-hint", &[("version", &v)]), ACCENT),
        update::State::Failed => (t("update-status-failed"), SUBTLE),
        update::State::NotInstalled => (t("update-status-not-installed"), SUBTLE),
    };

    SetBkMode(dc, TRANSPARENT as _);
    let old = SelectObject(dc, s.label_font);
    let pad = s.margin * 2;
    let mut text_rc = RECT {
        left: pad,
        top: top + 1,
        right: s.w - pad,
        bottom,
    };
    let flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
    SetTextColor(dc, SUBTLE);
    let left = wide(&left);
    DrawTextW(dc, left.as_ptr(), -1, &mut text_rc, DT_LEFT | flags);
    if !right.is_empty() {
        SetTextColor(dc, color);
        let right = wide(&right);
        DrawTextW(dc, right.as_ptr(), -1, &mut text_rc, DT_RIGHT | flags);
    }
    paint_footer_help(dc, s, top + 1, bottom);
    SelectObject(dc, old);
}

/// Centre of the footer: the prefixes of the search modes, "/ Commands  ? Custom
/// commands  % Files". The keys are brighter than their labels.
unsafe fn paint_footer_help(dc: HDC, s: &Style, top: i32, bottom: i32) {
    let gap = s.margin * 3;
    let parts = [
        ("/ ".to_string(), TEXT, 0),
        (t("footer-help-commands"), SUBTLE, gap),
        ("? ".to_string(), TEXT, 0),
        (t("footer-help-custom"), SUBTLE, gap),
        ("% ".to_string(), TEXT, 0),
        (t("footer-help-files"), SUBTLE, 0),
    ];
    let measured: Vec<(Vec<u16>, u32, i32, SIZE)> = parts
        .into_iter()
        .map(|(text, color, after)| {
            let text: Vec<u16> = text.encode_utf16().collect();
            let mut size: SIZE = zeroed();
            GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
            (text, color, after, size)
        })
        .collect();
    let total: i32 = measured
        .iter()
        .map(|(_, _, after, size)| size.cx + after)
        .sum();
    let mut x = (s.w - total) / 2;
    for (text, color, after, size) in &measured {
        SetTextColor(dc, *color);
        let y = top + (bottom - top - size.cy) / 2;
        TextOutW(dc, x, y, text.as_ptr(), text.len() as i32);
        x += size.cx + after;
    }
}

/// Windows' shortcut arrow as a premultiplied bitmap: (pixel size it was made for,
/// bitmap, width, height). Made on first use, remade when the size changes (DPI).
static mut ARROW: (i32, HBITMAP, i32, i32) = (0, null_mut(), 0, 0);

/// Draws Windows' shortcut arrow, fitted into a `size` square at (x, y).
unsafe fn draw_shortcut_arrow(dc: HDC, x: i32, y: i32, size: i32) {
    if ARROW.0 != size {
        if !ARROW.1.is_null() {
            DeleteObject(ARROW.1);
        }
        ARROW = match shortcut_arrow(size) {
            Some((bmp, w, h)) => (size, bmp, w, h),
            None => (size, null_mut(), 0, 0),
        };
    }
    let (_, bmp, w, h) = ARROW;
    if bmp.is_null() {
        return;
    }
    let scale = size as f32 / w.max(h) as f32;
    let (dw, dh) = (
        ((w as f32 * scale).round() as i32).max(1),
        ((h as f32 * scale).round() as i32).max(1),
    );
    let src = CreateCompatibleDC(dc);
    let old = SelectObject(src, bmp);
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    GdiAlphaBlend(
        dc,
        x + (size - dw) / 2,
        y + (size - dh) / 2,
        dw,
        dh,
        src,
        0,
        0,
        w,
        h,
        blend,
    );
    SelectObject(src, old);
    DeleteDC(src);
}

/// Cuts the arrow out of the stock shortcut overlay, an icon-sized image with the arrow
/// in one corner. The overlay is loaded at the size where the arrow alone comes out at
/// about `size` pixels, so it is drawn by Windows at that size and stays sharp.
unsafe fn shortcut_arrow(size: i32) -> Option<(HBITMAP, i32, i32)> {
    use windows_sys::Win32::UI::Shell::*;
    let mut info: SHSTOCKICONINFO = zeroed();
    info.cbSize = size_of::<SHSTOCKICONINFO>() as u32;
    if SHGetStockIconInfo(SIID_LINK, SHGSI_ICONLOCATION, &mut info) != 0 {
        return None;
    }
    const PROBE: i32 = 256;
    let (x0, y0, x1, y1) = opaque_bounds(&icon_pixels(&info, PROBE)?, PROBE)?;
    let fraction = (x1 - x0).max(y1 - y0) as f32 / PROBE as f32;
    let px = ((size as f32 / fraction).round() as i32).clamp(16, PROBE);
    let pixels = icon_pixels(&info, px)?;
    let (x0, y0, x1, y1) = opaque_bounds(&pixels, px)?;
    let (w, h) = (x1 - x0, y1 - y0);

    let mut bmi: BITMAPINFO = zeroed();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    let mut bits: *mut core::ffi::c_void = null_mut();
    let bmp = CreateDIBSection(null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if bmp.is_null() || bits.is_null() {
        return None;
    }
    let out = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            out[(y * w + x) as usize] = premultiply(pixels[((y0 + y) * px + x0 + x) as usize]);
        }
    }
    Some((bmp, w, h))
}

/// The icon at `info`'s location, rendered at `px` pixels, as top-down BGRA pixels
/// with straight (not premultiplied) alpha.
unsafe fn icon_pixels(
    info: &windows_sys::Win32::UI::Shell::SHSTOCKICONINFO,
    px: i32,
) -> Option<Vec<u32>> {
    use windows_sys::Win32::UI::Shell::SHDefExtractIconW;
    let mut icon: HICON = null_mut();
    if SHDefExtractIconW(
        info.szPath.as_ptr(),
        info.iIcon,
        0,
        &mut icon,
        null_mut(),
        px as u32,
    ) != 0
        || icon.is_null()
    {
        return None;
    }
    let mut ii: ICONINFO = zeroed();
    let ok = GetIconInfo(icon, &mut ii) != 0;
    let mut pixels = vec![0u32; (px * px) as usize];
    let mut read = 0;
    if ok && !ii.hbmColor.is_null() {
        let mut bmi: BITMAPINFO = zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = px;
        bmi.bmiHeader.biHeight = -px;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let dc = GetDC(null_mut());
        read = GetDIBits(
            dc,
            ii.hbmColor,
            0,
            px as u32,
            pixels.as_mut_ptr() as _,
            &mut bmi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(null_mut(), dc);
    }
    if ok {
        DeleteObject(ii.hbmColor);
        DeleteObject(ii.hbmMask);
    }
    DestroyIcon(icon);
    (read == px).then_some(pixels)
}

/// Bounding box (x0, y0, x1, y1; end exclusive) of the pixels that are not transparent.
fn opaque_bounds(pixels: &[u32], px: i32) -> Option<(i32, i32, i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (px, px, 0, 0);
    for y in 0..px {
        for x in 0..px {
            if pixels[(y * px + x) as usize] >> 24 != 0 {
                (x0, y0) = (x0.min(x), y0.min(y));
                (x1, y1) = (x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// BGRA with straight alpha → premultiplied, as AlphaBlend expects.
fn premultiply(p: u32) -> u32 {
    let a = p >> 24;
    let channel = |shift: u32| (((p >> shift) & 0xFF) * a / 255) << shift;
    (a << 24) | channel(16) | channel(8) | channel(0)
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);

    // Double buffered: draw everything off-screen, then copy in one go.
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
    let old_bmp = SelectObject(mem, bmp);

    with_state(|st| {
        let s = st.style;
        FillRect(mem, &rc, s.bg);
        paint_footer(mem, &s, rc.bottom);
        if st.results.is_empty() {
            return;
        }
        let sep = RECT {
            left: 0,
            top: s.input_h,
            right: s.w,
            bottom: s.input_h + 1,
        };
        FillRect(mem, &sep, s.separator);

        SetBkMode(mem, TRANSPARENT as _);
        let icon_dc = CreateCompatibleDC(mem);
        let old_pen = SelectObject(mem, GetStockObject(NULL_PEN));
        let old_brush = SelectObject(mem, s.selection);
        let old_font = SelectObject(mem, s.name_font);

        if let Some(header) = &st.header {
            let title = wide(header);
            let first = row_rect(st, st.scroll);
            let mut title_rc = RECT {
                left: first.left + (s.row_h - s.icon) / 2,
                top: first.top - s.header_h,
                right: first.right,
                bottom: first.top,
            };
            SelectObject(mem, s.label_font);
            SetTextColor(mem, SUBTLE);
            DrawTextW(
                mem,
                title.as_ptr(),
                -1,
                &mut title_rc,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
        }

        let kind_labels = [
            t("result-kind-application"),
            t("result-kind-setting"),
            t("result-kind-command"),
            t("result-kind-shortcut"),
            t("result-kind-custom"),
            t("result-kind-calculator"),
            t("result-kind-file"),
            t("result-kind-folder"),
        ];
        if st.results.len() > MAX_ROWS {
            // Scroll bar: a thin thumb in the right margin.
            let track_top = row_rect(st, st.scroll).top;
            let track = MAX_ROWS as i32 * s.row_h;
            let len = st.results.len() as i32;
            let thumb_top = track_top + track * st.scroll as i32 / len;
            let thumb_h = (track * MAX_ROWS as i32 / len).max(s.margin);
            let x = s.w - s.margin / 2;
            let thumb = RECT {
                left: x - 1,
                top: thumb_top,
                right: x + 1,
                bottom: thumb_top + thumb_h,
            };
            let brush = CreateSolidBrush(SUBTLE);
            FillRect(mem, &thumb, brush);
            DeleteObject(brush);
        }

        for (row, &i) in st.results.iter().enumerate().skip(st.scroll).take(MAX_ROWS) {
            let app = entry(st, i);
            let r = row_rect(st, row);
            if row == st.selected {
                RoundRect(
                    mem,
                    r.left,
                    r.top,
                    r.right + 1,
                    r.bottom + 1,
                    s.radius,
                    s.radius,
                );
            }

            let pad = (s.row_h - s.icon) / 2;
            let icon_x = r.left + pad;
            let icon_y = r.top + (s.row_h - s.icon) / 2;
            if let Some(glyph) = app.glyph().filter(|_| app.icon == 0) {
                // Commands (and custom commands until their icon is loaded) have no
                // bitmap icon: draw a glyph of the Windows icon font.
                let glyph = wide(glyph.encode_utf8(&mut [0u8; 4]));
                let mut glyph_rc = RECT {
                    left: icon_x,
                    top: icon_y,
                    right: icon_x + s.icon,
                    bottom: icon_y + s.icon,
                };
                SelectObject(mem, s.glyph_font);
                SetTextColor(mem, TEXT);
                DrawTextW(
                    mem,
                    glyph.as_ptr(),
                    -1,
                    &mut glyph_rc,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                );
            } else if app.icon != 0 {
                let hbm = app.icon as HBITMAP;
                let mut info: BITMAP = zeroed();
                GetObjectW(hbm, size_of::<BITMAP>() as i32, &mut info as *mut _ as _);
                let old_icon = SelectObject(icon_dc, hbm);
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                GdiAlphaBlend(
                    mem,
                    icon_x,
                    icon_y,
                    s.icon,
                    s.icon,
                    icon_dc,
                    0,
                    0,
                    info.bmWidth,
                    info.bmHeight,
                    blend,
                );
                SelectObject(icon_dc, old_icon);
            }
            // Shortcuts: Windows' shortcut arrow as a second, smaller icon to the right.
            let arrow = if app.shortcut { s.icon * 2 / 3 } else { 0 };
            if app.shortcut {
                draw_shortcut_arrow(
                    mem,
                    icon_x + s.icon + pad / 2,
                    r.top + (s.row_h - arrow) / 2,
                    arrow,
                );
            }

            let answer_label = app.answer.as_ref().and_then(|a| a.label.as_ref());
            let label = wide(match app.command.map(|c| c.kind) {
                None if answer_label.is_some() => answer_label.unwrap(),
                None if app.file == Some(false) => &kind_labels[6],
                None if app.file == Some(true) => &kind_labels[7],
                None if app.answer.is_some() => &kind_labels[5],
                None if app.custom.is_some() => &kind_labels[4],
                None if app.shortcut => &kind_labels[3],
                None => &kind_labels[0],
                Some(Kind::Setting) => &kind_labels[1],
                Some(Kind::System) => &kind_labels[2],
            });
            let mut label_rc = RECT {
                left: r.right - s.icon * 5,
                right: r.right - pad,
                ..r
            };
            SelectObject(mem, s.label_font);
            SetTextColor(mem, SUBTLE);
            DrawTextW(
                mem,
                label.as_ptr(),
                -1,
                &mut label_rc,
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );

            // A search command shows what it will search for: "Google: rust gdi".
            let query = app
                .custom
                .as_ref()
                .zip(st.custom_text.as_deref())
                .and_then(|(c, text)| c.query_in(text))
                .filter(|q| !q.is_empty());
            let name = match query {
                Some(q) => wide(&format!("{}: {q}", app.name)),
                None => wide(&app.name),
            };
            let mut name_rc = RECT {
                left: icon_x + s.icon + pad + if arrow > 0 { arrow + pad / 2 } else { 0 },
                right: label_rc.left - pad,
                ..r
            };
            SelectObject(mem, s.name_font);
            SetTextColor(mem, TEXT);
            let flags = DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
            DrawTextW(mem, name.as_ptr(), -1, &mut name_rc, flags);
            // Files: the folder they are in, grey, after the name.
            if app.file.is_some() {
                let mut measure = name_rc;
                DrawTextW(mem, name.as_ptr(), -1, &mut measure, flags | DT_CALCRECT);
                let parent = std::path::Path::new(&app.target)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut folder_rc = RECT {
                    left: measure.right + pad,
                    ..name_rc
                };
                if folder_rc.left < folder_rc.right && !parent.is_empty() {
                    SelectObject(mem, s.label_font);
                    SetTextColor(mem, SUBTLE);
                    let parent = wide(&parent);
                    DrawTextW(
                        mem,
                        parent.as_ptr(),
                        -1,
                        &mut folder_rc,
                        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_PATH_ELLIPSIS,
                    );
                }
            }
        }

        SelectObject(mem, old_font);
        SelectObject(mem, old_brush);
        SelectObject(mem, old_pen);
        DeleteDC(icon_dc);
    });

    BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bmp);
    DeleteObject(bmp);
    DeleteDC(mem);
    EndPaint(hwnd, &ps);
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TOGGLE => {
            if IsWindowVisible(hwnd) != 0 {
                hide();
            } else {
                show_after_start_menu();
            }
            0
        }
        WM_TIMER if wp == TIMER_AFTER_START_MENU => {
            start_menu_poll();
            0
        }
        m if m != 0 && m == SHOW_MSG => {
            if IsWindowVisible(hwnd) == 0 {
                show();
            }
            0
        }
        m if m != 0 && m == QUIT_MSG => {
            DestroyWindow(hwnd);
            0
        }
        m if m != 0 && m == UNINSTALL_MSG => {
            control::remove_autostart_quietly();
            DestroyWindow(hwnd);
            0
        }
        WM_FILES_READY => {
            show_file_results();
            0
        }
        WM_INDEX_UPDATED => {
            if IsWindowVisible(hwnd) != 0 {
                let selected = with_state(|st| st.selected);
                update_results();
                with_state(|st| select(st, selected));
            }
            0
        }
        WM_COMMAND => {
            if (wp >> 16) as u32 == EN_CHANGE && (wp & 0xFFFF) == EDIT_ID {
                update_results();
            }
            0
        }
        WM_MOUSEMOVE => {
            // Windows also sends WM_MOUSEMOVE when the window grows under a resting
            // cursor; only a real mouse movement should change the selection.
            let (x, y) = (
                (lp & 0xFFFF) as i16 as i32,
                ((lp >> 16) & 0xFFFF) as i16 as i32,
            );
            let changed = with_state(|st| {
                if st.last_mouse == (x, y) {
                    return false;
                }
                st.last_mouse = (x, y);
                match row_at(st, y) {
                    Some(row) if row != st.selected => {
                        st.selected = row;
                        true
                    }
                    _ => false,
                }
            });
            if changed {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            if let Some(row) = with_state(|st| row_at(st, y)) {
                launch(Some(row));
            }
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xFFFF) as u32 == WA_INACTIVE {
                hide();
            }
            0
        }
        WM_ERASEBKGND => 1, // everything is painted in WM_PAINT
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_CTLCOLOREDIT => {
            let hdc = wp as HDC;
            SetTextColor(hdc, TEXT);
            SetBkColor(hdc, BG);
            with_state(|st| st.style.bg) as LRESULT
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Subclassed EDIT procedure: draws the grey placeholder while the field is empty.
/// (The built-in EM_SETCUEBANNER needs comctl32 v6, which would slow down startup.)
unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let result = CallWindowProcW(EDIT_PROC, hwnd, msg, wp, lp);
    if msg == WM_PAINT && GetWindowTextLengthW(hwnd) == 0 {
        const EM_GETRECT: u32 = 0x00B2;
        let mut rc: RECT = zeroed();
        SendMessageW(hwnd, EM_GETRECT, 0, &mut rc as *mut RECT as LPARAM);
        let hdc = GetDC(hwnd);
        let old_font = SelectObject(hdc, SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT);
        SetBkMode(hdc, TRANSPARENT as _);
        SetTextColor(hdc, PLACEHOLDER_TEXT);
        let text = wide(&t("search-placeholder"));
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut rc,
            DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, old_font);
        ReleaseDC(hwnd, hdc);
    }
    result
}

unsafe fn font(height: i32, weight: i32) -> HFONT {
    font_face("Segoe UI", height, weight)
}

unsafe fn font_face(face: &str, height: i32, weight: i32) -> HFONT {
    let mut lf: LOGFONTW = zeroed();
    lf.lfHeight = -height;
    lf.lfWeight = weight;
    lf.lfQuality = CLEARTYPE_QUALITY as _;
    for (i, c) in face.encode_utf16().enumerate() {
        lf.lfFaceName[i] = c;
    }
    CreateFontIndirectW(&lf)
}

/// Runs the launcher until it is told to quit.
/// `--ready <event>`: signalled once the hook is live (benchmarking).
/// `--shown <event>`: signalled after the window has been shown and painted.
pub fn run(args: &[String]) {
    unsafe {
        let mut ready: HANDLE = null_mut();
        let mut i = 1;
        while i + 1 < args.len() {
            match args[i].as_str() {
                "--ready" => ready = open_event(&args[i + 1]),
                "--shown" => SHOWN_EVENT = open_event(&args[i + 1]),
                _ => {}
            }
            i += 2;
        }

        // Single instance
        CreateMutexW(null(), 1, wide(MUTEX_NAME).as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            show_running_instance();
            return;
        }

        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
        let dpi = GetDpiForSystem() as i32;
        let s = |v: i32| v * dpi / 96;
        let style = Style {
            w: s(BASE_W),
            input_h: s(BASE_INPUT_H),
            row_h: s(BASE_ROW_H),
            list_pad: s(BASE_LIST_PAD),
            header_h: s(BASE_HEADER_H),
            icon: s(BASE_ICON),
            radius: s(10),
            margin: s(8),
            bg: CreateSolidBrush(BG),
            separator: CreateSolidBrush(SEPARATOR),
            selection: CreateSolidBrush(SELECTION),
            name_font: font(s(16), 400),
            label_font: font(s(13), 400),
            footer_h: s(BASE_FOOTER_H),
            glyph_font: font_face("Segoe MDL2 Assets", s(18), 400),
        };
        STATE.with(|st| {
            *st.borrow_mut() = Some(State {
                style,
                index: Arc::default(),
                results: Vec::new(),
                header: None,
                scroll: 0,
                custom_text: None,
                answer: None,
                files: Vec::new(),
                notes: Vec::new(),
                showing_whats_new: false,
                file_generation: 0,
                selected: 0,
                last_mouse: (-1, -1),
            })
        });

        let hinst = GetModuleHandleW(null());
        let class = wide(CLASS_NAME);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: null_mut(),
        };
        RegisterClassExW(&wc);

        MAIN = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            wide("WinLaunch").as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            style.w,
            style.input_h,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        let round: u32 = DWMWCP_ROUND as u32;
        DwmSetWindowAttribute(
            MAIN,
            DWMWA_WINDOW_CORNER_PREFERENCE as _,
            &round as *const u32 as _,
            4,
        );
        let dark: u32 = 1;
        DwmSetWindowAttribute(
            MAIN,
            DWMWA_USE_IMMERSIVE_DARK_MODE as _,
            &dark as *const u32 as _,
            4,
        );

        // `winlaunch --quit` may run without admin rights while we run elevated
        // (autostart task). UIPI drops such messages unless explicitly allowed.
        QUIT_MSG = RegisterWindowMessageW(wide(QUIT_MSG_NAME).as_ptr());
        ChangeWindowMessageFilterEx(MAIN, QUIT_MSG, MSGFLT_ALLOW, null_mut());
        SHOW_MSG = RegisterWindowMessageW(wide(SHOW_MSG_NAME).as_ptr());
        ChangeWindowMessageFilterEx(MAIN, SHOW_MSG, MSGFLT_ALLOW, null_mut());
        UNINSTALL_MSG = RegisterWindowMessageW(wide(UNINSTALL_MSG_NAME).as_ptr());
        ChangeWindowMessageFilterEx(MAIN, UNINSTALL_MSG, MSGFLT_ALLOW, null_mut());

        let pad = s(16);
        let edit_h = s(32);
        EDIT = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            null(),
            WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
            pad,
            (style.input_h - edit_h) / 2,
            style.w - 2 * pad,
            edit_h,
            MAIN,
            EDIT_ID as HMENU,
            hinst,
            null(),
        );
        SendMessageW(EDIT, WM_SETFONT, font(s(24), 400) as WPARAM, 1);
        EDIT_PROC = std::mem::transmute::<isize, WNDPROC>(SetWindowLongPtrW(
            EDIT,
            GWLP_WNDPROC,
            edit_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT as usize
                as isize,
        ));

        start_hook_thread();
        if !ready.is_null() {
            SetEvent(ready);
        }

        // Ready for Win+Space; now build the app index in the background.
        apps::refresh(MAIN as usize, WM_INDEX_UPDATED, style.icon);
        update::start(MAIN as usize, WM_INDEX_UPDATED);

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == WM_MOUSEWHEEL {
                // Arrives at the focused EDIT control; scrolls the list instead.
                let notches = (msg.wParam >> 16) as u16 as i16 as isize / WHEEL_DELTA as isize;
                scroll_by(-notches);
                continue;
            }
            if msg.message == WM_KEYDOWN {
                match msg.wParam as u16 {
                    VK_ESCAPE => {
                        hide();
                        continue;
                    }
                    VK_RETURN => {
                        // Ctrl+Enter on a file: show it in Explorer.
                        launch_with(None, ctrl_only()); // hides unless the command stays open
                        continue;
                    }
                    VK_DOWN => {
                        move_selection(1);
                        continue;
                    }
                    VK_UP => {
                        move_selection(-1);
                        continue;
                    }
                    // The classic single-line EDIT control knows neither Ctrl+A nor
                    // Ctrl+Backspace (it inserts a box character). Handled here instead,
                    // before TranslateMessage would turn them into WM_CHAR.
                    k if ctrl_only() && k == b'A' as u16 => {
                        SendMessageW(EDIT, EM_SETSEL, 0, -1);
                        continue;
                    }
                    VK_BACK if ctrl_only() => {
                        delete_word_before_caret();
                        continue;
                    }
                    _ => {}
                }
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_bounds_finds_the_arrow_corner() {
        let px = 8;
        let mut pixels = vec![0u32; 64];
        for (x, y) in [(1, 5), (3, 7), (2, 6)] {
            pixels[y * 8 + x] = 0xFF00_0000;
        }
        assert_eq!(opaque_bounds(&pixels, px), Some((1, 5, 4, 8)));
        assert_eq!(opaque_bounds(&[0; 64], px), None);
    }

    #[test]
    fn premultiply_scales_colors_by_alpha() {
        assert_eq!(premultiply(0xFFFF_8040), 0xFFFF_8040);
        assert_eq!(premultiply(0x80FF_FFFF), 0x8080_8080);
        assert_eq!(premultiply(0x00FF_FFFF), 0);
    }
}
