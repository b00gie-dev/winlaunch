//! System commands that show up in the search next to apps: Windows settings pages
//! ("Check for updates") and session actions (lock, sleep, restart, ...).
//!
//! Display names come from the translations (`cmd-<id>`); the English name is added as
//! a search alias, so "shutdown" also finds "Herunterfahren".

use std::os::windows::process::CommandExt;
use std::process::Command as Process;
use std::ptr::{null, null_mut};
use windows_sys::Win32::System::Power::SetSuspendState;
use windows_sys::Win32::System::Shutdown::LockWorkStation;
use windows_sys::Win32::UI::Shell::SHEmptyRecycleBinW;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Prefix of a command's target string (used as its key in the launch history).
const TARGET_PREFIX: &str = "cmd:";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// Opens a page of the Windows Settings app.
    Setting,
    /// Acts on the session or the system.
    System,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    /// `ms-settings:` URI
    Uri(&'static str),
    Lock,
    Sleep,
    SignOut,
    Restart,
    Shutdown,
    EmptyRecycleBin,
    /// Applies the downloaded WinLaunch update and restarts.
    ApplyUpdate,
    /// Checks for WinLaunch updates now; the result shows in the footer.
    CheckWinLaunchUpdates,
    /// Uninstalls WinLaunch (after a confirmation).
    Uninstall,
    /// Opens the file with the user's own commands (creates it first).
    EditCustomCommands,
}

#[derive(Debug)]
pub struct Command {
    /// Stable id: message `cmd-<id>` and history key `cmd:<id>`. Never rename.
    pub id: &'static str,
    /// Icon: a glyph of the "Segoe MDL2 Assets" font that ships with Windows.
    pub glyph: char,
    pub kind: Kind,
    action: Action,
}

const fn setting(id: &'static str, glyph: char, uri: &'static str) -> Command {
    Command {
        id,
        glyph,
        kind: Kind::Setting,
        action: Action::Uri(uri),
    }
}

const fn system(id: &'static str, glyph: char, action: Action) -> Command {
    Command {
        id,
        glyph,
        kind: Kind::System,
        action,
    }
}

pub static COMMANDS: &[Command] = &[
    // "-action" opens Windows Update and starts the check right away.
    setting(
        "check-updates",
        '\u{E895}',
        "ms-settings:windowsupdate-action",
    ),
    setting("windows-update", '\u{E777}', "ms-settings:windowsupdate"),
    setting("bluetooth", '\u{E702}', "ms-settings:bluetooth"),
    setting("wifi", '\u{E701}', "ms-settings:network-wifi"),
    setting("display", '\u{E7F4}', "ms-settings:display"),
    setting("sound", '\u{E767}', "ms-settings:sound"),
    setting("installed-apps", '\u{E71D}', "ms-settings:appsfeatures"),
    system("lock", '\u{E72E}', Action::Lock),
    system("sleep", '\u{E708}', Action::Sleep),
    system("sign-out", '\u{E748}', Action::SignOut),
    system("restart", '\u{E72C}', Action::Restart),
    system("shutdown", '\u{E7E8}', Action::Shutdown),
    system("empty-recycle-bin", '\u{E74D}', Action::EmptyRecycleBin),
    system(
        "check-winlaunch-updates",
        '\u{E946}',
        Action::CheckWinLaunchUpdates,
    ),
    system(
        "edit-custom-commands",
        '\u{E70F}',
        Action::EditCustomCommands,
    ),
];

/// "Edit custom commands" also shows in the "?" list (custom commands).
pub fn edit_custom_commands() -> &'static Command {
    COMMANDS
        .iter()
        .find(|c| matches!(c.action, Action::EditCustomCommands))
        .expect("edit-custom-commands is in COMMANDS")
}

/// Shown only while a downloaded update is waiting (see update.rs). Its name needs the
/// version, so it is not part of COMMANDS.
pub static UPDATE_COMMAND: Command = system("apply-update", '\u{E896}', Action::ApplyUpdate);

/// Shown only in the installed version (a dev build has nothing to uninstall).
pub static UNINSTALL_COMMAND: Command =
    system("uninstall-winlaunch", '\u{E74D}', Action::Uninstall);

impl Command {
    pub fn target(&self) -> String {
        format!("{TARGET_PREFIX}{}", self.id)
    }

    pub fn message_id(&self) -> String {
        format!("cmd-{}", self.id)
    }

    /// The launcher stays open after this command, to show its result in the footer.
    pub fn stays_open(&self) -> bool {
        matches!(self.action, Action::CheckWinLaunchUpdates)
    }
}

/// Runs a command in the background. Settings pages are opened like apps (through
/// Explorer when elevated). Sign out, restart and shut down act at once, like in the
/// Start menu: picking the command is the confirmation.
pub fn run(cmd: &'static Command, elevated: bool) {
    if let Action::Uri(uri) = cmd.action {
        crate::apps::open(uri.to_string(), elevated);
        return;
    }
    if let Action::EditCustomCommands = cmd.action {
        crate::custom::edit(elevated);
        return;
    }
    std::thread::spawn(move || unsafe {
        match cmd.action {
            Action::Uri(_) => {}
            Action::Lock => {
                LockWorkStation();
            }
            Action::Sleep => {
                SetSuspendState(false, false, false);
            }
            Action::SignOut => shutdown(&["/l"]),
            Action::Restart => shutdown(&["/r", "/t", "0"]),
            // /hybrid: same as the Start menu's "Shut down" (keeps fast startup).
            Action::Shutdown => shutdown(&["/s", "/hybrid", "/t", "0"]),
            Action::ApplyUpdate => crate::update::apply(),
            Action::Uninstall => crate::update::uninstall(),
            Action::EditCustomCommands => {}
            // Run by the UI itself (update::check_now), it needs the window handle.
            Action::CheckWinLaunchUpdates => {}
            Action::EmptyRecycleBin => {
                // Flags 0: Windows shows its own confirmation and progress.
                SHEmptyRecycleBinW(null_mut(), null(), 0);
            }
        }
    });
}

fn shutdown(args: &[&str]) {
    let _ = Process::new(crate::system_exe("shutdown.exe"))
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::t_en;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique() {
        let ids: HashSet<&str> = COMMANDS.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), COMMANDS.len());
    }

    #[test]
    fn every_command_has_a_name() {
        for cmd in COMMANDS {
            let id = cmd.message_id();
            assert_ne!(t_en(&id), id, "missing message {id} in en.ftl");
        }
    }

    #[test]
    fn settings_use_ms_settings_uris() {
        for cmd in COMMANDS {
            match (cmd.kind, cmd.action) {
                (Kind::Setting, Action::Uri(uri)) => assert!(uri.starts_with("ms-settings:")),
                (Kind::System, Action::Uri(_)) | (Kind::Setting, _) => {
                    panic!("{}: kind and action do not match", cmd.id)
                }
                _ => {}
            }
        }
    }

    #[test]
    fn update_command_name_contains_version() {
        let id = UPDATE_COMMAND.message_id();
        let name = crate::i18n::t_args(&id, &[("version", "9.8.7")]);
        assert!(name.contains("9.8.7"), "{name}");
    }

    #[test]
    fn uninstall_command_has_a_name_and_own_id() {
        let id = UNINSTALL_COMMAND.message_id();
        assert_ne!(t_en(&id), id, "missing message {id} in en.ftl");
        assert!(COMMANDS.iter().all(|c| c.id != UNINSTALL_COMMAND.id));
    }

    #[test]
    fn target_is_stable_history_key() {
        assert_eq!(COMMANDS[0].target(), "cmd:check-updates");
    }
}
