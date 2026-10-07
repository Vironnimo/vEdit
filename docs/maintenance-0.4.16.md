# Morrow 0.4.16: Fehlerkorrekturen und geschlossene Tabs

Zwei Subagents und der Hauptagent haben die wichtigsten Nutzerabläufe geprüft:
Bearbeiten, Suchen, Dateizustand, Recovery, Projekte und Splits sowie die lokale
Agenten-Verbindung. Git, Sprachserver, Aufgaben und Markdown wurden anhand ihrer
Integrationsstellen und vorhandenen Tests einbezogen. Ausgangspunkt war
`61f194b` (0.4.15). Das war eine breite, risikoorientierte Prüfung, keine Zusicherung,
dass jede Zeile des Repositorys oder jede native Windows-Interaktion geprüft ist.

## Behobene Fehler

- Bei Raw-CRLF-Text, etwa aus einer Agenten-Bearbeitung, behandelte Vim das CR
  teilweise als sichtbares Zeilenzeichen. Bewegungen ans Zeilenende, Anhängen,
  Löschen, zeilenweises Kopieren/Einfügen, Visual-Ersetzen und Absatznavigation
  erhalten nun die vollständigen Zeilenumbrüche. Undo stellt die Originalfassung
  wieder her. Eine UI-Regression prüft den tatsächlichen CLI-Edit-Eingang und
  anschließende Vim-Bedienung.
- Reguläre Ausdrücke behandeln CRLF als Zeilenende: `$` liegt vor dem ganzen
  Umbruch, `.` trifft dessen CR nicht. Ganze-Wörter-Suche mit Satzzeichen wie
  `foo.` oder `*` behält ihre Grenzen auch beim Wiederverwenden mit `:s//…/`.
- **Save As** nutzte für gemischte Zeilenenden bisher das vorhandene Ziel als
  Formatvorlage. Eine neue Kopie konnte das Format verlieren; ein binäres Ziel
  konnte den bestätigten Speichervorgang verhindern. Jetzt liefert ausschließlich
  die verifizierte Originaldatei die Separatoren. Der Ziel-Fingerprint bleibt
  unabhängig davon der Schutz vor Überschreibkonflikten. Der Tab übernimmt das
  tatsächlich geschriebene Format.
- Unter Windows erbte ein akzeptierter CLI-Socket den nichtblockierenden Modus
  des Listeners. Eine Anfrage über mehrere TCP-Pakete konnte dadurch vorzeitig als
  ungültig abgewiesen werden. Empfangene Verbindungen warten jetzt innerhalb des
  bestehenden Zeitlimits auf den vollständigen Frame. Größenlimits und die
  Unterscheidung zwischen Dispatch und Anwendung bleiben erhalten.

Wenn die Originaldatei mit gemischten Zeilenenden inzwischen verändert oder
entfernt wurde, kann Morrow deren Separatoren nicht exakt rekonstruieren. Save As
speichert dann eine LF-Kopie mit sichtbarem Statushinweis. Text und BOM bleiben
erhalten, die externe Originalfassung wird nicht überschrieben. Ein vollständig
persistiertes Separator-Modell war nicht Teil dieser Änderung.

## Umgesetzte Erweiterung

Versehentlich geschlossene Dateien sollten ohne erneute Dateisuche weiterlesbar
sein. **Ctrl+Shift+T**, **File → Reopen closed tab** und die gleichnamige
Palettenaktion stellen daher die zuletzt geschlossene Datei des aktuellen Projekts
im aktuellen Editorbereich wieder her, einschließlich Cursorposition und
Markdown-Ansicht. Die Belegung lässt sich in Settings ändern. Bereits gespeicherte
eigene Belegungen von Ctrl+Shift+T behalten beim Upgrade Vorrang.

Pro Projekt bleiben maximal 20 verschiedene Dateien in Schließreihenfolge erhalten.
Die Historie endet beim Schließen des Projekts oder der Anwendung. Ein noch in
einem anderen Split offener Puffer wird wiederverwendet; sonst wird der aktuelle
Disk-Inhalt geladen. Nie gespeicherte Entwürfe und bewusst verworfene Änderungen
werden nicht wiederhergestellt. Fehlende Dateien zeigen den üblichen Ladefehler;
ältere History-Einträge bleiben erreichbar.

Diese Erweiterung wurde gegenüber größeren Vorhaben priorisiert, weil sie einen
häufigen Arbeitsabbruch unmittelbar behebt und die bestehende Projekt-/Split-
Bedienung ergänzt. Weitere sinnvolle Kandidaten sind ein gemeinsamer Regex-Modus
für Find/Replace mit Vorschau, dauerhaft zuletzt verwendete Dateien sowie
Symbolsuche. Diese Kandidaten sind noch nicht umgesetzt; sie brauchen eigene
Entscheidungen zu Bedienung, Ersetzungssemantik und Sprachserverunterstützung.

## Dokumentation und Validierung

Die neue `AGENTS.md` übernimmt die vorgegebenen Arbeitsgrundsätze und ergänzt
Codezuständigkeiten, Datensicherungsregeln, Koordinatensysteme, Eingabe-/Worker-
Invarianten und die notwendigen Prüf- und Paketbefehle. `docs/development.md`
beschreibt isolierte Tests, Bildreferenzen, optionale SDK-Prüfungen und den
portablen Build. README und der aktuelle Agenten-Vertrag sind aktualisiert;
historische Versionsberichte behalten ihre damaligen Ergebnisse.

CLI-Hilfe, JSON-Schema und Morrow-Skill einschließlich beider Referenzen sind
unverändert. Die gemeinsame lokale Messung gegenüber `61f194b` ergibt mit
`tiktoken o200k_base`, kompakter sortierter JSON-Serialisierung und LF
**14.332 → 14.332 Tokens, Differenz 0**. Provider-Wrapper sind nicht enthalten;
das ist keine Abrechnungsmessung. Diese Definitionen fallen in jedem Modellaufruf
an, der sie tatsächlich enthält, abhängig von Caching und Framing. Argument- und
Ergebniskosten sowie neue Modellläufe wurden nicht gemessen.

Die vollständige Suite mit `cargo test --all-targets -- --test-threads=1`
besteht: **1.228 Editor- und 49 CLI-Tests**, insgesamt **1.277 bestanden**,
zwölf optionale Tests ignoriert. Alle **55 Bildreferenzen** stimmen überein.
Die drei absichtlich geänderten Referenzen wurden angesehen: File-Menü,
Aktionszahl in den Tastatureinstellungen und Versionsanzeige auf der Startseite.
Die Abschlussprüfung lief ohne automatische Snapshot-Aktualisierung.
`cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings` und
`git diff --check` bestehen.

Ein früherer paralleler Lauf überschritt die bestehende Zwei-Sekunden-Wartezeit
eines Speicher-/Autosave-Tests; isoliert bestand derselbe Test in 0,02 Sekunden.
Ein weiterer paralleler Lauf endete mit `STATUS_ACCESS_VIOLATION`; das Windows-
Ereignisprotokoll nennt `nvoglv64.dll` als fehlerhaftes Modul. Der erfolgreiche
serielle Lauf behält alle Tests und Assertions bei. Diese Beobachtung belegt
keinen behobenen Treiberfehler oder einen manuell geprüften nativen Editor.
Rohprotokolle und der Windows-Fehlereintrag liegen unter
`target/verification-0.4.16/`; fehlgeschlagene Läufe wurden aufbewahrt.

Auch der vollständige parallele Lauf mit `WGPU_BACKEND=dx12` und acht Testthreads
besteht mit **1.277 erfolgreichen Tests** und identischen Bildreferenzen. Diese
verifizierte Einstellung umgeht die OpenGL-Backend-Erkennung des Testharness;
die Entwicklungsanleitung enthält den Befehl mit Wiederherstellung der vorherigen
Umgebungsvariable. Der Renderer der ausgelieferten Anwendung wurde nicht geändert.

Das portable Windows-Paket **0.4.16** wurde erfolgreich gebaut. Beide Programme,
alle 55 Bildreferenzen, Skill-Dateien und mitgelieferten Dokumente/Lizenzen wurden
mit den Build-Ausgaben beziehungsweise Quellen abgeglichen (78 Dateinachweise).
Zwölf lokale Dokumentationslinks wurden geprüft; die paketierte CLI liefert ihre
Hilfe und das unveränderte Schema erfolgreich. Der Nachweis steht in
`target/verification-0.4.16/package-proof.json`. Start über `Morrow.lnk` oder
`dist/Morrow/morrow.exe`; eine bereits laufende Anwendung verwendet die neue
Version nach ihrem nächsten Start.

Nicht Bestandteil dieser Runde sind manuelle Windows-Dialog-/Snap-/DPI-Tests,
eine vollständige IME-/Graphemprüfung, externe Live-Sprachserver und ein neuer
Modellversuch zur Erstbenutzung der CLI. Die vorhandene Editor-Sitzung bleibt beim
Paketbau erhalten; die neue Anwendung wird erst beim nächsten Start verwendet.
