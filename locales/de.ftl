# WinLaunch – Deutsch
# Kommentare zu Ort und Bedeutung der Texte stehen in en.ftl.

## Launcher-Fenster

search-placeholder = Apps durchsuchen …
suggestions-title = Häufig verwendet
whats-new-title = Neu in WinLaunch { $version }
notes-label = Neu in { $version }
commands-title = Befehle
custom-commands-title = Eigene Befehle
custom-commands-error = Fehler in commands.toml: { $details }
result-kind-application = Anwendung
result-kind-shortcut = Verknüpfung
result-kind-custom = Eigener Befehl
result-kind-calculator = Rechner
result-kind-file = Datei
result-kind-folder = Ordner
result-kind-currency = Währung
currency-source = EZB, { $date }
currency-loading = Kurse werden geladen …
currency-unavailable = Keine Kurse (offline?)

result-kind-setting = Einstellungen
result-kind-command = Befehl

## Systembefehle

cmd-check-updates = Nach Updates suchen
cmd-windows-update = Windows Update
cmd-bluetooth = Bluetooth-Einstellungen
cmd-wifi = WLAN-Einstellungen
cmd-display = Anzeigeeinstellungen
cmd-sound = Soundeinstellungen
cmd-installed-apps = Installierte Apps
cmd-lock = Sperren
cmd-sleep = Energie sparen
cmd-sign-out = Abmelden
cmd-restart = Neu starten
cmd-shutdown = Herunterfahren
cmd-empty-recycle-bin = Papierkorb leeren
cmd-apply-update = WinLaunch auf { $version } aktualisieren

cmd-check-winlaunch-updates = Nach WinLaunch-Updates suchen

cmd-uninstall-winlaunch = WinLaunch deinstallieren
cmd-edit-custom-commands = Eigene Befehle bearbeiten

## Fußzeile

footer-version = WinLaunch { $version }
footer-version-dev = WinLaunch { $version } (Entwicklerversion)
footer-help-commands = Befehle
footer-help-custom = Eigene Befehle
footer-help-files = Dateien
update-hint = Update { $version } verfügbar
update-status-checking = Suche nach Updates …
update-status-current = Auf dem neuesten Stand
update-status-failed = Update-Prüfung fehlgeschlagen
update-status-not-installed = Updates nur in der installierten Version

## Rückfrage

confirm-uninstall = WinLaunch deinstallieren? Autostart und Nutzungsstatistik werden ebenfalls entfernt.

## Meldungen der Befehle

install-success = Autostart eingerichtet.

    WinLaunch startet ab jetzt bei jeder Anmeldung und läuft bereits. Win+Leertaste öffnet das Eingabefeld.

install-failed = Autostart konnte nicht eingerichtet werden:

    { $details }

uninstall-success = Autostart entfernt und WinLaunch beendet.

uninstall-failed = Autostart konnte nicht entfernt werden:

    { $details }

error-schtasks-start = schtasks.exe konnte nicht gestartet werden: { $details }

## Windows-Aufgabenplanung

task-description = WinLaunch – Launcher (öffnet mit Win+Leertaste)
