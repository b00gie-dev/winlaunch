# Changelog

What changed in each WinLaunch release. The release workflow takes the section of the
version it builds as the release notes (shown in WinLaunch before and after an update),
so every release needs a section here. Keep entries short: one line each, for users.

Start each entry with its kind in bold; WinLaunch shows a matching icon and hides the
prefix: `**New:**`, `**Improved:**`, `**Fix:**` or `**Security:**`.

## 0.8.1

- **Fix:** "Check for updates" now reliably starts the check in Windows Update instead of only opening the page

## 0.8.0

- **New:** WinLaunch is open source (MIT license): github.com/b00gie-dev/winlaunch
- **Improved:** Updates now come from the WinLaunch repository itself
- **Security:** The admin autostart instance loads DLLs only from the Windows folder and ignores test settings from the environment
- **Improved:** Sign out, restart and shut down act right away, without asking first
- **Fix:** Rare crashes with unusual characters on a website (favicons) or very long input in the calculator
- **Fix:** Icons no longer get lost or mixed up when the app list refreshes

## 0.7.2

- **Fix:** Icons of custom commands under "?" no longer disappear after a while

## 0.7.1

- **Fix:** With the Start menu or Windows search open, Win+Space now closes it and your typing goes to WinLaunch

## 0.7.0

- **New:** See what an update brings: "/update" lists its changes under the update command
- **New:** After an update, the launcher shows once what is new
- **Improved:** Each change shows an icon for its kind: new, improved, fix or security

## 0.6.1

- **Fix:** Smoother icon edges: icons now look as crisp as in the taskbar

## 0.6.0

- **New:** Calculator: type a calculation such as 85 + 20%, Enter copies the result
- **New:** Currency converter: 100 usd, $100 or 12,50 € in £ with daily ECB rates
- **New:** File search: start with % to search files and folders in the Windows index
- **Improved:** Custom web and search commands show the site's favicon
- **Improved:** Icons appear right after startup thanks to an icon cache

## 0.5.3

- **Improved:** Custom commands show real icons: programs, folders and your browser

## 0.5.2

- **Fix:** Updates that fail to apply can be retried
- **Security:** Hardened release pipeline

## 0.5.1

- **New:** Short help for "/" and "?" in the footer
- **Fix:** The hotkey keeps working reliably, also after Win+L
- **Security:** Safer autostart setup and launching
- **Fix:** Apps with "Uninstaller" in their name are no longer hidden

## 0.5.0

- **New:** Command mode: start with / to list and search commands only
- **New:** Custom commands: start with ? for your own commands from commands.toml
- **Improved:** Long lists scroll

## 0.4.0

- **New:** Finds shortcuts on the desktop and pinned to the taskbar, with their arguments

## 0.3.0

- **New:** Uninstall WinLaunch from the launcher (/uninstall)
- **Fix:** Uninstalling also removes the autostart reliably

## 0.2.0

- **New:** Installer and automatic updates
- **New:** Command to check for WinLaunch updates; version shown in the footer
