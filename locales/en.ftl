# WinLaunch – English (fallback language: must contain every message).
# Format: Project Fluent, https://projectfluent.org
# Comments above each message explain where it appears, for translators.

## Launcher window

# Placeholder in the empty search field at the top of the launcher window.
search-placeholder = Search apps …

# Small grey title above the list of the user's most-used apps.
# Shown only while the search field is empty.
suggestions-title = Frequently used

# Title above the list of changes, shown once after WinLaunch was updated.
# $version (text) – the new version, e.g. 0.7.0
whats-new-title = New in WinLaunch { $version }
# Grey label of each change line (after an update, and under the update command).
# $version (text) – the version the change belongs to
notes-label = New in { $version }

# Small grey title above the list of all commands and settings pages.
# Shown while the search field contains only "/".
commands-title = Commands

# Small grey title above the list of the user's own commands.
# Shown while the search field contains only "?".
custom-commands-title = Custom commands

# Shown instead of that title when the file of custom commands has an error.
# $details (text) – what is wrong, in English, e.g. "TOML parse error at line 3"
custom-commands-error = Error in commands.toml: { $details }

# Grey label at the right edge of each search result row, naming the kind of item.
# This one: an installed application (program or Store app).
result-kind-application = Application

# Label for shortcuts found on the desktop or pinned to the taskbar (they may pass
# extra arguments to the program).
# Keep it short (about 12 characters).
result-kind-shortcut = Shortcut

# Label for the user's own commands (from commands.toml, used with "?").
result-kind-custom = Custom

# Label for the calculator row: the result of the arithmetic typed in the field.
result-kind-calculator = Calculator

# Labels of file search results ("%" prefix).
result-kind-file = File
result-kind-folder = Folder

## Currency converter ("100 usd in eur"): first row of the list.

# Label of the converter row while it has no rates yet.
result-kind-currency = Currency
# Label of the converter row: where the rates come from and for which day.
# $date (text) – date of the rates, e.g. 13.11.2026 or 2026-11-13
currency-source = ECB, { $date }
# Shown instead of a result while the rates are downloaded for the first time.
currency-loading = Loading exchange rates …
# Shown instead of a result when no rates could be downloaded (e.g. offline).
currency-unavailable = No exchange rates (offline?)

# Label for results that open a page of the Windows Settings app.
result-kind-setting = Settings

# Label for results that act on the system or session (lock, restart, ...).
result-kind-command = Command

## System commands: names shown as search results.
## Keep them short, and close to the wording Windows itself uses in this language.

# Opens Windows Update and starts searching for updates right away.
cmd-check-updates = Check for updates
# Opens the Windows Update page of the Settings app. "Windows Update" is a product name.
cmd-windows-update = Windows Update
cmd-bluetooth = Bluetooth settings
cmd-wifi = Wi-Fi settings
cmd-display = Display settings
cmd-sound = Sound settings
# Opens the list of installed apps in the Settings app.
cmd-installed-apps = Installed apps
# Locks the PC (Win+L).
cmd-lock = Lock
# Puts the PC to sleep.
cmd-sleep = Sleep
# Signs the current user out of Windows.
cmd-sign-out = Sign out
cmd-restart = Restart
cmd-shutdown = Shut down
cmd-empty-recycle-bin = Empty Recycle Bin
# Only listed while a new WinLaunch version is downloaded and ready.
# Applies it and restarts WinLaunch (takes a fraction of a second).
# $version (text) – the new version number, e.g. 0.2.0
cmd-apply-update = Update WinLaunch to { $version }

# Checks right away whether a newer WinLaunch version exists; the result appears in the
# footer. "WinLaunch" is the product name; keep it apart from Windows' own update check.
cmd-check-winlaunch-updates = Check for WinLaunch updates

# Only listed in the installed version. Asks for confirmation first (confirm-uninstall).
cmd-uninstall-winlaunch = Uninstall WinLaunch

# Opens the user's file of custom commands (used with "?") in an editor.
cmd-edit-custom-commands = Edit custom commands

## Footer: thin grey bar at the bottom of the launcher window.
## Left: the installed version. Right: update status. Keep both short (about 30 characters).

# $version (text) – installed version, e.g. 0.2.0
footer-version = WinLaunch { $version }
# Shown instead when running a build that was not installed (developers only).
# $version (text) – version the build was made as
footer-version-dev = WinLaunch { $version } (development build)

# Centre of the footer, after the key "/": what typing "/" first searches. Keep it short.
footer-help-commands = Commands
# Centre of the footer, after the key "?": what typing "?" first searches. Keep it short.
footer-help-custom = Custom commands
# Centre of the footer, after the key "%": what typing "%" first searches. Keep it short.
footer-help-files = Files

# Highlighted in blue while a downloaded update waits to be applied.
# $version (text) – the new version number, e.g. 0.2.0
update-hint = Update { $version } available
# While the user's update check runs.
update-status-checking = Checking for updates …
# The user's update check found no newer version.
update-status-current = Up to date
# The user's update check failed, e.g. no internet connection.
update-status-failed = Update check failed
# Development build: it cannot update itself.
update-status-not-installed = Updates only in the installed version

## Confirmation question (message box with Yes / No).

# Before "Uninstall WinLaunch". The usage statistics live in the install folder.
confirm-uninstall = Uninstall WinLaunch? Autostart and usage statistics will be removed as well.

## Message boxes of the command-line commands

# Shown after `winlaunch --install` succeeded.
install-success = Autostart is set up.

    WinLaunch now starts every time you sign in and is already running. Press Win+Space to open it.

# Shown when `winlaunch --install` failed.
# $details (text) – error message from Windows, may be in any language.
install-failed = Autostart could not be set up:

    { $details }

# Shown after `winlaunch --uninstall` succeeded.
uninstall-success = Autostart removed and WinLaunch stopped.

# Shown when `winlaunch --uninstall` failed.
# $details (text) – error message from Windows, may be in any language.
uninstall-failed = Autostart could not be removed:

    { $details }

# Error detail when the Windows tool schtasks.exe itself could not be run.
# $details (text) – system error message.
error-schtasks-start = Could not start schtasks.exe: { $details }

## Windows Task Scheduler

# Description of the scheduled task created by `winlaunch --install`,
# visible in the Windows Task Scheduler.
task-description = WinLaunch launcher (opens with Win+Space)
