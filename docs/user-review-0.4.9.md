# Morrow 0.4.9: Prüfung aus Nutzersicht

Drei unabhängige Reviewer haben alltägliches Arbeiten im Editor, Orientierung zwischen Projekten und Ansichten sowie Lesen und Zusammenarbeit mit Agenten kritisch geprüft. Ihre erste Runde arbeitete ohne Implementierungswissen anhand der öffentlichen Anleitung, der CLI-Hilfe und vorhandener Screenshots. Das war eine Prüfung aus Nutzersicht, keine manuelle Bedienung der nativen Windows-Anwendung. Anschließend wurden die Befunde im Code und mit echten Pointer-, Tastatur- und Renderereignissen im egui-Testfenster überprüft.

## Umgesetzt

| Befund | Verbesserung | Überprüfung |
| --- | --- | --- |
| Gleichnamige Dateien und Projekte sind nur anhand eines Tooltips unterscheidbar. | Tabs bekommen bei Bedarf den kürzesten unterscheidbaren Ordnerzusatz. Einzelne Dateien im Split behalten kurze Namen. Gleichnamige Projekte zeigen auch im aktiven Projektwechsel ihren Elternordner. | Windows-Pfadfälle, tatsächliche Tab-Auswahl, Split-Ansichten und Projektwechsel. |
| Speicherzustand und die Wirkung von Auto save sind zu versteckt. | Permanenter Dateistatus und direkt bedienbares **Auto save on/off**. Unsaved, Saving, Agent draft und Dateifehler bleiben von kurzlebigen Meldungen unabhängig. | Mindestfenster, Zustandswechsel, gespeicherte Einstellung und Rückkehr des Editorfokus; ein Agent-Entwurf bleibt geschützt. |
| Im Explorer bleibt die alte Tastaturauswahl optisch stärker als die aktive Datei. | Bei Editorfokus wird die aktive Datei hervorgehoben; bei Explorerfokus bleibt die unabhängige Tastaturauswahl sichtbar. | Gemalte Hervorhebung und unveränderte Navigation zwischen Datei, Ordner und Editor. |
| Schmale Markdown-Tabellen lassen weitere Spalten ohne erkennbare Scrollmöglichkeit verschwinden. | Schmale, dauerhaft erkennbare Scrollleisten bei tatsächlich überlaufenden Tabellen und Dokumenten. Passende Inhalte bekommen keine zusätzliche Leiste. | Gemalte Leisten ohne Hover, Größe, Abstand zum folgenden Text, Scrollposition und wiederverwendeter Dokumentcache. |
| **Restore last** benennt weder Umfang noch Herkunft eindeutig. | Sichtbare Anzahl und Unterscheidung zwischen entfernten und von Agenten abgeholten Blöcken. Tooltip, Clear all und ausstehende Abholung beschreiben das tatsächliche Wiederherstellungsziel. | Take → Entfernen → Wiederherstellen → Clear all, neue Notizen, Neustart und Vorrang des jüngsten Vorgangs. |
| Zeichenbereiche mit exklusivem Ende wirken in der Übersicht wie falsche Markierungen. | Enthaltene Zeilen und Unicode-Zeichenanzahl in der Übersicht; exakte Koordinaten mit erklärtem exklusivem Ende im Tooltip. | Einzelzeichen, Unicode, ganze Zeilen und mehrzeilige Bereiche. |
| Die Dringlichkeit von Review-Hinweisen hängt allein von Farben ab. | **Info**, **Warning** und **Critical** stehen zusätzlich im Hinweis. | Sichtbare Texte und zugängliche Beschriftungen. |
| Beim ersten Start fehlen die Grundlagen der standardmäßig aktiven Vim-Bedienung. | **i / Esc**, Visual-Auswahl und der Weg zu den Einstellungen stehen direkt auf der Startseite. | Alle Aktionen und Versionszeile passen in 960 × 640. |

## Zusätzlich gefundene Fehler

Das Kontextmenü versprach **Ctrl+Z / Ctrl+Y**, während diese Kürzel im Vim-Modus nicht zuverlässig dieselbe Historie wie `u` und **Ctrl+R** verwendeten. Beide Bedienarten nutzen nun dieselbe Undo-Historie. Insert bleibt bei den Windows-Kürzeln aktiv; vorausgehender und nachfolgender Text im selben Eingabebatch bleibt chronologisch korrekt. Getrennte Dateien behalten ihre eigene Historie, und Dialoge oder Suchfelder können die Quelltexthistorie nicht versehentlich verändern.

Der Nachreview hat gezählte Insert-Transaktionen als weiteren Randfall identifiziert. Count-Abschluss und Wiederholung mit `.` sind jetzt auch bei Windows- und Menü-Undo konsistent; Redo ohne vorhandenen Schritt meldet eine durch Count-Abschluss entstandene Textänderung korrekt.

Ein zunächst ungeeigneter Test mit einem rohen CRLF-Bearbeitungspuffer führte zu einer Prüfung des tatsächlichen Windows-Paste-Ereignisses. Damit ließ sich derselbe Fehler reproduzieren: CRLF aus der Zwischenablage konnte die LF-Invariante im Quelltext verletzen und Vim-Positionen verschieben. Quelltext-Paste normalisiert jetzt CRLF und CR zu LF, einschließlich nativer Eingabe und Kontextmenü. Die gespeicherte Zeilenumbruch-Einstellung bleibt erhalten. Der rote Nachweis liegt unter `target/verification-0.4.9/clipboard-before.log`.

## Abgewogene weitere Vorschläge

Eine zusätzliche Sprunghistorie für Definitionen und Suchtreffer ist sinnvoll, wurde aber in dieser Runde nicht als kleiner UX-Fix eingebaut. Ihre Bedienung und Lebensdauer müssten zusammen mit geschlossenen Dateien und unabhängigen Projekt-/Split-Ansichten festgelegt werden; **Ctrl+O** besitzt bereits die native Dateiöffnung als dokumentierte Funktion. Ebenso bleibt eine zusätzliche Zurück-Aktion zwischen Commit- und Diff-Dialog eine mögliche Verbesserung. Beide bestehenden Dialoge haben bereits Schließen, Escape und Außenklick. Die aktuelle Runde priorisiert bestätigte Mehrdeutigkeiten und Fehler in vorhandenen Abläufen.

Die Änderungen erfordern keine neuen CLI-Befehle oder Protokollfelder. Die Anleitung für Agenten bleibt im Repository.

## Nachweise

- **911 Tests bestanden**: 863 für die Anwendung und 48 für die CLI, ohne Fehler. Sieben manuelle Benchmark-/Hilfstests bleiben wie zuvor ausgenommen. Die abschließende Runde lief ohne automatische Snapshot-Aktualisierung.
- **35 Bildprüfungen bestanden**, einschließlich schmaler Markdown- und Split-Ansichten. Die neue [Dateiidentitätsansicht](../tests/snapshots/vedit_file_identity.png), die [kompakte Startseite](../tests/snapshots/vedit_welcome.png) und die [schmale Source-/Preview-Ansicht](../tests/snapshots/vedit_narrow_source_preview_split.png) wurden zusätzlich visuell kontrolliert.
- **Formatierung, Clippy mit `-D warnings` und Git-Diff-Prüfung bestanden.**
- Das portable Windows-Paket wurde als **0.4.9** gebaut und mit dem bestehenden Benutzerprofil gestartet. Die CLI bestätigte acht vollständig geladene Dateien in zwei Projekten. Vollständige Puffertexte, Cursorpositionen, Ansichten, Projektlayouts, Scrollpositionen, Einstellungen, Bookmarks und die wiederherstellbaren Agent-Markierungen blieben erhalten. Die Originaldateien wurden anhand ihrer SHA-256-Hashes unverändert nachgewiesen.
- Die Headless-Tests haben das Benutzerprofil bytegenau unverändert gelassen. Die Kontrolle der nativen Anwendung erfolgte über ihre echte CLI-Verbindung; eine zusätzliche manuelle Mausprüfung des Windows-Fensters wurde nicht durchgeführt.

Test-, Paket- und Upgrade-Protokolle liegen unter `target/verification-0.4.9`, insbesondere `tests-strict.log`, `clippy.log`, `release-package.log` und `upgrade-proof.json`.
