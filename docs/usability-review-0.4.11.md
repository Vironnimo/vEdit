# Morrow 0.4.11: Alltagstauglichkeit und Komfort

Die Prüfung begann mit drei voneinander getrennten Perspektiven: zwei Devil’s Advocates für Layout und tägliche Arbeitsabläufe sowie einem Komfort-Audit. Nach der Umsetzung prüften drei weitere Subagents unabhängig Layout, Explorer/Einstellungen und Sprachunterstützung/Dateirückmeldungen. Die Prüfungen lesen Code und Tests. Die Darstellung wird zusätzlich mit dem egui-Testharness und gerenderten Screenshots überprüft; dies ersetzt keinen mehrstündigen manuellen Einsatz der nativen Windows-Anwendung.

## Umgesetzt

- Zwei separat schließbare und skalierbare Seitenleisten. Das Preset **Explorer left, Git right** ist über View, Settings und Befehlspalette erreichbar. Panels lassen sich links oder rechts zuordnen; Zuordnung, gewählte Ansicht, Sichtbarkeit und Wunschbreiten bleiben über Neustarts erhalten. Sichere Kürzel funktionieren auch im Vim-Normalmodus. Schmale Fenster beziehungsweise hoher Zoom blenden Platz beanspruchende Seitenleisten vorübergehend mit einem sichtbaren Hinweis aus, ohne das gespeicherte Layout zu überschreiben.
- Gespeicherter Oberflächenzoom (75–200 %), Codeschriftgröße (10–32), Zeilenabstand (1–2×), absolute/relative/ausgeblendete Zeilennummern, optionale Cursorzeile und Autosave-Verzögerung (300–5000 ms). Die gruppierten Einstellungen haben eine dauerhaft sichtbare Scrollleiste. Recovery bleibt bei 850 ms; Agent-Entwürfe warten weiterhin auf explizites Speichern.
- **Show ignored files** für Explorer und Quick Open. Die Umstellung reindiziert alle offenen Projekte im Hintergrund; VCS-Metadaten und Symlinks bleiben ausgenommen. Indexlimits werden sichtbar erklärt. Projektsuche behält ihre eigenen Filter.
- Git erklärt ungespeicherte Dokumente und bietet **Save all files**. Das Speichern löst weder Stage noch Commit aus. Offene Unterordner desselben Repositorys werden beim Hinweis berücksichtigt.
- Fehlerbanner erklären den Erhalt eigener Änderungen und bieten Save as. Reload bleibt bei ungespeichertem Text gesperrt und erklärt den weiteren Weg. Die bisher falschen Verweise auf einen Reload-Eintrag im File-Menü sind entfernt.
- Rust-Analyse zeigt lesbare Zustände und lässt sich über Run, Problems oder die Palette neu starten. Veraltete Diagnosen verschwinden beim Neustart, offene Texte bleiben erhalten. Das Output-Panel erklärt fehlende interaktive Eingabe und die zehnminütige Laufzeitgrenze und bietet ein Terminal an.

## Ergebnisse der Nachreviews

Die Nachreviews fanden zusätzliche Fehler in den neuen beziehungsweise berührten Abläufen: die Panel-Seitenzuordnung hing zunächst nur von der gerade ausgewählten Ansicht ab; F6 konnte versteckte Ansichten wieder öffnen; kleine Fenster überschrieben Wunschbreiten. Diese Fälle werden mit eigener Zuordnung, explizitem Fokuszyklus und temporärer Platzanpassung behandelt.

Der erweiterte Explorer durfte ignorierte Cargo-Manifeste aus Abhängigkeiten nicht zur Rust-Projekterkennung verwenden. Der Index führt diese Erkennung jetzt unabhängig vom Sichtbarkeitsschalter mit den ursprünglichen Ignore-Regeln durch. Ein vorübergehender Rust-Fehler darf nach echter erfolgreicher Erholung ebenfalls nicht als aktueller Fehler sichtbar bleiben; Fehlerantworten dürfen dabei nicht als leere erfolgreiche Antworten gelten.

## Bewusste Grenzen

Das bestehende dunkle Farbschema bleibt erhalten. Ein vollständiges helles Theme, frei belegbare Tastenkürzel und Einstellungen je Dateityp benötigen eigene zusammenhängende Umsetzung. Nicht-Rust-Sprachen besitzen weiterhin Syntaxfarben, aber keinen angebundenen Sprachserver. Markdown-Vorschau und Git lassen sich über den globalen Zoom vergrößern; die neue Zeilenhöhe betrifft Quelltext.

## Prüfung

Der vollständige Testlauf ohne Aktualisierung der Screenshot-Referenzen besteht: **935 Editor-Tests und 48 CLI-Tests**, kein Fehler. Sieben ausdrücklich ignorierte Tests sind vom Standardlauf ausgenommen. Der darin enthaltene Integrationstest mit dem installierten **rust-analyzer 1.97.1** wurde anschließend separat ausgeführt und besteht ebenfalls: echte Diagnosen, Completion und Go to definition. Damit wurden insgesamt **984 Tests erfolgreich ausgeführt**. `cargo fmt --all -- --check` und `cargo clippy --all-targets -- -D warnings` bestehen ebenfalls.

Die gerenderten Ansichten wurden zusätzlich visuell geprüft: Explorer links/Git rechts, schmales Fenster sowie Arbeitsbereich und Einstellungen bei 200 % Zoom. Der optionale Dateisuche-Knopf verdeckt keine Menüs; der Explorer behält auch bei hohem Zoom sichtbare Dateizeilen. Die unabhängigen Nachreviews melden nach den Korrekturen keine weiteren konkreten Befunde. Testartefakte liegen lokal unter `target/usability-*`, die sichtbaren Referenzansichten unter `tests/snapshots`.

Der optimierte Release-Build von **0.4.11** ist erfolgreich erstellt und nach `dist/Morrow` kopiert. Beide ausgelieferten Programme stimmen per SHA-256 mit den Build-Ergebnissen überein; die ausgelieferte CLI liefert Hilfe und Schema erfolgreich. `Morrow.lnk` verweist auf die neue portable Anwendung. Die laufende Benutzersitzung wurde für diese Prüfung nicht gestartet oder verändert.

Die CLI-Schnittstelle wurde mit dem Skill `design-agent-interfaces` auf vollständige Beispiele, additive Änderungen, atomare Einstellungen und unterscheidbare gewünschte/tatsächliche Sichtbarkeit geprüft. Das vollständige Schema plus CLI-Hilfe umfasst vor der Änderung auf `246fc59` **6793**, danach **7198 Tokens** (**+405**), gemessen mit `tiktoken/o200k_base` und derselben kompakten JSON-Serialisierung. Provider-Wrapper sind nicht enthalten; dies ist eine lokale Tokenabschätzung, keine Abrechnung. Diese Definitionen werden nur Teil eines Modellaufrufs, wenn der Agent sie abruft beziehungsweise im Kontext mitführt. Es wurde kein separater blinder Modellversuch zur CLI-Erstbenutzung durchgeführt; dies ist eine verbleibende Verifikationsgrenze, keine behauptete Erfolgsquote.
