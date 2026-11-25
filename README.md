# WinLaunch

A minimal, extremely fast-starting launcher for Windows.

**Win+Space** opens a search field. Matching apps appear as you type, **Enter** launches the selected one.

- Plain Win32 (`windows-sys`, plus `windows` for the Shell COM interfaces), no runtime (the C runtime is linked in), ~3 MB EXE
- A low-level keyboard hook catches Win+Space (instead of switching the keyboard layout) without opening the Start menu. It runs on its own thread (Windows silently removes hooks that answer too slowly), is reinstalled every 5 minutes and asks for the real state of the Win key (so it does not get "stuck" after Win+L).
- Single instance through a named mutex
- User interface in English and German

## App search

- **Source:** the Start menu's "All apps" list (shell folder `AppsFolder`), so desktop applications **and** Store apps such as Calculator or Settings. Uninstallers (recognized by the program they start, such as `unins000.exe`, or by the whole word "Uninstall" in the name, so "Geek Uninstaller" stays), documents and web links are filtered out.
- **Shortcuts:** also the shortcuts pinned to the taskbar and those on the desktop (your own and the public one), types `.lnk`, `.url` (e.g. Steam games) and `.appref-ms`. They are started as files, so the shortcut's arguments and working folder apply, and they are labeled "Shortcut" on the right. A shortcut named like a Start menu app is only added if it passes arguments; that way most installer shortcuts do not show up twice.
- **Loading:** in the background, after the hotkey is already active (~250 ms for names and icons from the cache, several seconds for the icons without it). Opening the window rescans the list if it is older than 60 s.
- **Icon cache:** `%LOCALAPPDATA%\WinLaunch\icons.bin` (~350 KB) holds the finished icon of each app for the current icon size. The first scan after startup takes them from there as long as they are still valid: applications and shortcuts by the file's modification time and size (an update brings the new icon), Store apps are fetched again after 7 days. New apps are added, uninstalled ones dropped; the file is only written on changes, atomically. A different scaling or file version discards and rebuilds the cache. Deleting it is always safe.
- **Ranking by text:** exact name > name prefix > word prefix > initials (`vsc` → Visual Studio Code) > substring > fuzzy (letters in order). Ties go to the shorter name. Several words (`stu code`) must all match.
- **Ranking by usage:** on top comes a bonus from your launch history:
  - *Frequency × recency:* apps launched often and recently move up, by at most about one text tier.
  - *Learned searches:* if you launched OneNote after typing "no", OneNote is on top the next time you type "no" (and already at "n").
  - The bonus only reorders; it never adds apps that do not match the text.
  - Stored in `%LOCALAPPDATA%\WinLaunch\usage.tsv` (a text file; deleting it resets the history). For tests, `WINLAUNCH_DATA_DIR` redirects the folder (not in the elevated autostart instance, see [Security](#security)).
- **Empty field:** shows the placeholder "Search apps …" and below it, under "Frequently used", up to 6 most-used apps (frequency × recency). Without a history the window stays collapsed. The placeholder is drawn by WinLaunch itself: the built-in one (`EM_SETCUEBANNER`) would need comctl32 v6 and slow down the start.
- **Controls:** ↑/↓ or the mouse select, Enter or a click launches, Esc closes.
- **Rights:** when WinLaunch runs with admin rights (autostart task), it launches apps through Explorer, so they do **not** inherit those rights.

## System commands

Besides apps, the search also finds commands. They show an icon from the Windows symbol font and the label "Settings" or "Command".

| Command | Effect |
|---|---|
| Check for updates | opens Windows Update and starts checking right away |
| Windows Update, Bluetooth, Wi-Fi, Display and Sound settings, Installed apps | opens that page of Settings |
| Lock, Sleep | right away |
| Sign out, Restart, Shut down | right away, like in the Start menu |
| Empty Recycle Bin | with Windows' own confirmation |

- The names are translated; the English name also works as a search term in other languages (`shutdown` finds "Herunterfahren").
- **Command mode:** input starting with `/` searches only commands and settings, no apps (`/upd` → the update commands). `/` alone lists all commands under the heading "Commands", used ones first. Long lists scroll (arrow keys or mouse wheel, with a scroll bar on the right).
- Commands count in the history like apps: frequently used ones move up and appear when the field is empty.
- New command: an entry in `COMMANDS` in `src/commands.rs` and the text `cmd-<id>` in the `.ftl` files.

## Custom commands

With `?` in the search field: `?` alone lists all custom commands, `?text` searches them. They do not appear in the normal search.

- **File:** `%APPDATA%\WinLaunch\commands.toml`. The command "Edit custom commands" (under `/` and `?`) creates it with examples the first time and opens it. Changes apply the next time the launcher opens; the file survives updates and reinstalls.
- **Actions:** `open` opens a URL, a folder or a file; `run` starts a program, optionally with `args` and a working folder `folder`. Like apps, they are started without admin rights.
- **Searches with input:** `{query}` is replaced by the text after the shortcut (`keyword`) or the name. `?g rust gdi` with `keyword = "g"` and `open = "https://www.google.com/search?q={query}"` searches for "rust gdi"; the row then reads "Google: rust gdi". In URLs the text is encoded.
- **Icons:** programs show their own icon (from the EXE, also when found through PATH, like `cmd`), folders and files the one from Explorer, web pages and searches the **site's favicon** (otherwise the default browser's icon or a magnifier). `icon = 'C:\path\file.ico'` (or an .exe) sets the icon yourself. Icons load in the background; a placeholder icon shows until then.
- **Favicons:** fetched directly from the site (first `/favicon.ico`, otherwise the home page's `<link rel="icon">`, no third-party service), stored as `.ico` in `%LOCALAPPDATA%\WinLaunch\favicons\` (a PNG is wrapped in an ICO), renewed after 30 days; after a failure, the next try is a day later. They load in the background and the icon changes as soon as one is there.
- Environment variables such as `%USERPROFILE%` are expanded. An error in the file is shown as the heading of the `?` list (with the line number).

```toml
[[command]]
name = "Google"
keyword = "g"
open = "https://www.google.com/search?q={query}"

[[command]]
name = "OrcaSlicer with profile folder"
run = '%ProgramFiles%\OrcaSlicer\orca-slicer.exe'
args = '--datadir "D:\3D printing\Orca"'
```

## Calculator

If the input is a calculation, the result is the first row ("Calculator"); Enter copies it to the clipboard (without thousands separators, so it can be used right away).

- `+ - * / ^`, also `×`, `÷`, `·` and `:` for division, parentheses, comma or point as the decimal separator.
- Percent like on a pocket calculator: `85 + 20%` = 102, `100 - 10%` = 90, `20% of 85` = 17.
- `pi`, `e` and `sqrt`, `abs`, `round`, `floor`, `ceil`, `ln`, `log`, `sin`, `cos`, `tan` (radians).
- A calculation needs a number and an operation: `42`, `e` or `7-Zip` stay normal searches.
- Up to 12 significant digits (`0.1 + 0.2` = 0.3); shown in the format of the UI language (`1,234.5`).

## Currency converter

`100 usd`, `100$`, `12.50 € in £`, `50 chf to eur`, `20 dollar euro`: the result is the first row (e.g. "100.00 USD = 92.35 EUR"), with the source and date of the rates on the right ("ECB, 2026-11-13"); Enter copies the converted amount.

- Currencies: the roughly 30 of the ECB reference rates, as a code (`usd`), symbol (`$ € £ ¥`) or name (`dollar`, `pound`, `franc`, `yen` …). Without a target, amounts go to euros, and euros to US dollars.
- Rates: daily rates of the European Central Bank (`eurofxref-daily.xml`, ~1.5 KB, no account). They are fetched **only when a conversion is typed** and the stored rates are older than a day, in the background through WinHTTP (delay-loaded). Stored in `%LOCALAPPDATA%\WinLaunch\rates.xml`, so they also work offline; after a failure, the next try is an hour later at the earliest.
- The very first time, "Loading exchange rates …" shows briefly.

## Unit converter

`100 ps`, `100 ps in kw`, `5 mi`, `-5 °c to f`, `3,5 gb mb`, `20 m2 in sq ft`: the result is the first row (e.g. "100 PS = 73.549875 kW", label "Units"); Enter copies the converted value. Works offline, no data is fetched.

- Without a target, each unit goes to its usual counterpart: PS ↔ kW, mi ↔ km, °F ↔ °C, lb ↔ kg, in → cm, mph ↔ km/h, bar ↔ psi, GB → MB …
- Length, area, volume, mass, temperature, speed, power, energy, pressure, data sizes and time, as symbols (`km`, `m²`/`m2`/`qm`, `km/h`) or English and German names (`miles`, `Meilen`, `Stunden`).
- Data sizes like Explorer: 1 KB = 1024 bytes (`KiB`, `MiB` … mean the same); bits with decimal prefixes like network speeds (1 Mbit = 1,000,000 bits).
- Short names that would take over normal searches are left out: `3d` stays a search for "3D Viewer", `4k` for "4K" (use `days`, `kelvin` …).
- Up to 8 significant digits, in the format of the UI language.

## File search

With `%` in the search field: `%invoice 2024` searches files and folders whose name contains words starting like these (every word must match), newest first. Next to the name the folder is shown in gray, on the right "File" or "Folder".

- **Enter** opens with the default app, **Ctrl+Enter** shows the file in Explorer. Started without admin rights.
- The source is the **Windows search index** (the same as Explorer's search box): it finds what Windows indexes, by default the user folders and the Start menu. More locations can be added in Windows Settings under "Searching Windows".
- Queried through OLE DB (`Search.CollatorDSO`, Windows Search SQL) on its own thread with an open connection; it only ever works on the latest input. Typically 5–30 ms per query, up to 30 results with icons.
- The input is reduced to letters, digits and `. - _` before it goes into the query.

## Languages

The UI follows the **Windows display language** (`GetUserPreferredUILanguages`), not the regional format. English and German are available; English is the fallback for all other languages and for single missing texts. Windows already provides app names in its own language.

- Translations are [Project Fluent](https://projectfluent.org) files in `locales/` and are embedded in the EXE. `en.ftl` has a comment for translators on every text.
- **New language:** add `locales/<code>.ftl` (e.g. `fr.ftl` or `pt-BR.ftl`) and list it in `LOCALES` in `src/i18n.rs`. A unit test makes sure every language has the same texts and placeholders as `en.ftl`.
- **Testing:** `WINLAUNCH_LANG=en` (or `de`, …) forces a language. `WINLAUNCH_LANG=qps-ploc` shows a pseudo-language with accents and 40 % longer texts, to find hard-coded texts and layouts that are too tight.
- The texts load in the background and cost no startup time.

## Measurements

| | |
|---|---|
| Start until the hotkey is active (median) | ~10.5 ms |
| Win+Space until the window is visible | ~8 ms (prototype) |
| Memory with app index and icons | ~9 MB private, ~41 MB working set (mostly shell DLLs shared with other processes) |

COM and shell DLLs are delay-loaded only when needed (see `build.rs`), so they do not add to the startup time.

## Installation and updates

WinLaunch is installed with [Velopack](https://velopack.io): `WinLaunch-win-Setup.exe` from this repository's [releases](https://github.com/b00gie-dev/winlaunch/releases/latest).

- Installs per user to `%LocalAppData%\WinLaunch`, without admin rights; with entries in the Start menu and under "Installed apps".
- On the **first start**, WinLaunch sets up the autostart (one UAC prompt). The task points to `%LocalAppData%\WinLaunch\current\winlaunch.exe`, a path that stays the same across updates.
- **Updates:** WinLaunch checks at startup and every 6 hours in the background and quietly downloads new versions. The footer then shows "Update x.y.z available" and the search offers "Update WinLaunch to x.y.z"; only that command applies the update (restart in a fraction of a second). Before and after an update, WinLaunch shows what changed (from `CHANGELOG.md`).
- **Check now:** the command "Check for WinLaunch updates" (also via `/update`). The window stays open, the result shows in the footer.
- **Footer:** on the left the installed version, in the middle the short help "/ Commands  ? Custom commands  % Files", on the right the update status.
- **Uninstall:** the command "Uninstall WinLaunch" (also via `/uninstall`, with a confirmation) or "Installed apps". Both start Velopack's uninstaller. Its hook sends the running instance the message `WinLaunch.Uninstall`; the autostart instance has admin rights, deletes the task itself and exits. If no instance is running, the hook tries itself (without UAC). Velopack then removes `%LocalAppData%\WinLaunch` including the usage statistics.
- Starting WinLaunch again (e.g. from the Start menu) opens the window of the running instance.
- `WINLAUNCH_UPDATE_SOURCE` (URL or local folder) redirects the update check for testing; like `WINLAUNCH_DATA_DIR` and `WINLAUNCH_LOG` (a diagnostics log file), it is ignored while WinLaunch runs elevated.

### Creating a release

1. Add a section for the new version to `CHANGELOG.md`, raise `version` in `Cargo.toml`, build (updates `Cargo.lock`), merge to `main` through a PR.
2. Create and push the tag: `git tag v1.2.3` and `git push origin v1.2.3`.
3. The workflow `release.yml` builds, tests, packs with `vpk` (including a delta update from the previous version) and publishes the packages as a GitHub release of this repository – the update feed of every installation.

Whoever can publish a release here can run code with admin rights on every installation (the autostart instance runs elevated and applies updates). Hence:

- only a tag starts the workflow, and a ruleset restricts who may create `v*` tags;
- building (Cargo, the dependencies' build scripts, `vpk`) runs with read-only rights; only a separate job gets write access, and it runs nothing from the repository but only uploads the finished packages;
- all actions are pinned to a commit (Dependabot proposes updates monthly) and the release is built without a build cache.

## Building

Needs Rust (MSVC toolchain) and the Visual Studio Build Tools (C++ workload).

```
cargo build --release
```

The EXE is then in `target\release\winlaunch.exe`.

While WinLaunch is running, the EXE cannot be overwritten. Run `winlaunch --quit` before rebuilding.

## Command line

| Command | Effect |
|---|---|
| `winlaunch` | Starts the launcher (runs invisibly, Win+Space opens the field) |
| `winlaunch --quit` | Stops the running instance and waits until it is gone (e.g. before updates) |
| `winlaunch --install` | Sets up the autostart and starts WinLaunch (one UAC prompt) |
| `winlaunch --uninstall` | Stops WinLaunch and removes the autostart |
| `winlaunch --list-apps \| more` | Diagnostics: lists all apps found, with target and load time (output only visible when redirected) |

### Autostart

`--install` creates the task **WinLaunch** in Task Scheduler:

- Trigger "At log on" of the current user. This avoids the delay with which Windows starts programs from the normal autostart.
- "Run with highest privileges", so Win+Space also works while an admin window is in the foreground.
- Normal process priority and no time limit (Task Scheduler's defaults would be "below normal" and a stop after 72 hours).

The task starts the EXE from the path `--install` was called from, so the EXE should be in its final place first.

It is created through Task Scheduler's COM interface, with the definition passed in memory. An XML file in `%TEMP%` (as with `schtasks /XML`) could be swapped by a program without admin rights between writing and reading it.

When WinLaunch runs with admin rights, it always launches apps, shortcuts and custom commands through Explorer, so without admin rights. If Explorer cannot be reached, it launches nothing rather than passing its rights on.

## Security

The autostart instance runs with admin rights, so that Win+Space also works while an admin window is in the foreground. Everything it starts runs without them (through Explorer, see above). Hardening:

- It loads DLLs only from System32 (`SetDefaultDllDirectories`); DLLs that are not Windows "known DLLs" are delay-loaded so this applies to them too, and the C runtime is linked in. System programs (`shutdown.exe`, `schtasks.exe`, `explorer.exe`) are started by their full path.
- It ignores the test overrides `WINLAUNCH_DATA_DIR`, `WINLAUNCH_UPDATE_SOURCE` and `WINLAUNCH_LOG`: user environment variables can be set by any program.
- The autostart task is created through COM, without a temporary XML file.
- Updates come only from this repository's releases (see [Creating a release](#creating-a-release)).

**Known limitation:** Velopack installs per user, so `%LOCALAPPDATA%\WinLaunch\current\winlaunch.exe` can be replaced by any program running as you, without admin rights; the autostart task would then start the replacement elevated at the next sign-in. A program that can do this already runs as you, but it could gain admin rights this way without a UAC prompt. Fixing it would need the EXE in a protected folder (and a signed update path), which a per-user install cannot offer.

## Benchmark parameters

- `--ready <EventName>`: signals a named event as soon as the hook and the window are ready
- `--shown <EventName>`: signals a named event after the window has been shown and painted

## License

[MIT](LICENSE)
