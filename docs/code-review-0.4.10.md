# Morrow 0.4.10: drei unabhängige Code-Reviews

Wie angefordert haben genau drei Subagents mit **Astra und xhigh Thinking** geprüft: Editor und Vim; Projekte, Dateizustand und Recovery; Markdown, Suche und Agent-CLI. Ausgangspunkt war der Code auf `0890008` (0.4.9). Sie haben keine native Anwendung bedient, keine Screenshots betrachtet und keine Verbindung zur laufenden Morrow-Instanz verwendet. Die automatisierten Tests verwenden isolierte Daten und, wo notwendig, den egui-Testharness ohne natives Windows-Fenster.

## Bestätigte Befunde und Änderungen

Die erste Runde ergab 15 konkrete Fehler. Die Nachprüfung der Korrekturen fand weitere Rennen und einen Fehler beim asynchronen Markdown-Restore.

| Bereich | Fehler und resultierendes Verhalten |
| --- | --- |
| Recovery-Basis | Wiederherstellung übernahm die aktuelle Disk-Version als Speicherbasis eines älteren Entwurfs. Text, ursprünglicher Fingerprint, Zeilenumbrüche und BOM werden jetzt gemeinsam atomar gesichert. Externe Änderungen bleiben geschützt; der wiederhergestellte Entwurf kann über **Save As** separat gespeichert werden. |
| Ältere Recovery-Dateien | Das bisherige reine Textformat bleibt lesbar. Ohne beweisbare ursprüngliche Basis autorisiert ein wiederhergestellter Entwurf kein Überschreiben einer bestehenden Datei. |
| Fehlendes Projekt | Ein vorübergehend fehlender Projektordner entfernte auch dessen wiederherstellbare Entwürfe aus der Sitzung. Projekte mit Recovery behalten jetzt Tabs und Layout über weitere Neustarts. |
| Laden und Undo | Überlappende Reloads und Laden während des Speicherns werden verhindert; verspätete Ladeergebnisse überschreiben keine neuen Änderungen. Ein tatsächlicher Austausch durch eine Disk-Version setzt auch die native Undo-Historie zurück. |
| Undo bis zur gespeicherten Version | Ein alter Recovery-Entwurf konnte nach einem Absturz wiederkehren, obwohl er bereits rückgängig gemacht war. Eine saubere Version entfernt ihn mit einer geordneten Tombstone-Operation. Fehler bei der Entfernung werden erneut versucht. |
| Save und Recovery gleichzeitig | Eine laufende Speicherung kann einen scheinbar sauberen Puffer anschließend wieder dirty machen. Recovery wird währenddessen nicht vorschnell entfernt. Ändert ein abgeschlossenes Save die Basis neuerer Änderungen, wird die Recovery auch ohne weitere Textänderung erneuert. Globale Snapshot-Stamps verhindern, dass ältere Worker einen neueren Zustand ersetzen. |
| Unlesbare Recovery | Beschädigte Records bleiben erhalten. Neue Änderungen benutzen einen getrennten Recovery-Pfad; Meldung und Sitzung halten beide Fälle auseinander. |
| Vim Undo und Wiederholung | Windows-Undo/Redo verwarf nicht alle begonnenen Vim-Kommandos. Offene Operatoren, Counts und Visual-Präfixe werden jetzt sauber beendet, damit `.` keine alten Eingaben wiederholt. |
| Native Eingabereihenfolge | Vorab abgefangene Enter-, Tab- und Undo-Aktionen konnten vorausgehenden Text im selben Eingabebatch überholen. Der native Präfix wird jetzt zuerst von TextEdit verarbeitet, danach folgt das Kommando mit seinem Suffix. |
| Modifizierte Insert-Eingaben | Eine vereinfachte Nachbildung nativer Eingaben verlor etwa Ctrl+Backspace und die Wortbewegung mit Ctrl+Pfeiltasten. Die tatsächlichen TextEdit-Semantiken bleiben jetzt erhalten. |
| Visual-Wiederholung | Nach Mausauswahl konnten Ersetzen, Change und Einrücken bei `.` eine andere Fläche bearbeiten. Die abgeschlossene Auswahlgeometrie wird jetzt gespeichert und als ein Undo-Schritt wiederholt. |
| Counts und Zeilenende | `1dG` und `d1G` behandelten die explizite Eins unterschiedlich. Counts bleiben erhalten; `$` behält bei vertikalen Bewegungen außerdem die Absicht „Zeilenende“ statt nur einer numerischen Spalte. |
| Neue Agent-Entwürfe | Ein per CLI angelegter Entwurf bekam ohne weitere Bearbeitung keine Recovery. Auch benannte leere Entwürfe werden jetzt gesichert, ohne sie automatisch auf Disk zu speichern. |
| Markdown-Scrollpositionen | Cache-Verdrängung entfernte ausstehende Scroll-Restores inaktiver Tabs. Sie bleiben jetzt bis zur Darstellung erhalten und werden beim tatsächlichen Schließen des Dokuments oder der Ansicht bereinigt. Ein Ladeabschluss darf die wiederhergestellte Position nicht durch einen impliziten Cursor-Sprung verdrängen. |
| Unicode-Suche | CLI-Suche/Ersetzen und Disk-Suche erkannten Wortgrenzen anders als die Editorsuche. Kombinationszeichen, Joiner und verbindende Unicode-Zeichen werden jetzt konsistent berücksichtigt. |
| CLI-Argumente | Werte nach `--exclude`, etwa `--help`, konnten irrtümlich als globale Optionen behandelt werden. Der Parser erhält sie jetzt als Ausschlussmuster. |

Die native Eingabefortsetzung ist an Datei, Pane, Revision, Auswahl, Modus, Fokus und relevante Einstellungen gebunden. Externe Navigation oder Bearbeitung verwirft eine veraltete Fortsetzung. Neu eintreffende Tastatureingaben folgen einer weiterhin gültigen Fortsetzung. Ein Kommando ohne vorausgehenden nativen Text wird sofort verarbeitet. Tests warten auf den tatsächlichen Abschluss dieser Fortsetzung, statt feste Wartezeiten anzunehmen.

## Agent-Schnittstelle

Diese Runde korrigiert bestehende Semantik. CLI-Hilfe, JSON-Schema und die im Repository liegende Agent-Anleitung bleiben bytegleich; es wurde kein Skill persönlich installiert.

Der lokale Kostenvergleich verwendet für Schema, Hilfe und Skill einschließlich beider Referenzen dieselbe kompakte JSON-Serialisierung mit sortierten Schlüsseln, LF und UTF-8 sowie `tiktoken o200k_base`: **12.854 → 12.854 Tokens, Differenz 0**. Provider-Wrapper sind nicht enthalten; dies ist keine Abrechnungsmessung. Es gab keinen zusätzlichen Modellversuch zur Erstbenutzung und keinen Vergleich der Kosten von Aufrufen und Ergebnissen. Die Tests belegen die korrigierten Abläufe, keine neu gemessene Agent-Usability.

## Validierung

- **942 Tests bestanden:** 894 für die Anwendung und 48 für die CLI, ohne Fehler. Gegenüber 0.4.9 sind 31 Regressionstests hinzugekommen. Sieben ausdrücklich manuelle Benchmark-/Hilfstests bleiben ausgenommen.
- **35 Render-Baselines geprüft.** Die Startseite hat ausschließlich wegen der Versionsanzeige eine neue Baseline erhalten; sie wurde separat erzeugt und als einziges Testbild übernommen. Die abschließende volle Suite lief gegen die normalen Baselines ohne automatische Aktualisierung. Kein Bild wurde manuell betrachtet.
- **Formatprüfung, Clippy für alle Targets mit `-D warnings` und Git-Diff-Prüfung bestanden.**
- Das portable **Windows-Paket 0.4.10** wurde erfolgreich gebaut. Anwendung, CLI, Agent-Anleitung, Dokumentation und alle 35 Baselines wurden anhand ihrer SHA-256-Hashes mit den gebauten bzw. gespeicherten Quellen verglichen. Die bestehende lokale Startverknüpfung verweist auf das Paket.

Die erste volle Runde dokumentiert die aufgedeckten Integrationsfehler unverändert in `target/verification-0.4.10/tests-initial.log`. Bei zwei Eingabetests wurde der Testharness an den expliziten Abschluss der nativen Eingabefortsetzung angepasst; alle bisherigen Text-, Modus- und Undo-Erwartungen blieben erhalten. Der native Reload-Test modelliert jetzt vor dem Laden einen gespeicherten Puffer. Zusätzliche Tests prüfen unabhängig davon, dass tatsächlich neue Änderungen und verspätete Ladeergebnisse geschützt bleiben. Der rote Markdown-Neustart-Test führte zu der oben beschriebenen Korrektur im Ladeabschluss.

Die Nachweise liegen unter `target/verification-0.4.10`, insbesondere `tests-strict.log`, `clippy.log`, `release-package.log`, `package-proof.json` und `definition-token-cost.json`. Eine statische Nachprüfung durch den Recovery-Reviewer fand nach den Korrekturen keine weiteren konkreten Fehler.

Es wurde keine manuelle Prüfung des nativen Windows-Fensters durchgeführt. Auch das neue Release wird in dieser Runde nicht gestartet.
