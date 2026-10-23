# Repo-Prüfung vom 23. Oktober 2026

Ausgangspunkt: `af6755f` (vEdit 0.5.6). Acht Subagents prüften getrennte
Bereiche; die zentrale Prüfung kontrollierte Integration und Änderungen und
führte die gemeinsame Validierung aus. Ergebnis: zwölf belegte Fehler korrigiert
und neun Performance-Stellen verbessert, ohne neue Funktionen, Bedienkonzepte
oder Änderungen am CLI-Vertrag einzuführen.

## Fehlerkorrekturen

| Bereich | Korrektur |
| --- | --- |
| Installation | Das Paketmanifest enthält verschachtelte npm-`package.json`-Dateien. Der Installer übernimmt dadurch auch die Metadaten der Sprachwerkzeuge und prüft ihre Hashes. |
| Wiederherstellung | Entspricht ein extern gespeicherter Text dem Puffer, läuft die Recovery-Bereinigung im Hintergrund. Vorübergehend gesperrte Dateien werden erneut entfernt; alte Snapshot-Ergebnisse gelten nicht als aktueller Recovery-Stand. |
| Explorer | Gekoppelte Umbenennungsereignisse entfernen den alten Verzeichniscache. |
| Unicode-Suche | Ein verworfener Teiltreffer innerhalb einer Kleinbuchstaben-Erweiterung überspringt keinen späteren gültigen Treffer mehr. |
| Markdown | Automatische Überschriftenanker kollidieren nicht mehr mit expliziten Zahlensuffixen anderer Überschriften. |
| Bookmarks | Ein finales einzelnes CR bleibt Textinhalt; der Sprung verliert dadurch keine Spalte und der gespeicherte Textanker bleibt korrekt. |
| Git-Gutter | Markierungen zu einem alten HEAD oder einer invalidierten Indexbasis können keinen veralteten Abschnitt mehr in den Puffer zurücksetzen. |
| Git im Explorer | Direkte Statusangaben für Submodule und zusammengefasste ungetrackte Ordner werden angezeigt. |
| Completion | Ein maskiertes `\}` beendet einen Snippet-Platzhalter nicht mehr vorzeitig. |
| Navigation | Ein neuer Sprung verwirft den Vorwärtsverlauf auch dann, wenn seine Ausgangsposition dedupliziert wird. |
| Dateibeobachtung | Verspätete Watcher-Ergebnisse gehören zur ursprünglichen Projektinstanz und verändern kein inzwischen erneut geöffnetes Projekt. |
| Other files | Fensteraktivierung startet keine Ordnerindexierung für den folderlosen Workspace. |

Die Fehler wurden vor ihrer Korrektur mit gezielten fehlschlagenden Regressionen
belegt. Zusätzliche Tests sichern unter anderem Unicode, rohe CRLF, Undo/Redo,
Cursorpositionen, Ergebnisreihenfolge, große Ersetzungen und sperrende Dateileser.
Die Test-Recovery liegt außerdem jetzt unter `.tmp/test-state/` statt `target/`.

## Performance

- Vim-Blockeinrückung kopiert den Text einmal statt pro Zeile UTF-8-Präfixe erneut
  zu durchsuchen und den restlichen Text zu verschieben. Leere Zeilen reservieren
  auch mit extrem großen Zählern keinen Einrückungstext.
- Explorer, Dateiindex und Quick Open berechnen Sortierschlüssel einmal je Eintrag.
- Git-Reviews berechnen ihre Sortierschlüssel ebenfalls einmal je Datei.
- Wiederholte Absätze verwenden schon beim ersten Layout dasselbe Shaping.
- CLI-Ersetzungen teilen den Ersatztext zwischen Treffern und vermeiden eine
  zusätzliche vollständige Zeichenliste. Eine Anfrage mit 10.000 Treffern und
  1 MiB Ersatz erzeugt dadurch nicht mehr rund 10 GiB Zwischenkopien.
- CLI-Suchseiten lesen ihre Treffertexte mit einem gemeinsamen Zeicheniterator.
- LSP-Referenzen kopieren offene Texte einmal pro Datei und ermitteln sämtliche
  Vorschauzeilen mit einem gemeinsamen Durchlauf je Quelle.
- Tastenkürzel verwenden einmalig geparste statische Standardbelegungen;
  Benutzerbelegungen werden weiterhin sofort berücksichtigt.
- Split-Rendering und Fokuswiederherstellung kopieren nur die aktive Ansicht
  statt aller geparkten Tabzustände und Navigationseinträge.

Isolierte lokale Messungen, jeweils mit gleichem Ergebnis vor/nach der Änderung:

| Teiloperation und Eingabe | Vorher | Nachher |
| --- | ---: | ---: |
| Vim: 10.000 Unicode-/CRLF-Zeilen einrücken | 726,8 ms | 5,46 ms |
| Vim: dieselben Zeilen ausrücken | 1.181,2 ms | 5,07 ms |
| Explorer: 30.000 Einträge sortieren | ca. 91 ms | ca. 21 ms |
| Index: 30.000 Einträge sortieren | ca. 165 ms | ca. 84 ms |
| Quick Open: 30.000 Pfade sortieren | ca. 258 ms | ca. 12 ms |
| Review: 20.000 Pfade sortieren | 170,20 ms | 10,88 ms |
| CLI: Textaufbau für 500 Treffer nach 6 MB Unicode | 230,17 ms | 0,623 ms |
| Shortcuts: 1,92 Mio. Standardabfragen | 610,08 ms | 8,33 ms |
| LSP: 2.000 Vorschauen über 100.000 Zeilen, Debug-Build | 4.441 ms | 9,73 ms |
| Erster Layoutdurchlauf: 200 gleiche Absätze | 200 Shaping-Aufrufe | 2 Shaping-Aufrufe |

Das sind Messungen einzelner Codepfade, keine Gesamtanwendungs-Latenzen.
Die Zeitmessungen außer LSP verwenden optimierte Rust-Harnesses. Vim verwendet
den Median aus drei Läufen, Review 20 Durchläufe und CLI fünf Durchläufe.
Die ad-hoc-Harnesses und Prüfprotokolle liegen lokal unter `.tmp/repo-audit/`.
Der LSP-Benchmark ist als gezielt ausführbarer ignorierter Test hinterlegt.
Der Layouttest vergleicht die vollständigen Galleys mit egui, einschließlich
Unicode, Tabs, Leerzeilen und Zeilenumbruch.

## Gemeinsame Validierung

- `cargo fmt -- --check`: erfolgreich.
- `cargo test --all-targets -- --test-threads=8` mit `WGPU_BACKEND=dx12`:
  **1.740 bestanden**, kein Fehler, **27 regulär ignoriert**
  (GUI-Binary 1.673/26, CLI-Binary 66/1, Temp-Integration 1/0).
- `cargo clippy --all-targets -- -D warnings`: erfolgreich.
- `git diff --check`: erfolgreich.
- `scripts/test-install.ps1`: **26 isolierte Installer-Prüfungen bestanden**,
  einschließlich Metadatenintegrität und Transaktions-Rollback.
- `cargo test --bin vedit benchmark_reference_preview_batches -- --ignored --nocapture`:
  gezielt ausgeführter Benchmark erfolgreich.
- Vorhandene Bildreferenzen unverändert; die Snapshots bestehen im Gesamtlauf.

Während paralleler Teilprüfungen blockierte Windows einmal das erneute Linken
einer noch laufenden Test-EXE. Die abschließende Validierung lief zentral und
vollständig erfolgreich, ohne Tests abzuschwächen.

## Umfang und Grenzen

Geprüft wurden eigene Rustmodule für Textbearbeitung, Speichern, Recovery,
Projekt- und Panezustand, Git/Reviews, CLI/Control, Sprachdienste, Tasks,
Suche/Rendering, Navigation sowie Build-/Installationsskripte. Die lokalen
winit-Patchgrenzen wurden angesehen; unveränderter fremder Vendor-Code erhielt
keinen vollständigen erneuten Audit.

Native Windows-Dialoge, Snap und physische DPI-Wechsel sowie echte Sprachserver
und Prozessbäume wurden nicht zusätzlich manuell geprüft. Kein Releasepaket
gebaut, keine Benutzerinstallation aktualisiert, kein Editor neu gestartet und
nichts gepusht. Der Quellstand bleibt bei Version 0.5.6.

## Implementierungscommits

| Commit | Inhalt |
| --- | --- |
| `cf4f94e` | npm-Metadaten im Release-Manifest |
| `d1acf58` | Recovery-Bereinigung nach externem Speichern |
| `963d842` | Umbenennungen und Workspace-Sortierung |
| `6789e7e` | Lineare Vim-Einrückung |
| `e488d13` | Unicode-Suche, Markdown-Anker und Absatzlayout |
| `7e7e665` | Begrenzte CLI-Allokationen und Bookmark-CR |
| `e0bf401` | Aktuelle Git-Gutter-Aktionen und Review-Sortierung |
| `95287a2` | Snippet-Escapes und Referenzvorschauen |
| `068c47a` | Watcher-Zuordnung und Navigation |
