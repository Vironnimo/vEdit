# Morrow 0.4.12: Mehrsprachigkeit und Tastaturbedienung

Ausgangspunkt war der Umstieg auf Morrow als täglichen Editor, besonders für Python und zusätzlich Dart. Die vorherige Prüfung hatte konfigurierbare Tastenkürzel und mehrsprachige Arbeitsabläufe zu gering gewichtet. Zwei neue adversarische Audits untersuchten diese Abläufe; getrennte Implementierungen und drei unabhängige Nachreviews prüften anschließend Sprache/Projektbefehle, Tastenkürzel und räumliche Navigation einschließlich Git.

## Direkt einrichten

- Gewünschte Anordnung: **View → Explorer left, Git right**. Rechts mit **Ctrl+Alt+B**, links mit **Ctrl+Alt+E** unabhängig ein-/ausblenden.
- Zwischen Bereichen: **Ctrl+Alt+Pfeile** oder **F6 / Shift+F6**. Zwischen Tabs: **Ctrl+Tab / Ctrl+Shift+Tab**. Tab in benachbarten Split: **Ctrl+Alt+Shift+Pfeile**.
- Eigene Belegung: **Settings → Keyboard**. Schrift, Zeilenhöhe und Zoom: **Appearance**. Einrückung: **Editing**.
- Python-Interpreter und Projektbefehle: **Settings → Tasks**. Dart-SDK und weitere Analyseserver: **Languages**.

## Umsetzung

- Ein zentrales, persistentes Befehlsverzeichnis für eigene Tastenkürzel. Settings → Keyboard bietet Suche, Aufnahme, Konfliktprüfung, Deaktivieren und Zurücksetzen. Menüs, Palette und Hinweise folgen der wirksamen Belegung. Die Aufnahme berücksichtigt physische Modifier, Windows-Zwischenablage-Ereignisse, deutsche Tastaturvarianten und reservierte Eingaben. Quellenbezogene Aktionen beachten Vim und Textfelder; mehrere Eingaben desselben Frames bleiben geordnet.
- Ctrl+Alt+Pfeile navigieren anhand der sichtbaren Anordnung durch linke Seitenleiste, Editor-Splits, rechte Seitenleiste und Output/Problems. F6 läuft durch sichtbare Bereiche. Ctrl+Alt+Shift+Pfeile verschieben Tabs mitsamt ihrer Ansicht. Expliziter Git-/Explorer-Fokus funktioniert auch bei vorübergehend eingeklappter Leiste. Alle Befehle sind neu belegbar.
- Git unterstützt Pfeiltasten, Home/End, Enter, Tab und Shift+F10; sichtbarer Fokus, Scrollen zum Ziel und Rückkehr aus verschachtelten Diffs erhalten den Arbeitskontext. Die Commit-Nachricht bleibt ein normales Textfeld.
- Python, Dart und Rust besitzen getrennte Analysesitzungen in demselben Projekt. Zusätzliche Server sind über Programm, Argumente, Dateiendungen und JSON-Optionen konfigurierbar. Zustände und Fehler sind sichtbar, Neustarts betreffen die gewählte Sprache. Diagnosen, Completion und Definitionen werden dem Ursprungsdokument zugeordnet; veraltete Antworten dürfen neue Eingaben nicht übergehen.
- Die portable Ausgabe enthält basedpyright 1.40.1 samt Node-Runtime und Lizenzen. Python-Ausführung und Analyse verwenden denselben einstellbaren Interpreter; Projektumgebungen werden erkannt. Dart verwendet das installierte Dart-/Flutter-SDK, Rust rust-analyzer. Vorhandene Server und eigene Konfigurationen bleiben verwendbar.
- Run/Check/Test/Build wählen passende Python-, Dart- oder Rust-Befehle. Projektprofile und eigene Befehle sind gespeichert. Native Argumente und Umgebungsvariablen erhalten Dateipfade unverändert. Python Check prüft `.py`, `.pyw` und `.pyi` unabhängig von Großschreibung und Bytecode-Cache; ausgewählte Windows-Interpreter bringen ihr Scripts-Verzeichnis für Terminalbefehle mit. Ungespeicherte Dateien werden vor einem Lauf angezeigt; Speichern bleibt explizit. Output und Terminal erhalten die gewählte Python-Umgebung.
- Gemeinsame Dateityperkennung einschließlich Python-Stubs, Python-Windowsdateien und Großschreibung; eigene Dart-Grammatik aus dem offiziellen Dart-Projekt. Einrückung kann erkannt oder je Sprache konfiguriert werden. Native Eingabe, Vim und CLI verwenden dieselbe Einstellung, einschließlich sichtbarer Tab-Breite und CRLF-Erhalt.
- Einstellungen besitzen kurze, direkt anwählbare Kategorien statt einer langen Liste sämtlicher Optionen.

## Nachreviews und Korrekturen

Die Reviews prüften konkrete Übergänge: Fokuswechsel bei wenig Platz, herausgescrollte Git-Zeilen, Escape aus Kontextmenüs, geordnete Text-/Shortcut-Ereignisse, Modifier-Tasten bei der Aufnahme, deaktivierte Sprachvorgaben mit eigener Alternative, manuell gewählte Serverprogramme, Interpreterwechsel und verspätete Definitionsergebnisse. Die Rückmeldungen führten zu gezielten Regressionstests und Korrekturen. Die Sprachserver-Ereigniswarteschlange verwendet abbrechbaren Rückstau, damit ein blockierter GUI-Thread keine Zustände oder Diagnosen stillschweigend verliert.

## Verifikation

Der finale vollständige Lauf besteht: **1.036 Editor-Tests und 48 CLI-Tests, 0 Fehler**. Die regulären Läufe lassen elf beziehungsweise eine opt-in Prüfung aus; die sechs sprach- und laufzeitbezogenen Integrationstests wurden zusätzlich ausdrücklich ausgeführt. Manuelle Leistungsbenchmarks und der separate Launcher-Test gehören nicht zu diesem Lauf. Alle sechs expliziten Live-Tests bestehen: Python-, Dart- und Rust-Diagnosen/Completion/Definitionen; Python-venv mit ausschließlich dort installiertem Testpaket; Unicode-Ausgabe über native und eigene Python-Befehle; Python-Syntaxprüfung aller unterstützten Endungen einschließlich Cache- und Ausschlussfällen. Die beiden Python-LSP-Tests wurden zusätzlich mit dem Testprogramm im portablen Paketordner und den dort gebündelten Server-/Node-Dateien erfolgreich wiederholt. `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check` und der Release-Build bestehen. Die ausgelieferte EXE stimmt per SHA-256 mit dem Release-Build überein; die paketierte CLI liefert ein gültiges Schema. UI-Prüfungen verwenden den echten egui-Ereignispfad und gerenderte Ansichten, einschließlich 200 % Zoom. Die opt-in Integrationstests verwenden echte Sprachserver und eine isolierte Python-Umgebung; sie verändern keine Benutzerprojekte. Das ist keine Behauptung eines mehrstündigen manuellen Alltagseinsatzes.

## Agent-Schnittstelle

CLI-Schema und vollständiger CLI-Hilfetext wurden gemeinsam als kompaktes UTF-8-JSON mit demselben Tokenizer (`tiktoken`, `o200k_base`) gemessen: 7.198 Tokens im Arbeitsstand 0.4.11 vor dieser Aufgabe, 7.602 Tokens danach, also +404. Provider-Wrapper sind nicht enthalten. Das ist eine lokale Schätzung, keine Abrechnung; die Definitionen fallen in jeder Modellanfrage an, die sie enthält, abhängig von Framing und Caching. Hinzugekommen sind belegbare Befehle, Einrückungsregeln und sprachabhängige Projektaktionen. Eine Vergleichsmessung vollständiger Agent-Läufe und neue Blackbox-Erstbenutzer-Modellversuche wurden nicht durchgeführt. Die CLI-Verifikation prüft deterministisch Dispatch, Validierung und tatsächliche Zustandsänderungen.

## Quellen und reproduzierbare Komponenten

- [basedpyright: Installation von CLI und Sprachserver](https://docs.basedpyright.com/latest/installation/command-line-and-language-server/); Version und Integrität stehen in `tools/basedpyright/package-lock.json`.
- [basedpyright: Servereinstellungen](https://docs.basedpyright.com/latest/configuration/language-server-settings/).
- [Dart-Syntaxgrammatik](https://github.com/dart-lang/dart-syntax-highlight); gebündelte Quelle, Revision, Konverter und Lizenz liegen unter `assets` beziehungsweise `scripts`.
- [Language Server Protocol](https://microsoft.github.io/language-server-protocol/); Morrow verbindet Sprachserver über Standard-Eingabe/-Ausgabe.

Projektbefehle laufen im geöffneten Projektordner. Eigenständige verschachtelte Projekte können als weiterer Projektordner geöffnet werden; alternativ lässt sich ein passender eigener Befehl konfigurieren. Eigene Sprachserver, SDKs und Projektwerkzeuge verwenden die lokal gewählte Installation.
