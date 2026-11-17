//! Installation and updates with Velopack (https://velopack.io).
//!
//! * Setup.exe installs per user to `%LOCALAPPDATA%\WinLaunch`; the EXE lives in
//!   `...\current\winlaunch.exe`, a path that stays the same across updates, so the
//!   autostart task keeps working.
//! * Velopack's install/update/uninstall hooks (`--veloapp-*`) run first in `main` and
//!   exit right away. Everything else (first-run setup, update checks) runs on a
//!   background thread after the hotkey is live, so it never costs startup time.
//! * Updates come from the releases of this (public) repository. They are downloaded in the
//!   background, then WinLaunch shows a hint and an "update" command; the update is
//!   only applied when the user picks that command.
//! * `WINLAUNCH_UPDATE_SOURCE` overrides the feed (URL or local folder) for testing,
//!   but not in the elevated autostart instance.

use crate::debug_log::log;
use std::sync::Mutex;
use std::time::Duration;
use velopack::sources::AutoSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, VelopackApp};

const RELEASES: &str = "https://github.com/b00gie-dev/winlaunch";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Downloaded update waiting for the user to apply it.
static READY: Mutex<Option<(UpdateManager, Box<UpdateInfo>)>> = Mutex::new(None);

/// True if this process was started by Velopack to run a hook (`--veloapp-...`).
pub fn is_hook(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| a.starts_with("--veloapp-"))
}

/// Runs a Velopack hook and exits. Hooks must be quick and may not show any UI.
pub fn run_hook() {
    VelopackApp::build()
        // An update or uninstall replaces/deletes the EXE: the running launcher (maybe
        // elevated, which Velopack itself could not stop) has to go first.
        .on_before_update_fast_callback(|_| {
            crate::control::quit_running();
        })
        .on_before_uninstall_fast_callback(|_| crate::control::uninstall_running())
        .run(); // exits the process for every hook
}

/// Background part of startup: Velopack bookkeeping, autostart setup on the first run
/// after installation, then periodic update checks. `notify`/`msg` is posted to the UI
/// once an update is downloaded and ready.
pub fn start(notify: usize, msg: u32) {
    std::thread::spawn(move || {
        let mut first_run = false;
        log(&format!(
            "update thread: VELOPACK_FIRSTRUN={:?}",
            std::env::var("VELOPACK_FIRSTRUN").ok()
        ));
        VelopackApp::build()
            // Only apply updates when the user asks for it (see `apply`).
            .set_auto_apply_on_startup(false)
            .on_first_run(|_| first_run = true)
            .run();
        log(&format!("VelopackApp::run done, first_run={first_run}"));
        if first_run {
            // Asks for admin rights once (UAC); the elevated autostart instance then
            // replaces this one.
            let code = crate::control::install_cmd();
            log(&format!("install_cmd returned {code}"));
        }

        // The override is ignored while elevated (see `test_env`).
        let source = crate::test_env("WINLAUNCH_UPDATE_SOURCE")
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| RELEASES.into());
        // Fails when not installed through Velopack (e.g. a dev build): no updates then.
        let manager = match UpdateManager::new(AutoSource::new(&source), None, None) {
            Ok(manager) => manager,
            Err(e) => {
                log(&format!("no update manager: {e:?}"));
                *STATE.lock().unwrap() = State::NotInstalled;
                post(notify, msg);
                return;
            }
        };
        *INSTALLED_VERSION.lock().unwrap() = Some(manager.get_current_version_as_string());
        crate::apps::set_installed(); // offers "Uninstall WinLaunch" now
        *MANAGER.lock().unwrap() = Some(manager);
        post(notify, msg); // the footer can show the installed version now
        while !check(notify, msg, false) {
            std::thread::sleep(CHECK_EVERY);
        }
    });
}

/// What the footer shows about updates.
#[derive(Clone, PartialEq, Debug)]
pub enum State {
    /// Nothing to report (no check asked for, or none finished yet).
    Idle,
    /// A check the user asked for is running.
    Checking,
    /// The user's check found nothing new.
    UpToDate,
    /// A newer version is downloaded and can be applied.
    Ready(String),
    /// The user's check failed (no internet, GitHub unreachable, ...).
    Failed,
    /// Dev build, not installed through Velopack: updates are not possible.
    NotInstalled,
}

static STATE: Mutex<State> = Mutex::new(State::Idle);
/// Release notes of the downloaded update: (version, items).
static NOTES: Mutex<Option<(String, Vec<crate::changelog::Note>)>> = Mutex::new(None);

/// What the downloaded update brings, if one is ready and has notes.
pub fn pending_notes() -> Option<(String, Vec<crate::changelog::Note>)> {
    NOTES
        .lock()
        .unwrap()
        .clone()
        .filter(|(_, items)| !items.is_empty())
}
static MANAGER: Mutex<Option<UpdateManager>> = Mutex::new(None);
static INSTALLED_VERSION: Mutex<Option<String>> = Mutex::new(None);
/// Held while a check runs, so the periodic and a manual check never overlap.
static CHECKING: Mutex<()> = Mutex::new(());

pub fn state() -> State {
    STATE.lock().unwrap().clone()
}

/// Forgets results of a manual check ("up to date", "failed") once the window closes;
/// a ready update stays.
pub fn clear_transient_state() {
    let mut state = STATE.lock().unwrap();
    if matches!(*state, State::UpToDate | State::Failed | State::Checking) {
        *state = State::Idle;
    }
}

/// Installed version as Velopack knows it, or the version this EXE was built as
/// (dev build, or before the background thread has looked it up).
pub fn installed_version() -> (String, bool) {
    match INSTALLED_VERSION.lock().unwrap().clone() {
        Some(version) => (version, false),
        None => (
            env!("CARGO_PKG_VERSION").to_string(),
            *STATE.lock().unwrap() == State::NotInstalled,
        ),
    }
}

/// "Check for WinLaunch updates": checks right away on a background thread; the UI
/// gets `msg` when the state changes (checking, then up to date / ready / failed).
pub fn check_now(notify: usize, msg: u32) {
    {
        let mut state = STATE.lock().unwrap();
        match *state {
            State::Ready(_) | State::NotInstalled => return post(notify, msg),
            _ => *state = State::Checking,
        }
    }
    post(notify, msg);
    std::thread::spawn(move || {
        check(notify, msg, true);
    });
}

/// One update check; downloads a found update. Returns true once an update is ready.
/// `manual` checks report "up to date" and "failed" to the footer; the periodic ones
/// stay silent unless they find something.
fn check(notify: usize, msg: u32, manual: bool) -> bool {
    let _running = CHECKING.lock().unwrap();
    if matches!(state(), State::Ready(_)) {
        post(notify, msg);
        return true;
    }
    let Some(manager) = MANAGER.lock().unwrap().clone() else {
        // Not set up yet (startup still running) or not installed.
        if manual && state() == State::Checking {
            *STATE.lock().unwrap() = State::Failed;
            post(notify, msg);
        }
        return false;
    };
    let result = manager.check_for_updates();
    log(&format!(
        "update check: {}",
        match &result {
            Ok(UpdateCheck::UpdateAvailable(i)) =>
                format!("available {}", i.TargetFullRelease.Version),
            Ok(UpdateCheck::NoUpdateAvailable) => "no update".into(),
            Ok(UpdateCheck::RemoteIsEmpty) => "feed empty".into(),
            Err(e) => format!("error {e:?}"),
        }
    ));
    let new_state = match result {
        Ok(UpdateCheck::UpdateAvailable(info)) => match manager.download_updates(&info, None) {
            Ok(()) => {
                let version = info.TargetFullRelease.Version.to_string();
                let notes = crate::changelog::bullets(&info.TargetFullRelease.NotesMarkdown);
                *NOTES.lock().unwrap() = Some((version.clone(), notes));
                *READY.lock().unwrap() = Some((manager.clone(), info));
                crate::apps::set_update_version(Some(version.clone()));
                State::Ready(version)
            }
            Err(e) => {
                log(&format!("download failed: {e:?}"));
                State::Failed
            }
        },
        Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty) => State::UpToDate,
        Err(_) => State::Failed,
    };
    let ready = matches!(new_state, State::Ready(_));
    if manual || ready {
        *STATE.lock().unwrap() = new_state;
        post(notify, msg);
    }
    ready
}

fn post(notify: usize, msg: u32) {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(notify as _, msg, 0, 0);
    }
}

/// Applies the downloaded update and restarts WinLaunch. Does not return on success.
/// The restarted process inherits this one's rights, so the elevated autostart
/// instance stays elevated. If applying fails, the update stays ready, so the update
/// command can be picked again (it used to stay in the list but do nothing).
pub fn apply() {
    // Cloned, not taken: on success the process ends anyway.
    let ready = READY.lock().unwrap().clone();
    if let Some((manager, info)) = ready {
        if let Err(e) = manager.apply_updates_and_restart(&*info) {
            log(&format!("applying the update failed: {e:?}"));
        }
    }
}

/// "Uninstall WinLaunch": after a confirmation, starts Velopack's uninstaller, the same
/// one "Installed apps" runs. It calls our uninstall hook, which makes this instance
/// remove the autostart task and exit (see `control::uninstall_running`).
pub fn uninstall() {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let question = crate::wide(&crate::i18n::t("confirm-uninstall"));
    let answer = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            question.as_ptr(),
            crate::wide("WinLaunch").as_ptr(),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND | MB_TOPMOST,
        )
    };
    if answer != IDYES {
        return;
    }
    // Layout of a Velopack install: <root>\Update.exe and <root>\current\winlaunch.exe.
    let update_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("Update.exe")));
    match update_exe {
        Some(path) if path.is_file() => {
            if let Err(e) = std::process::Command::new(&path).arg("--uninstall").spawn() {
                log(&format!("uninstall: cannot start {path:?}: {e}"));
            }
        }
        other => log(&format!("uninstall: no Update.exe ({other:?})")),
    }
}
