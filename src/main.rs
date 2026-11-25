// WinLaunch: a minimal, instant-start launcher for Windows.
//
//   winlaunch               run the launcher (Win+Space opens it, typing searches apps)
//   winlaunch --quit        stop the running instance (e.g. before an update)
//   winlaunch --install     autostart at logon via Task Scheduler (asks for admin once)
//   winlaunch --uninstall   stop the launcher and remove the autostart task
#![windows_subsystem = "windows"]

mod apps;
mod calc;
mod changelog;
mod commands;
mod control;
mod currency;
mod custom;
mod debug_log;
mod favicon;
mod filesearch;
mod i18n;
mod icon_cache;
mod net;
mod search;
mod ui;
mod units;
mod update;
mod usage;

use std::ffi::OsString;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::{OpenEventW, EVENT_MODIFY_STATE};

pub const CLASS_NAME: &str = "WinLaunch.Main";
pub const MUTEX_NAME: &str = "Local\\WinLaunch";
pub const QUIT_MSG_NAME: &str = "WinLaunch.Quit";
/// Sent by a second start of WinLaunch (e.g. from the Start menu) to show the window.
pub const SHOW_MSG_NAME: &str = "WinLaunch.Show";
/// Sent by the uninstall hook: the running instance removes the autostart task (it
/// usually runs elevated and may delete it, the hook may not) and exits.
pub const UNINSTALL_MSG_NAME: &str = "WinLaunch.Uninstall";
pub const TASK_NAME: &str = "WinLaunch";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opens a named event for signalling (used by the benchmark flags).
///
/// # Safety
/// Plain Win32 call; the returned handle may be null if the event does not exist.
pub unsafe fn open_event(name: &str) -> HANDLE {
    OpenEventW(EVENT_MODIFY_STATE, 0, wide(name).as_ptr())
}

/// Overrides for tests and debugging (`WINLAUNCH_DATA_DIR`, `WINLAUNCH_LOG`,
/// `WINLAUNCH_UPDATE_SOURCE`). Ignored while running elevated: user environment
/// variables can be set by any program without admin rights, and they would redirect
/// the update feed or make the elevated instance write to protected folders.
pub fn test_env(name: &str) -> Option<OsString> {
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    if *ELEVATED.get_or_init(control::is_elevated) {
        return None;
    }
    std::env::var_os(name)
}

/// `%LOCALAPPDATA%` + `WinLaunch`: history, caches, rates. `WINLAUNCH_DATA_DIR` replaces
/// it for tests (not while elevated).
pub fn local_data_dir() -> Option<std::path::PathBuf> {
    match test_env("WINLAUNCH_DATA_DIR") {
        Some(dir) => Some(dir.into()),
        None => Some(std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("WinLaunch")),
    }
}

/// Full path of a program in the Windows system folder (System32), so it is never
/// looked up next to winlaunch.exe, a folder any program may write to.
pub fn system_exe(name: &str) -> String {
    in_folder(
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW,
        name,
    )
}

/// Full path of a program in the Windows folder (e.g. explorer.exe).
pub fn windows_exe(name: &str) -> String {
    in_folder(
        windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW,
        name,
    )
}

/// `name` in the folder that `get_folder` (GetSystemDirectoryW-like) returns.
fn in_folder(get_folder: unsafe extern "system" fn(*mut u16, u32) -> u32, name: &str) -> String {
    let mut buf = [0u16; 260];
    let len = unsafe { get_folder(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if len == 0 || len >= buf.len() {
        return name.to_string();
    }
    format!(r"{}\{name}", String::from_utf16_lossy(&buf[..len]))
}

fn main() {
    // First of all: DLLs only from System32. The autostart instance runs elevated from
    // %LOCALAPPDATA%\WinLaunch\current, a folder any program may write to; by default
    // Windows looks for DLLs there first, so a planted DLL would run with admin rights.
    // The DLLs that are not "known DLLs" are delay-loaded (build.rs), so this applies to
    // them as well.
    unsafe {
        use windows_sys::Win32::System::LibraryLoader::{
            SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32,
        };
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
    let args: Vec<String> = std::env::args().collect();
    // Velopack install/update/uninstall hooks: handled and exited before anything else.
    if update::is_hook(&args) {
        update::run_hook();
        return;
    }
    let code = match args.get(1).map(String::as_str) {
        Some("--quit") => {
            control::quit_running();
            0
        }
        Some("--install") => control::install_cmd(),
        Some("--uninstall") => control::uninstall_cmd(),
        Some("--list-apps") => {
            apps::print_list();
            0
        }
        // internal: re-launched with admin rights by --install / --uninstall
        Some("--install-elevated") => {
            control::install(args.get(2).map(String::as_str).unwrap_or(""))
        }
        Some("--uninstall-elevated") => control::uninstall(),
        _ => {
            ui::run(&args);
            0
        }
    };
    std::process::exit(code);
}
