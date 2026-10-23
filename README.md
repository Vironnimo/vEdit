# vEdit 0.6.1

Ein nativer Code-Editor für Windows mit mehreren Projekten, Vim-Bedienung, Git und Sprachunterstützung für Rust, Python und Dart. Projektordner, offene Dateien, Cursorpositionen und Einstellungen werden beim nächsten Start wiederhergestellt. Die Oberfläche verwendet englische Beschriftungen.

[Starten](#starten) · [Arbeiten im Editor](#arbeiten-im-editor) · [Tastenkürzel](#tastenkürzel) · [Mit Agenten arbeiten](#mit-agenten-arbeiten) · [Entwicklung](docs/development.md)

Versionsnotizen stehen im [Versionsverlauf](docs/changelog.md); technische
Dokumentation und frühere Prüfberichte im [Dokumentationsindex](docs/README.md).

![vEdit mit geöffnetem Projekt](tests/snapshots/vedit_workspace.png)

## Starten

Für eine feste Benutzerinstallation aus dem Quellcode im Projektordner ausführen:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\install.ps1
```

Der Befehl baut das vollständige Paket und installiert es unter `%LOCALAPPDATA%\Programs\vEdit`. Er legt einen Startmenüeintrag an und nimmt `vedit` sowie `vedit-cli` in den Benutzer-PATH auf. **Derselbe Befehl aktualisiert die Installation** nach späteren Quellcodeänderungen. Ein gewöhnlicher Build allein ändert die installierte Version nicht. Eine laufende Sitzung bleibt geöffnet; die neue Version wird beim nächsten normalen Neustart verwendet. Bereits laufende Terminals und Agent-Anwendungen müssen neu gestartet werden, um den geänderten PATH zu übernehmen. Bis dahin funktioniert der absolute Programmpfad.

Start über das Startmenü oder `vedit`; der Projektordner ist dafür nicht erforderlich. Installation und Aktualisierung aus einem fertig entpackten Paket funktionieren auch ohne Quellcode, Rust oder npm: dessen `scripts\install.ps1` mit PowerShell ausführen. `-PackagePath 'C:\Downloads\vEdit'` übernimmt ein bestimmtes Paket. Die enthaltenen Dateien werden vor der Installation anhand des Paketmanifests geprüft. Git-Funktionen benötigen weiterhin `git` im PATH, Sprachwerkzeuge gegebenenfalls das jeweilige SDK.

Unter **Windows-Einstellungen → Apps → Standard-Apps → vEdit** lassen sich Text- und Codeformate zuordnen. vEdit erscheint auch bei **Öffnen mit**. Bestehende Standardzuordnungen werden bei der Installation erhalten. Mit `-OpenDefaultApps` öffnet das Installationsskript die passende Einstellungsseite. Unter **Installierte Apps → vEdit → Deinstallieren** wird die Anwendung entfernt; persönliche Einstellungen und Wiederherstellungsdaten bleiben erhalten.

Alternativ bleibt das Paket `dist/vEdit/` portabel. **vEdit.lnk** im Projektordner startet dieses Paket; der Startmenüeintrag startet die eigenständige Installation.

Ein externer Dateiaufruf öffnet im zuletzt aktiven Fenster: bereits geöffnete Dokumente werden wiederverwendet, Dateien innerhalb eines offenen Projekts bleiben dort, andere landen in **Other files**. Ordner werden als Projekte geöffnet. **Settings → Layout → Open external files in a new window** ändert externe Dateiaufrufe auf ein neues Fenster; Dateidialoge, Drag-and-drop und `vedit-cli` bleiben im jeweiligen Fenster. Für einen einzelnen Aufruf:

```powershell
vedit --new-window "C:\Notizen\Idee.txt"
vedit --reuse-window "C:\Notizen\Idee.txt"
vedit-cli windows
vedit-cli --window main status
```

Jedes zusätzliche Fenster beginnt ohne die Projekte und Tabs eines anderen Fensters und übernimmt anfangs die gespeicherten Einstellungen des Hauptfensters. Fenster besitzen getrennte Sitzungen; Änderungen derselben Datei in mehreren Fenstern unterliegen dem normalen Schutz vor externen Änderungen. Nach einem Absturz stellt der nächste Hauptfensterstart auch unterbrochene Zusatzfenster wieder her. Bewusst geschlossene Zusatzfenster bleiben als gespeicherte Fenster unter `vedit-cli windows` auffindbar und lassen sich mit `vedit --window ID` wiederherstellen. CLI-Befehle verwenden bei genau einem offenen Fenster dieses; bei mehreren verlangt die CLI `--window ID` **vor** dem Befehl. Tab- und Ansichts-IDs gelten nur innerhalb dieses Fensterprozesses.

Zum Bauen aus dem Quellcode: Rust mit dem stabilen Windows-Toolchain installieren und im Projektordner ausführen:

```powershell
cargo build --release
.\target\release\vedit.exe
```

Alternativ startet `cargo run --release` die Anwendung. Ein Ordner oder eine Datei kann beim Start als Argument übergeben werden:

```powershell
.\dist\vEdit\vedit.exe "C:\Code\MeinProjekt"
```

Beim ersten Start mit **Ctrl+Shift+O** Projektordner hinzufügen. **Ctrl+O**, das Hinzufügen von Projekten und **Save as…** verwenden die nativen Windows-Dialoge. Beim Öffnen können mehrere Dateien beziehungsweise Projektordner ausgewählt werden. Links oben zwischen Projekten wechseln, im Explorer Dateien öffnen oder mit **Ctrl+P** nach Dateinamen und Pfaden suchen. Die Dateiauswahl hat eine dauerhaft sichtbare Scrollleiste; Pfeiltasten, **Home**, **End** und **Page Up/Down** bewegen die Auswahl. **Esc** oder ein Klick außerhalb schließt die Auswahl. **Ctrl+Shift+P** öffnet die Befehlspalette; dort sind auch Aktionen ohne Tastenkürzel erreichbar.

Die Projekt-Tabs sitzen oben neben dem Namen der App und wechseln per Klick direkt zum Projekt. Ein gelber Punkt direkt hinter dem Namen zeigt uncommittete Git-Änderungen; der Tooltip nennt die Anzahl. Saubere Projekte erhalten keinen Punkt. Auch inaktive Projekte werden im Hintergrund aktualisiert. Viele Projekte lassen sich horizontal durchscrollen; Tastenkürzel machen den aktiven Tab sichtbar. Der **+**-Knopf beziehungsweise **Ctrl+Alt+P** öffnet die durchsuchbare Projektliste; Namen, Pfade sowie offene und geänderte Dateien helfen bei der Auswahl. Pfeiltasten und **Enter** wechseln direkt zum Projekt, **Esc** schließt die Liste. **Ctrl+Alt+PageDown** / **Ctrl+Alt+PageUp** wechseln zum nächsten beziehungsweise vorherigen offenen Projekt. Jedes Projekt besitzt eigene Tab-Leisten, Ansichten und Splits, die beim Wechsel zusammen mitwechseln und nach einem Neustart wiederhergestellt werden. Die Dateien bleiben geöffnet und ungespeicherte Änderungen erhalten. Weitere Projektordner lassen sich über die Liste mit dem nativen Dialog hinzufügen. Im Explorer steht der aktive Projektname als feste Überschrift über seinem Dateibaum.

Ein Rechtsklick auf einen Projekt-Tab (oder **Shift+F10** bei Tastaturfokus) öffnet **Reveal in Explorer**, **Copy path** und **Close project**. Die Aktionen gelten für den angeklickten Tab, ohne zuvor das aktive Projekt zu wechseln. **Close project** entfernt das Projekt aus der Sitzung; der Ordner und seine Dateien bleiben auf dem Datenträger. Bei ungespeicherten Änderungen fragt vEdit vor dem Schließen nach.

Kurze Bedienhinweise erscheinen als normale Tooltips. Detail-Tooltips für Pfade, Diagnosen, Git-Daten und Dokumentation lassen sich mit der Maus betreten: Text markieren und mit **Ctrl+C** kopieren oder **Copy all** verwenden. Lange Inhalte scrollen innerhalb der Anzeige. Beim Wechsel vom Auslöser zur Anzeige bleibt sie kurz offen; **Esc**, ein Klick außerhalb oder das Verlassen beider Bereiche schließt sie. Das reine Überfahren nimmt dem Editor keinen Tastaturfokus. Ein per **Show hover** geöffnetes Dokumentationsfenster bleibt wie bisher bis **Esc**, einer Cursorbewegung oder einem Klick außerhalb geöffnet. Vollständige Statusfehler bleiben zusätzlich per Klick im Detailfenster erreichbar.

### Einzelne Dateien und Drag-and-drop

Dateien aus dem Windows-Explorer lassen sich in vEdit ziehen, auch mehrere auf einmal. Die Markierung zeigt den Zielbereich; in einem geteilten Editor öffnet vEdit Dateien desselben Arbeitsbereichs im Bereich unter der Maus. Dateien aus einem geöffneten Projekt landen in dessen Tabs, andere im Bereich **Other files** neben den Projekten. Bereits offene Dokumente werden wiederverwendet; ungespeicherter Text bleibt erhalten. **Ctrl+O**, Startargumente und `vedit-cli open` verwenden dieselbe Zuordnung. Ein vorhandenes Dokument behält seinen Arbeitsbereich, auch wenn später sein Ordner als Projekt geöffnet wird.

Ein hineingezogener Ordner wird als Projekt hinzugefügt. Bei einer gemeinsamen Auswahl von Dateien und Ordnern werden zuerst die Ordner hinzugefügt und dann die Dateien zugeordnet. Die Daten auf dem Datenträger werden durch einen Drop weder verschoben noch kopiert. Nicht lesbare oder nicht unterstützte Dateien melden einen Fehler; andere Dateien der Auswahl werden trotzdem geöffnet.

**Other files** erscheint mit der ersten losen Datei. Die Seitenleiste zeigt die geöffneten Dateien mit ihren Pfaden; der Bereich besitzt eigene Tabs und Splits. Beim Neustart werden Dateien, Entwürfe, Cursor und Ansichten wiederhergestellt. Projektweite Suche, Git, Sprachserver und Run gehören zu regulären Projekten; Textbearbeitung, Syntaxfarben, Markdown-Vorschau, Suchen in der Datei, Speichern und Konfliktschutz funktionieren auch hier. **Ctrl+N** ohne geöffneten Bereich erstellt dort ein unbenanntes Dokument.

Nach dem Schließen der letzten Datei verschwindet **Other files**. **Reopen closed file** kann die letzte gespeicherte Datei in derselben Sitzung wieder öffnen. Das Schließen eines regulären Projekts lässt andere Arbeitsbereiche offen. Ungespeicherte Änderungen werden vor dem Schließen abgefragt.

**File → Open recent** und die gleichnamige Palettenaktion zeigen bis zu 30 zuletzt genutzte Dateien und Projektordner, auch nach dem Schließen oder einem Neustart. Namen und Pfade lassen sich durchsuchen; Dateipfade bleiben zur Unterscheidung sichtbar. Bereits offene Dateien behalten ihre ungespeicherten Änderungen. Nicht verfügbare Einträge bleiben im Verlauf, etwa für vorübergehend getrennte Laufwerke; ein erfolgloses Öffnen meldet den Fehler. **Clear recent history** leert nur diese Liste.

**Ctrl+P** berücksichtigt neben dem Projektindex auch bereits offene Dateien, einschließlich ignorierter Dateien und unbenannter Entwürfe. In **Other files** durchsucht es die dort geöffneten Dokumente. **Ctrl+Shift+E** fokussiert dort die Dateiliste; Pfeile, **Home/End**, **Page Up/Down** und **Enter** erlauben die Bedienung ohne Maus.

## Arbeiten im Editor

Der Knopf mit drei Strichen ganz links über dem Explorer öffnet das Hauptmenü mit **File**, **View** und **Run**. Er bleibt auch ohne geöffnetes Projekt erreichbar. Windows verwendet eine einzelne vEdit-Titelleiste: freie Flächen ziehen das Fenster, ein Doppelklick maximiert beziehungsweise stellt es wieder her. Rechts liegen Minimieren, Maximieren/Wiederherstellen und Schließen; bei ungespeicherten Änderungen fragt vEdit vor dem Schließen nach. Die Fensterränder lassen sich weiterhin ziehen, **Alt+Space** öffnet das Windows-Fenstermenü, und der Maximieren-Knopf unterstützt die Windows-Snap-Auswahl.

- Gleichnamige Tabs erhalten einen kurzen Ordnerzusatz, etwa `lib.rs — client\src`; eindeutig benannte Dateien bleiben kompakt. Auch gleichnamige offene Projekte lassen sich im Projektwechsel links oben anhand ihres Ordners unterscheiden.
- Der Speicherzustand bleibt in der Statusleiste sichtbar: **Saved**, **Unsaved**, **Saving…**, **Agent draft**, **Reloaded** nach einer Aktualisierung von der Festplatte, **Disk conflict** oder ein Dateifehler. Ein Klick auf **Unsaved** oder **Agent draft** zeigt die ungespeicherten Änderungen als Diff, ein Klick auf **Disk conflict** vergleicht die Festplattenfassung mit dem Editorinhalt. **Auto save on/off** schaltet das automatische Speichern für alle Projekte um. Neue Dateien und Agent-Entwürfe warten weiter auf **Ctrl+S**; der Tooltip erklärt die jeweilige Situation.
- Syntaxfarben, Zeilennummern, aktuelle Zeile und Dateipfad helfen bei der Orientierung. Die Statusleiste zeigt aktuelle Zeile / Gesamtzeilen und die Cursorspalte; Zeilenumbruch verändert die Gesamtzahl nicht. Dateien über 192 KiB erscheinen ohne Syntaxfarben, damit das Tippen flüssig bleibt; die Statusleiste sagt dann **Plain text: file too large for highlighting**. Bearbeiten, Suche und Sprachanalyse funktionieren weiter.
- Daneben stehen **chars** und **≈ tokens** für die Datei, bei markiertem Text ausdrücklich **Selection** für den Ausschnitt. Gezählt werden Unicode-Codepoints einschließlich Leerraum und Zeilenumbrüchen im aktuellen Editorinhalt. Zusammengesetzte Emojis können mehrere Codepoints enthalten; raw CRLF aus Agent-Edits zählt als zwei. Die Tokenzählung verwendet lokal den mitgelieferten `o200k_base`-Tokenizer und aktualisiert sich nach einer kurzen Eingabepause im Hintergrund. **…** bedeutet, dass die Berechnung noch aussteht. Der Tooltip nennt ungekürzte Zeichen- und Tokenzahlen für Datei und Auswahl sowie den Tokenizer. **≈** kennzeichnet eine Vergleichsbasis für andere Modelle, keinen gemessenen Agent-Kontextverbrauch: Gespräch, Anweisungen und Werkzeugdaten kommen zusätzlich hinzu. Die Auswahl wird als eigenständiger Text gezählt. In schmalen Fenstern werden die Werte gekürzt; in sehr schmalen Fenstern stehen sie im Tooltip der Cursorposition.
- JetBrains Mono (leichter Schnitt „Light“) wird mitgeliefert und ist die Standardschrift für Code: klare, nicht zu fette Zeichen, etwas mehr Zeilenabstand und Syntaxfarben ohne zusätzliche Kursivschrift oder Unterstreichungen. Unter **Settings → Editor font** stehen außerdem die Windows-Schriften Consolas und Cascadia Mono zur Wahl; fehlt eine Systemschrift, wird die mitgelieferte Schrift verwendet. Schriftgröße und Schriftwahl werden gespeichert.
- Ein Klick setzt den Cursor. Ziehen und Doppelklick wählen Text aus, ein Dreifachklick ganze Zeilen. Beim Ziehen am sichtbaren Rand scrollt der Editor vertikal und horizontal mit, auch bei stillgehaltener Maus; bei Zeilenumbruch nur vertikal. Im Vim-Normalmodus wird daraus eine Visual-Auswahl; im Insert-Modus bleibt die Texteingabe aktiv.
- Im Insert-Modus oder bei ausgeschaltetem Vim sucht **Ctrl+F** in der Datei; **Ctrl+H** öffnet Suchen und Ersetzen. Im Normalmodus startet `/` die Vim-Suche. Groß-/Kleinschreibung (**Aa**) und ganze Wörter (**ab**) lassen sich in Find/Replace einstellen. Standardmäßig wird nach wörtlichem Text gesucht; **.\*** schaltet auf reguläre Ausdrücke in Rust-Regex-Syntax um und bleibt über Neustarts eingestellt. Ein ungültiges Muster meldet **Invalid pattern** und ersetzt nichts. Beim Ersetzen fügen `$1`, `${name}` und `$0` Gruppen beziehungsweise den ganzen Treffer ein; `$$` steht für ein Dollarzeichen. Replace und Replace all lassen sich jeweils mit einem Undo-Schritt zurücknehmen.
- Steht der Cursor auf einer Klammer `(`, `[` oder `{` beziehungsweise ihrem Gegenstück, umrandet der aktive Bereich beide Klammern dezent. Ohne Vim und im Insert-Modus zählt auch die Klammer direkt vor dem Cursor. Gesucht wird bis zu 20.000 Zeichen weit; in Dateien über 2 MB entfällt die Markierung.
- **Ctrl+Shift+F** durchsucht das aktive Projekt. Ein Treffer öffnet die passende Datei, zeigt die Fundstelle mittig und hebt die Zielzeile kurz hervor.
- **Alt+←** / **Alt+→** sowie die Zurück- und Vorwärts-Taste der Maus kehren zur vorherigen Stelle zurück beziehungsweise wieder vor. Jeder Editorbereich führt dafür seinen eigenen Verlauf. Vermerkt werden Dateiwechsel, Sprünge zur Definition, Suchtreffer, **Ctrl+P**, Bookmarks, Problems, Agent-Markierungen, Stellen aus Review und Diff sowie Cursorsprünge über mehr als zehn Zeilen. Eine inzwischen geschlossene Datei wird an der gemerkten Stelle wieder geöffnet. Stellen in nie gespeicherten Dateien entfallen mit deren Schließen, gelöschte Dateien in einem bereits aufgeklappten Explorer-Ordner werden übersprungen. Der Verlauf bleibt beim Tab-Wechsel erhalten, aber nicht über einen Neustart hinweg. Im Vim-Normalmodus gehen außerdem **Ctrl+O** und **Ctrl+I** zurück beziehungsweise vor, wie in Vims Sprungliste.
- **Settings** (**Ctrl+,**) gruppiert Lese-, Layout-, Speicher- und Vim-Einstellungen. **Interface zoom** vergrößert die gesamte Oberfläche einschließlich Explorer, Git und Vorschau (75–200 %). Beim Ziehen am Regler oder Prozentfeld wird der Zoom erst nach dem Loslassen angewendet, damit die Auswahl stabil bleibt. **Ctrl++ / Ctrl+−** ändern den Zoom sofort, **Ctrl+0** stellt 100 % wieder her, auch im Einstellungsdialog. Unabhängig davon sind Codeschriftgröße (10–32), Zeilenabstand (1–2×), absolute/relative/ausgeschaltete Zeilennummern und die Hervorhebung der Cursorzeile einstellbar. Dazu kommen Einrückungsbreite, Zeilenumbruch, **Smooth scrolling**, Vim, Leader-Taste und Autosave samt Verzögerung. Einstellungen werden automatisch gespeichert. Vim und Autosave sind standardmäßig eingeschaltet, weiches Scrollen ist ausgeschaltet.
- Bei Zeilenumbruch bleibt die Zeilennummer an der ersten sichtbaren Reihe einer Quelltextzeile stehen; Fortsetzungen bekommen keine zusätzliche Nummer.
- In versionierten Dateien zeigt ein schmaler Balken neben den Zeilennummern die Abweichungen vom letzten Commit (HEAD), einschließlich ungespeicherter Änderungen: grün für neue, orange für geänderte Zeilen, ein roter Keil für entfernte Zeilen. Unversionierte Dateien erhalten keine Markierung. Der Tooltip zeigt die bisherigen Zeilen. Ein Klick öffnet eine kleine Ansicht mit **Revert change**: Sie stellt nur diesen Abschnitt im Editorinhalt wieder her, als ein Schritt, den **Ctrl+Z** beziehungsweise Vim `u` rückgängig macht. Datei und Git-Index bleiben unverändert, bis gespeichert wird. Die Markierungen erscheinen in allen Editorbereichen und lassen sich unter **Settings → Reading & appearance → Show Git changes in gutter** ausschalten.
- **Ctrl+Z** sowie **Ctrl+Y** / **Ctrl+Shift+Z** verwenden mit eingeschaltetem Vim dieselbe Undo-Historie wie `u` / **Ctrl+R**, auch im Insert- und Visual-Modus. Im Insert-Modus bleibt die Texteingabe aktiv; Vim-Register und System-Zwischenablage bleiben getrennt.
- Im Insert-Modus oder bei ausgeschaltetem Vim: **Tab** springt zur nächsten Einrückungsposition, **Shift+Tab** rückt aus, **Ctrl+/** schaltet Zeilenkommentare um. **Alt+↑/↓** verschiebt Zeilen oder ausgewählte Zeilenblöcke; **Alt+Shift+↓** dupliziert sie. Beide Aktionen erhalten Cursor beziehungsweise Vim-Auswahl und sind rückgängig machbar. Neue Zeilen übernehmen die Einrückung.

Dateiindex, Dateizugriffe, Projektsuche, Git und Sprachanalyse laufen im Hintergrund. Auch die Suche in großen offenen Dateien berechnet ihre Treffer im Hintergrund; währenddessen zeigt sie **Searching…** und verwirft überholte Ergebnisse bei weiterer Eingabe oder einem Dateiwechsel. Der Explorer zeigt auch ignorierte und generierte Projektdateien. Er liest jeden geöffneten Ordner vollständig und lädt Unterordner erst beim Aufklappen; ein begrenzter Quick-Open-Index kann daher keine Explorer-Dateien ausblenden. VCS-Metadaten wie `.git` bleiben verborgen. Die Projektsuche bietet unter **Exclusions** eine gespeicherte Liste von Ordnern und Dateimustern, etwa `src/generated` oder `**/*.min.js`. Sie liest unabhängig vom Explorer den Datenträger; eine leere Filterliste erlaubt auch generierte Ordner, während `.gitignore` und VCS-Verzeichnisse weiter berücksichtigt werden. Die Projektsuche verfolgt symbolische Links nicht. Ihre Ressourcenlimits und die des Quick-Open-Index bleiben als Hinweise sichtbar.

Rechtsklick auf Dateien und Ordner markiert den betroffenen Eintrag über die ganze Zeile; sein Name steht zusätzlich im Menü. Die aktive Datei bleibt separat erkennbar. Das Menü bietet Öffnen, seitliches Öffnen, neue Dateien oder Ordner, Umbenennen, **Delete**, Pfade kopieren und Anzeigen im Windows-Explorer. **Delete** beziehungsweise **Entf** im fokussierten Explorer verschiebt das gewählte Element in den Windows-Papierkorb; dort lässt es sich wiederherstellen. Ungespeicherte Dokumente und Agent-Entwürfe im betroffenen Pfad verhindern das Löschen. Netzwerkpfade und Verknüpfungen werden nicht dauerhaft gelöscht, sondern mit einer Meldung abgelehnt. Auf der freien Fläche im Explorer lassen sich neue Dateien und Ordner direkt im Projekt anlegen; dort sind auch Projektpfad, Windows-Explorer, Aktualisieren und **Collapse all** erreichbar. **Collapse all** im Explorer-Menü klappt die Ordner des aktiven Projekts ein. Tab-Menüs bieten Speichern, Save as, Splits und Schließen; **Close saved tabs** erhält geänderte Dokumente. Im Editor stehen Ausschneiden, Kopieren, Einfügen, Auswahl und Undo/Redo im Kontextmenü bereit.

**Ctrl+Shift+T**, **File → Reopen closed tab** oder die gleichnamige Palettenaktion öffnet zuletzt geschlossene Dateien des aktiven Projekts im aktuellen Editorbereich. Cursorposition und Markdown-Ansicht kommen zurück. Jedes Projekt merkt sich bis zu 20 Dateien bis zum Schließen des Projekts oder der Anwendung. Bereits anderweitig offene Dateien behalten ihren aktuellen, auch ungespeicherten Text; sonst wird die aktuelle Datei vom Datenträger geladen. Bewusst verworfene Änderungen und nie gespeicherte Entwürfe werden nicht wiederhergestellt.

Im fokussierten Explorer navigieren Pfeiltasten oder `h j k l`; **Enter** öffnet die Datei, **Home/End** und **Page Up/Down** bewegen sich durch längere Listen. **F2** startet Umbenennen mit ausgewähltem Dateinamen ohne Erweiterung; bei Ordnern wird der ganze Name ausgewählt. **Shift+F10** öffnet das Kontextmenü des ausgewählten Eintrags.

Ein einfacher Klick auf eine Datei verwendet pro Editorbereich einen **Vorschau-Tab** mit kursivem Dateinamen. Der nächste Klick auf eine andere, noch nicht geöffnete Datei ersetzt diese Vorschau. Ein Doppelklick auf die Datei oder ihren Tab behält sie dauerhaft; ebenso Bearbeiten, Speichern, Teilen oder Verschieben des Tabs. Auch nach Autosave oder Undo bleibt eine bearbeitete Datei geöffnet. Bereits dauerhaft geöffnete Tabs werden nicht zur Vorschau. **Enter**, **Ctrl+O**, **Ctrl+P** und das Kontextmenü öffnen Dateien dauerhaft. Unter **Settings → Layout → Preview files from Explorer** lässt sich das standardmäßig eingeschaltete Verhalten ausschalten. Dabei bleiben aktuelle Vorschauen geöffnet. Wiederhergestellte Sitzungstabs werden ebenfalls dauerhaft geöffnet.

Der Explorer sortiert Ordner vor Dateien und macht die Hierarchie mit dezenten Verbindungslinien sichtbar. Dateinamen stehen in einheitlichen Spalten; Auswahl und aktive Datei bleiben über die gesamte Zeilenbreite lesbar. Git-ignorierte Dateien und Ordner erscheinen grau, bleiben aber ganz normal aufklappbar, auswählbar und bearbeitbar. Der Tooltip nennt **Ignored by Git**. Wie in VS Code tragen geänderte Dateien ihre Git-Farbe und rechts einen Buchstaben: **M** (orange) für geändert, **U** beziehungsweise **A** (grün) für neu, **R** (blau) für umbenannt und **!** (rot) für Merge-Konflikte. Ordner mit Änderungen erhalten die Farbe ihrer wichtigsten Änderung und einen kleinen Punkt; der Tooltip nennt den Status. Pfad-Tooltips erscheinen rechts neben dem Mauszeiger, bei Platzmangel links daneben, damit die Einträge darunter sichtbar bleiben. Regeln aus untergeordneten `.gitignore`-Dateien und deren Ausnahmen werden berücksichtigt. **Ctrl+O** öffnet Dateien direkt; **Ctrl+P** verwendet einen eigenen Suchindex.

Neue, gelöschte und umbenannte Dateien und Ordner erscheinen im Explorer und in **Ctrl+P** ohne Aktualisieren, auch wenn ein Agent sie im Terminal anlegt und vEdit gerade nicht das aktive Fenster ist. Das gilt für Projekte mit und ohne Git-Repository. vEdit beobachtet dazu den Projektordner mit Windows-Dateibenachrichtigungen; im Leerlauf kostet das keine Rechenzeit. Nach einer kurzen Ruhepause werden nur betroffene, bereits geladene Ordner im Hintergrund neu gelesen. Auswahl, aufgeklappte Ordner und Scrollposition bleiben erhalten, und ein gelöschter aufgeklappter Ordner verschwindet ohne Fehlermeldung. Bloßes Speichern vorhandener Dateien, Vorgänge in `.git` und generierte Ordner wie `target` oder `node_modules` lösen keine Neuindizierung aus. Auch geänderte `.gitignore`-Dateien werden übernommen. Geöffnete Dokumente schließt oder verändert diese Aktualisierung nie; für sie gilt der Abgleich mit der Festplatte unter „Speichern und Wiederherstellung“. Wo Windows keine Benachrichtigungen liefert, etwa auf manchen Netzlaufwerken, liest vEdit beim Aktivieren des Fensters neu. **Refresh** im Explorer-Menü bleibt jederzeit verfügbar.

## Seitenleisten einrichten

Explorer, Suche, Source control und Review lassen sich unabhängig links oder rechts anordnen. Jede Seite zeigt genau einen aktiven Inhalt. Die Symbole ganz links wechseln zur jeweiligen Ansicht auf ihrer zugeordneten Seite. Explorer und Git können nebeneinander oder beide auf derselben Seite liegen; auf derselben Seite ersetzt ein Wechsel den sichtbaren Inhalt. Der kompakte Seitenleistenkopf zeigt den Namen der aktiven Ansicht und ihre Aktionen; im Explorer steht dort direkt der Projektname. Der Dateibaum beginnt unmittelbar darunter. Der Drei-Punkte-Button behält beim Überfahren und Öffnen seine Größe, sodass der Inhalt nicht verrutscht. Beide Seiten können auch leer oder ausgeblendet bleiben. **Ctrl+Alt+B** blendet nur rechts aus oder ein; **Ctrl+Alt+E** tut dasselbe links. Beide Kürzel funktionieren auch im Vim-Normalmodus. Ein erneuter Klick auf das aktive Symbol ganz links blendet die ganze Seite aus, ohne die Zuordnung zu verändern. Der Seitenleistenkopf hat bewusst kein **×**, damit es nicht mit dem Schließen von Tabs oder Fenster verwechselt wird. Ziehen an der Innenkante ändert ihre Breite.

Das unauffällige **⋯**-Menü des aktiven Panels bietet **Move panel to left/right**, **Hide panel**, **Show …** für weitere Ansichten derselben Seite und **Hide left/right sidebar**. Bei Source control stehen dort zusätzlich **Fetch**, **Pull**, **Push**, **Stage/Unstage all changes** und **Copy repository path**. **View → Arrange sidebar panels…** und **Settings → Layout** bieten dieselbe Zuordnung und Sichtbarkeit. Zuordnung, aktive Ansicht und Breiten werden gespeichert; bisherige Sitzungen werden übernommen. Bei schmalen Fenstern begrenzt vEdit die Breiten zugunsten des Editors. Reicht der Platz nicht für beide Seiten, erscheint ein Hinweis zum Wechseln der vorübergehend ausgeblendeten Seite; das gespeicherte Layout bleibt erhalten.

## Bereiche für Agenten markieren

Text mit der Maus oder Vim `v` / `V` auswählen und **Leader a** oder **Ctrl+Shift+A** drücken. Der Ausschnitt bleibt als zusätzliche lila Markierung erhalten; weitere Bereiche können in beliebigen Dateien und Projekten hinzukommen. Die gewöhnliche Auswahl und das Vim-Register bleiben erhalten. Das **×** am Rand entfernt einen Bereich. Das Symbol **For agent** in der Tab-Leiste zeigt die Gesamtzahl; **Leader A** oder **Ctrl+Alt+Shift+A** öffnet die gemeinsame Übersicht mit Dateigruppen, Suche, optionalen Notizen, **Clear all** und **Restore last**.

Die Markierung enthält den Text zum Zeitpunkt der Auswahl, einschließlich ungespeicherter Änderungen. Bei Änderungen vor dem Bereich wandert sie mit; Änderungen innerhalb des Bereichs kennzeichnen den gespeicherten Ausschnitt als veraltet, statt einen anderen Text als ursprünglich ausgewählt auszugeben. Geschlossene Dateien und Projektwechsel löschen die Sammlung nicht.

Die Übersicht zeigt die tatsächlich enthaltenen Zeilen und die Zeichenanzahl. Bei **Restore last** steht sichtbar, wie viele entfernte oder von einem Agenten abgeholte Bereiche zurückkommen. Die letzte Entfernung, **Clear all** oder Abholung bestimmt das Ziel; neue Markierungen und Notizen bleiben beim Wiederherstellen erhalten.

Agenten lesen die Sammlung mit `vedit-cli agent-marks`. Mit `agent-marks take` holen sie die Bereiche ab und entfernen die lila Markierungen; die letzte abgeholte Sammlung bleibt über Neustarts hinweg wiederherstellbar. Große Sammlungen und lange Ausschnitte werden vollständig über Folgeseiten geliefert. Die Anleitung gehört zum [Repository-Skill](.agents/skills/vedit/SKILL.md), der auch im portablen Paket enthalten ist.

## Tastenkürzel

| Aktion | Tastenkürzel |
| --- | --- |
| Projektordner hinzufügen | Ctrl+Shift+O |
| Offenes Projekt auswählen | Ctrl+Alt+P |
| Nächstes / vorheriges Projekt | Ctrl+Alt+PageDown / Ctrl+Alt+PageUp |
| Datei über Dialog öffnen | Ctrl+O |
| Datei im Projekt finden | Ctrl+P |
| Befehlspalette | Ctrl+Shift+P |
| Explorer fokussieren | Ctrl+Shift+E |
| Aktiven Editorbereich fokussieren | Ctrl+1 |
| Sichtbare Seitenleisten, Editorbereiche und Output durchlaufen | F6 / Shift+F6 |
| Räumlich links / rechts / oben / unten fokussieren | Ctrl+Alt+← / → / ↑ / ↓ |
| Aktuellen Tab in einen benachbarten Split verschieben | Ctrl+Alt+Shift+← / → / ↑ / ↓ |
| Git / Output fokussieren | Ctrl+Shift+G / Ctrl+Alt+O |
| Neue Datei | Ctrl+N¹ |
| Aktuelle Datei speichern | Ctrl+S |
| Alle geänderten Dateien speichern | Ctrl+Shift+S |
| Tab schließen | Ctrl+W |
| Geschlossenen Tab wieder öffnen | Ctrl+Shift+T |
| Nächster / vorheriger Tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Suchen / Ersetzen in der Datei | Ctrl+F² / Ctrl+H |
| Nächster / vorheriger Treffer | F3 / Shift+F3 |
| Projekt durchsuchen | Ctrl+Shift+F |
| Zu einer Zeile oder `Zeile:Spalte` springen | Ctrl+G |
| Ganze Zeile / ausgewählte Zeilen löschen | Ctrl+Shift+K |
| Zeile darunter / darüber einfügen | Ctrl+Enter / Ctrl+Shift+Enter |
| Linke Seitenleiste ein-/ausblenden | Ctrl+Alt+E oder Ctrl+B² |
| Rechte Seitenleiste ein-/ausblenden | Ctrl+Alt+B |
| Oberfläche vergrößern / verkleinern / zurücksetzen | Ctrl++ / Ctrl+− / Ctrl+0 |
| Ausgabe öffnen | Ctrl+` oder View → Output |
| Zeilenumbruch umschalten | Alt+Z |
| Editor nach rechts / unten teilen | Ctrl+\ / Ctrl+Shift+\ |
| Aktuellen Editorbereich schließen | Ctrl+Alt+W |
| Einstellungen | Ctrl+, |
| Vervollständigung anfordern | Ctrl+Space |
| Zur Definition springen | F12 oder `gd` im Vim-Normalmodus |
| Referenzen finden | Shift+F12 oder `gr` im Vim-Normalmodus |
| Dokumentation unter dem Cursor anzeigen | `K` im Vim-Normalmodus oder Palette → Show hover |
| Symbol in der Datei / im Workspace suchen | Ctrl+P, dann `@` / Ctrl+T |
| Zurück / vorwärts zur letzten Stelle | Alt+← / Alt+→ oder Maustasten Zurück / Vorwärts³ |
| Nächster / vorheriger Fehler oder Warnung | F8 / Shift+F8 oder `]d` / `[d` im Vim-Normalmodus |
| Fehler und Warnungen anzeigen | Ctrl+Shift+M |
| Auswahl für einen Agenten markieren | Ctrl+Shift+A oder Leader a |
| Agent-Markierungen öffnen | Ctrl+Alt+Shift+A oder Leader A |
| Interaktives Terminal im Projekt öffnen | Run → Open terminal |
| Aktives Projekt prüfen | F5 |
| Aktive Datei ausführen | Ctrl+F5 |
| Projekttests starten | Alt+F6 |

¹ Im Vim-Insert-Modus fordert **Ctrl+N** stattdessen Vervollständigung an. Neue Dateien sind auch über **File → New file** erreichbar.

² Wenn der Vim-Normal- oder Visualmodus im Editor aktiv ist, blättern **Ctrl+F** und **Ctrl+B** eine Seite weiter/zurück. Suche mit `/`, die Befehlspalette oder **View → Explorer** bleiben erreichbar.

³ Im Vim-Normalmodus gehen auch **Ctrl+O** / **Ctrl+I** zurück beziehungsweise vor. **Datei über Dialog öffnen** bleibt dort über das Menü **File**, die Befehlspalette, den Insert- oder Visual-Modus und ohne Vim mit **Ctrl+O** erreichbar. Wer **Open file** selbst belegt, behält diese Belegung auch im Normalmodus.

## Eigene Tastenkürzel und räumlicher Fokus

**Settings → Keyboard** oder die Palettenaktion **Keyboard shortcuts** öffnet das durchsuchbare Befehlsverzeichnis. Auf eine Belegung klicken, die neue Kombination drücken; **Esc** bricht die Aufnahme ab. **Disable**, **Reset** und **Reset all** deaktivieren beziehungsweise restaurieren Belegungen. Konflikte werden mit dem betroffenen Befehl erklärt. Auch Aktionen ohne Standardkürzel sind belegbar. Änderungen gelten sofort und bleiben nach Neustarts erhalten; Menüs und Palette zeigen die aktuelle Belegung. Die hier genannten Kombinationen sind die Standardwerte. Textnavigation, Zwischenablage und einzelne Vim-Motions behalten ihre normale Eingabebedeutung.

**Ctrl+Alt+Pfeiltasten** wechseln anhand der sichtbaren Anordnung zwischen beiden Seitenleisten, Editor-Splits und dem unteren Ausgabebereich. Die Rückkehr in den Editor berücksichtigt die vorherige Zeile beziehungsweise Spalte der Splits. **F6 / Shift+F6** durchlaufen sichtbare Bereiche; ausgeblendete Seitenleisten werden dabei nicht geöffnet. **Ctrl+Alt+Shift+Pfeiltasten** verschieben den aktiven Tab in einen benachbarten Editor-Split. **Ctrl+Tab / Ctrl+Shift+Tab** wechseln Tabs im aktiven Bereich. Alle diese Befehle sind neu belegbar.

In Git bewegen **↑↓ / Home / End** den Fokus, **←→** öffnen und schließen Ordner, **Enter** öffnet Diff oder Commit. **Tab** erreicht Stage-/Unstage-Schaltflächen und Commit-Eingabe. Pfeile und Enter bleiben innerhalb der Commit-Nachricht normale Texteingaben. Der sichtbare Fokus kehrt nach dem Schließen eines Git-Diffs zurück.

## Vim

Neue und geöffnete Dateien beginnen im **Normalmodus**. `i` wechselt zum Schreiben in den **Insert-Modus**, **Esc** führt zurück. Der aktuelle Modus steht unten in der Statusleiste. Wer normale Texteditor-Bedienung bevorzugt, kann **Vim motions** in den Einstellungen ausschalten.

**Settings → Swap J and K** tauscht die Bewegungsrichtungen von `j` und `k`, einschließlich Anzahl, Operatoren und Visual-Auswahl. Die Einstellung gilt auch für die Navigation im Explorer und das Scrollen der Markdown-Vorschau. Eingetippter Text und Buchstaben als Ziel von `f`, `t` oder `r` bleiben unverändert; Pfeil- und Seitentasten behalten ihre Richtung.

**Ctrl+C**, **Ctrl+X** und **Ctrl+V** bedienen die Windows-Zwischenablage auch im Vim-Modus. Kopieren erhält Modus und Auswahl; ohne Auswahl wird die aktuelle Zeile kopiert. Ausschneiden ist rückgängig machbar. Einfügen im Normalmodus setzt den Text direkt an die Cursorposition und verändert das Vim-Register nicht. `y`, `d`, `p` und `P` verwenden weiterhin die Vim-Bedienung; `p` setzt Registertext hinter, `P` vor den Cursor, bei kopierten Zeilen unterhalb beziehungsweise oberhalb der aktuellen Zeile.

**Space** ist die Standard-Leader-Taste. Im Normal- und Visual-Modus öffnet sie eine kurze Hilfe: **Space m** setzt oder entfernt einen Bookmark an der aktuellen Zeile, **Space b** öffnet die durchsuchbare Bookmark-Liste. Bookmarks speichern Datei, Zeile und Cursorposition, bleiben beim Schließen einer Datei und nach Neustarts erhalten und können benannt oder entfernt werden. Im Picker lässt sich zwischen aktuellem Projekt und allen Bookmarks wechseln; die Markierung erscheint auch im Editor-Gutter. Kleine Verschiebungen werden beim Anspringen anhand der gespeicherten Quelltextzeile berücksichtigt. **Settings → Leader key** bietet Space, Komma und Backslash; die CLI erlaubt auch andere einzelne Zeichen. Im Insert-Modus bleibt Space ein normales Leerzeichen.

**Space Space f** startet den Bildschirm-Sprung. Ein bis drei Zeichen eingeben, etwa `gi`: vollständig sichtbare Treffer werden hervorgehoben und erhalten Buchstabenlabels. Das angezeigte Label tippen, um den Cursor dorthin zu bewegen. Die Tasten **f j d k s l a g h** werden bevorzugt, danach benachbarte Buchstaben; Shift ist nicht nötig. Solange die Suche noch ergänzt werden kann, bleiben mögliche Fortsetzungen frei: Bei `git`, `give` und `girl` nach `gi` sind beispielsweise `t`, `v` und `r` für die Suche reserviert. Zahlen und Sonderzeichen lassen sich ebenfalls normal suchen.

Nach dem dritten Suchzeichen ist die Suche festgelegt; **Enter** legt sie schon vorher fest und gibt alle Buchstaben für Labels frei. Bei genau einem Treffer springt **Enter** direkt dorthin. Falls alle Labelbuchstaben mögliche Fortsetzungen sind, weiter eingrenzen oder **Enter** drücken. Viele Treffer erhalten gleich lange Labels mit zwei, bei sehr vielen Treffern auch mehr Buchstaben. Nach dem ersten Labelbuchstaben werden die übrigen Ziele abgeblendet und der eingegebene Teil des Labels gedimmt. Beim Verfeinern bleiben Labels möglichst an derselben Stelle; bei einer anderen Labellänge oder einer neuen Kollision werden sie neu vergeben.

**Backspace** korrigiert zuerst ein angefangenes Label, gibt danach eine mit **Enter** festgelegte Suche wieder frei oder entfernt das letzte Suchzeichen. **Esc** bricht ab. Die Statusleiste zeigt den Suchtext und den nächsten Schritt. Im Visual-Modus erweitert der Sprung die bestehende Auswahl. Die Suche wirkt auf den aktiven Editorbereich und verändert keinen Text. Bookmarks und Bildschirm-Sprung sind auch über das Menü und die Befehlspalette erreichbar.

`v` startet eine Zeichenauswahl; `V` (**Shift+V**) wählt ganze Zeilen. Mit Bewegungen erweitern, dann beispielsweise `y` kopieren, `d` löschen, `c` ändern oder `>` / `<` ein-/ausrücken. **Esc** beendet die Auswahl.

| Aufgabe | Beispiele |
| --- | --- |
| Bewegen | `h j k l`, `w b e ge`, `W B E gE` |
| Zeilen- und Dateigrenzen | `0 ^ $ g_`, `gg`, `G`, `42G`, `8\|`, `Enter`, `+` / `-` |
| Bildschirmzeilen | `H` / `M` / `L` springen zur obersten, mittleren und untersten sichtbaren Zeile; `3H` / `3L` zählen vom Rand |
| Klammern / Absätze | `%`, `{`, `}` |
| Zeichen in der Datei finden | `f` / `F`, `t` / `T` suchen über Zeilengrenzen hinweg; `;` wiederholt, `,` sucht in Gegenrichtung |
| Einfügen | `i a I A`, `o O`; `3ox` öffnet drei Zeilen |
| Löschen / Ändern | `x`, `dd`, `dw`, `cw`, `cc`, `S`, `D`, `C`, `r` + Zeichen |
| Kopieren / Einfügen | `yy`, `yw`, `p`, `P` |
| Anzahl voranstellen | `3w`, `2dd`, `d2w`, `2d3w` |
| Textobjekte | `ciw`, `daw`, `di(`, `ca{`, `yi"`, `viw`, `dip`, `yap` |
| Klammern und Anführungszeichen | `ysiw)` umschließt ein Wort, `yss"` eine Zeile; `ds"` entfernt, `cs"'` ersetzt; im Visual-Modus `S` + Zeichen |
| Einrücken / Zeilen verbinden | `>>`, `<<`, `J`; `gJ` verbindet ohne zusätzliche Leerzeichen |
| Groß-/Kleinschreibung | `~`, `guw`, `gUiw`, `g~$`, `gugu` / `gUgU` / `g~g~`; in Visual `u`, `U`, `~` |
| Zahlen ändern | **Alt+A** erhöht, **Alt+X** verringert die nächste Zahl ab dem Cursor; `5` **Alt+A** addiert 5 |
| Letzte Auswahl | `gv` wählt den zuletzt markierten Bereich erneut aus |
| Letzte Änderung wiederholen | `.`; auch Insert-Änderungen werden erfasst |
| Rückgängig / Wiederholen | `u` / Ctrl+R |
| Blättern | Ctrl+F / Ctrl+B; Ctrl+U für eine halbe Seite zurück. Ctrl+D aktiviert Multicursor. |
| Ansicht ausrichten | `zt` / `zz` / `zb` setzen die Cursorzeile oben, mittig beziehungsweise unten; `z Enter`, `z.` und `z-` gehen zusätzlich zum ersten Zeichen |
| Sprachfunktionen | `gd` springt zur Definition, `gr` findet Referenzen, `K` zeigt die Dokumentation unter dem Cursor |
| Suchen | `/` sucht vorwärts, `?` rückwärts; `n` wiederholt die Richtung, `N` kehrt sie um. `*` / `#` suchen das Wort unter dem Cursor |

Wie in Vim beendet ein abschließender Zeilenumbruch die letzte Zeile; dahinter beginnt keine weitere leere Zeile. `G`, `dG`, `2dd`, `J` und `:%s` arbeiten deshalb bis zur letzten Textzeile und lassen den Zeilenumbruch am Dateiende stehen. Das gilt auch für Dateien mit CRLF. Wort-Textobjekte wie `iw` und `aw` bleiben in ihrer Zeile. Stehen die Klammern von `di{` oder `ci{` auf eigenen Zeilen, wirkt der Befehl auf die Zeilen dazwischen. `ip` und `ap` wählen einen Absatz, `ap` mit den folgenden Leerzeilen.

Vim's **Ctrl+A** und **Ctrl+X** bleiben in vEdit **Alles auswählen** und **Ausschneiden**; dafür ändern **Alt+A** und **Alt+X** Zahlen, solange diese Tasten keinem eigenen Kürzel zugewiesen sind. Erkannt werden ganze Dezimalzahlen einschließlich eines vorangestellten Minus; führende Nullen behalten ihre Breite.

Nicht unterstützte Normalmodus-Befehle wie `q`, `@`, `m`, `` ` ``, `'` oder `R` ändern keinen Text. Sie verbrauchen ihr Argument, etwa das Register bei `qa`, und die Statusleiste meldet beispielsweise **Not supported in vEdit: q**.

Das unbenannte Kopierregister wird zwischen Tabs geteilt: etwa in einer Datei `yy`, in einer anderen `p`. Es bleibt von der Systemzwischenablage getrennt. Jeder Tab hat seine eigene Undo-Historie; eine zusammenhängende Insert-Änderung wird als Schritt behandelt.

Die Vim-Suche springt schon während der Eingabe zum Treffer und markiert alle sichtbaren Ergebnisse. **Enter** bestätigt und gibt den Editor frei, **Esc** bricht ab und stellt die Ausgangsposition wieder her. Eine leere bestätigte Eingabe wiederholt das letzte Suchmuster. Im Visual-Modus erweitert die Suche die Auswahl; `n` und `N` unterstützen Zählwerte wie `3n`. `:noh` blendet die Treffer-Markierungen aus und behält das Suchmuster für den nächsten Sprung.

**↑/↓** beziehungsweise **Ctrl+P/N** in der Vim-Suche rufen bestätigte Suchmuster auf; die aktuelle Eingabe bleibt beim Zurückkehren erhalten. **Ctrl+W** löscht das Wort vor dem Cursor, **Ctrl+U** den Anfang der Eingabe. **Shift+Enter** in Find springt zum vorherigen Treffer.

Die Zeichensuche `f` / `F` und `t` / `T` reicht bis zum Datei-Ende beziehungsweise Datei-Anfang und funktioniert auch mit Zählwerten, Auswahlen und Operatoren wie `dfx` oder `cfx`. `;` wiederholt die ursprüngliche Richtung, `,` sucht einmal in Gegenrichtung; die Zeichensuche bleibt unabhängig vom Suchmuster für `n` / `N`. `.` wiederholt Textänderungen. `p` / `P` fügen aus dem Vim-Register ein; im Visual-Modus behält `P` dessen Inhalt für weitere Einfügungen bei.

**Space p** fügt das zuletzt mit `y` kopierte Stück ein, auch nachdem `d` oder `c` das unbenannte Register überschrieben haben. Dasselbe geht mit `"0p` beziehungsweise `"0P`, auch mit Zählwerten und im Visual-Modus. Das Yank-Register `0` wird wie das unbenannte Register zwischen Tabs geteilt und bleibt unabhängig von der Systemzwischenablage.

Die grundlegenden Surround-Befehle unterstützen `()` / `[]` / `{}` / `<>` sowie einfache, doppelte und Backtick-Anführungszeichen. Ein schließendes Zeichen wie `)` umschließt direkt, ein öffnendes wie `(` fügt innen Leerzeichen hinzu. Bewegungen, Textobjekte, Auswahlen und `.` sind kombinierbar; jede abgeschlossene Änderung bildet einen Undo-Schritt. XML-Tag-Eingabe und benutzerdefinierte Surround-Vorlagen sind nicht enthalten.

**Zweimal Esc kurz hintereinander** blendet Suchmarkierungen aus. Das Suchmuster bleibt erhalten; `n`, `N` oder eine neue Suche aktivieren die Markierungen wieder. Vim-Yanks heben den kopierten Bereich kurz hervor; `yy` markiert dabei die ganze Zeile.

Die Suche unterscheidet standardmäßig Groß- und Kleinschreibung. `\c` im Muster ignoriert sie, `\C` erzwingt sie. Reguläre Ausdrücke verwenden die Rust-Regex-Syntax, etwa `foo|bar`, `\w`, `^` und `$`; der vollständige Vim-Regex-Dialekt und Such-Offsets sind nicht enthalten. Die Find/Replace-Leiste sucht wörtlich, solange **.\*** nicht eingeschaltet ist.

Über `:` stehen diese Befehle zur Verfügung:

- `:w` speichern, `:wa` alle geänderten Dateien speichern.
- `:q` Tab schließen, `:wq` / `:x` speichern und schließen.
- `:q!` Tab schließen und ungespeicherte Änderungen verwerfen.
- `:e src/main.rs` eine Datei relativ zum Projekt öffnen.
- `:42` zu Zeile 42 springen, `:$` zur letzten Zeile; `:noh` die Treffer-Markierungen ausblenden.
- `:bn` / `:bp` zum nächsten beziehungsweise vorherigen Tab wechseln; `:new` eine Datei anlegen.
- `:split` / `:vsplit` den Editor unterhalb beziehungsweise rechts teilen; optional folgt ein Dateipfad.
- `:set wrap` / `:set nowrap` den Zeilenumbruch einstellen; `number`, `nonumber` und `relativenumber` steuern Zeilennummern.
- `:s/alt/neu/` ersetzt den ersten Treffer der aktuellen Zeile; `:%s/alt/neu/g` ersetzt alle Treffer im Dokument. Bereiche wie `:2,8s/alt/neu/g` und die Visual-Auswahl sind möglich. `i` ignoriert Groß-/Kleinschreibung, `I` erzwingt sie, `n` zählt ohne Änderung, `e` erlaubt null Treffer. Muster verwenden wie die Vim-Suche Rust-Regex-Syntax; `&` und `\1` bis `\9` setzen Treffer und Gruppen in der Ersetzung ein. `\r` erzeugt einen Zeilenumbruch, `\t` einen Tab. Anders als in Vim, wo `\n` ein Nullzeichen einfügt, erzeugt auch `\n` einen Zeilenumbruch. `\u` / `\l` schreiben das nächste Zeichen groß beziehungsweise klein, `\U` / `\L` den folgenden Text bis `\E` oder `\e`; so beginnt nach `:s/\w+/\u&/g` jedes Wort mit einem Großbuchstaben. `~` steht wörtlich für eine Tilde, nicht für die vorherige Ersetzung. Die Änderung bildet einen Undo-Schritt; ungültige Muster oder nicht unterstützte Flags verändern keinen Text. Interaktive Bestätigung mit `c` ist nicht enthalten.

Die `:`-Befehlszeile besitzt ebenfalls einen Verlauf mit **↑/↓**, der die aktuelle Eingabe beim Zurückkehren erhält.

vEdit implementiert einen eigenen Vim-Kern. Visual-Block, Makros (`q`, `@`), Marken (`m`, `` ` ``, `'`), der Replace-Modus `R`, weitere benannte Register, Vimscript, Neovim-Plugins und das vollständige Ex-Befehlssystem sind nicht enthalten.

## Multicursor

Mehrere Stellen lassen sich gemeinsam bearbeiten, auch bei eingeschaltetem Vim. **MULTI** und die Cursoranzahl erscheinen in der Statusleiste; der zuletzt hinzugefügte Cursor ist orange hervorgehoben. In diesem Modus gelten normale Editorbefehle. **Esc** beendet die Mehrfachbearbeitung am Hauptcursor; mit Vim geht es in den Normalmodus. Ein gewöhnlicher Klick beginnt wieder eine einzelne Auswahl.

| Aktion | Standardkürzel |
| --- | --- |
| Cursor hinzufügen oder vorhandenen entfernen | **Alt+Klick** |
| Rechteckauswahl über Bildschirmzeilen | **Alt+Shift+Ziehen** |
| Cursor in der logischen Zeile darüber / darunter | **Ctrl+Shift+↑ / ↓** |
| Wort auswählen, danach nächstes Vorkommen hinzufügen | **Ctrl+D** |
| Alle Vorkommen auswählen | **Ctrl+Shift+L** |
| Letztes Vorkommen überspringen und nächstes auswählen | **Ctrl+Shift+D** |
| Letzten Auswahlschritt zurücknehmen | **Ctrl+U** |
| Cursor an den Enden ausgewählter Zeilen | **Alt+Shift+I** |

Die Aktionen stehen auch unter **Selection**, im Kontextmenü und in der Befehlspalette. **Settings → Keyboard** ändert die Kürzel. Bestehende eigene Belegungen bleiben beim Upgrade erhalten; kollidierende neue Vorgaben starten unbelegt. Im Vim-Normalmodus aktiviert **Ctrl+D** die Mehrfachbearbeitung; **Ctrl+U** behält außerhalb von MULTI seine Vim-Funktion. **Ctrl+Alt+↑/↓** wechselt weiterhin den Fokus zwischen Bereichen.

Tippen, Löschen, Pfeile mit Shift/Ctrl, Home/End, Page Up/Down, Enter, Tab, Ein-/Ausrücken, Kommentare und Zeilenbefehle wirken auf alle Stellen. Überlappende Auswahlen verschmelzen; Zeilen mit mehreren Cursorn werden bei Zeilenbefehlen einmal bearbeitet. **Ctrl+Z/Y** macht die gemeinsame Änderung rückgängig beziehungsweise stellt sie samt Auswahlen wieder her. Cursor und Auswahlen bleiben je Split unabhängig; zusätzliche Cursor werden beim Neustart nicht wiederhergestellt.

**Ctrl+C/X** kopiert beziehungsweise schneidet die Auswahlen in Dokumentreihenfolge aus. Sind alle Cursor leer, gelten die betroffenen ganzen Zeilen. **Ctrl+V** verteilt eingefügten Text zeilenweise, wenn die Zeilenanzahl genau der Cursoranzahl entspricht; andernfalls erhält jeder Cursor den vollständigen Text. Unter **Settings → Editing → Multicursor** lässt sich die Verteilung ausschalten und **Ctrl+Klick** statt Alt wählen. Hinzu kommen Groß-/Kleinschreibung und Wortgrenzen für die Vorkommensauswahl sowie ein Cursorlimit von standardmäßig 1.000, einstellbar von 100 bis 10.000. Das Limit begrenzt die Auswahl, nicht den Dateiinhalt.

Vervollständigungen verwenden den Hauptcursor und werden nur übernommen, wenn der zu ersetzende Text an allen Stellen übereinstimmt; zusätzliche Sprachserver-Änderungen, etwa Imports, werden einmal angewendet. Bei widersprüchlichen Bereichen bleibt der Text unverändert. Eingabemethoden zeigen ihre laufende Komposition am Hauptcursor und übernehmen den bestätigten Text an allen Stellen.

## Markdown lesen

Markdown-Dateien bieten rechts in der Tab-Leiste ein Vorschau-Symbol. Es leuchtet, wenn die Vorschau aktiv ist; ein Klick oder **Ctrl+Alt+M** wechselt zwischen Quelltext und Vorschau. Der Quelltext bleibt ein normaler Vim-Editor; **Shift+V** wählt weiterhin ganze Zeilen aus.

Bei langen Vorschauen bleibt die vertikale Scrollleiste sichtbar. Breite Tabellen zeigen auch ohne Mausbewegung eine horizontale Scrollleiste; kurze Dokumente und passende Tabellen benötigen keine zusätzliche Leiste.

Die Vorschau zeigt Überschriften, Listen, Aufgaben, Tabellen, Zitate, Codeblöcke und Links. Text ist auswählbar; Codeblöcke lassen sich kopieren. Ein Doppelklick auf einen Block öffnet die passende Quelltextzeile. In der Vorschau scrollen `j` / `k`, **Ctrl+D** / **Ctrl+U**, `gg` und **Shift+G**. Bilder werden als Links angezeigt, HTML bleibt Text. Die Ansicht wird pro Datei beim Neustart wiederhergestellt.

## Geteilte Ansicht

**View → Split right** oder **Split down** teilt den Editor. Der neue Bereich beginnt mit der aktuellen Datei und verwaltet seine eigene Tab-Leiste. Dateien öffnen, **Ctrl+Tab**, **Ctrl+W**, **Close others** und **Close saved tabs** wirken auf diesen Bereich. Wenn sein letzter Tab geschlossen wird, verschwindet der Split; der letzte Editorbereich bleibt als leere Arbeitsfläche bestehen.

Jeder Bereich hat seine eigene Navigation und Scrollposition. Unterschiedliche Dateien können nebeneinander stehen; bei Markdown sind auch Quelltext und Vorschau derselben Datei möglich. Beide Ansichten bearbeiten denselben Text und teilen dessen Undo-Historie. Ein Tab schließt nur seine Ansicht: Ist die Datei noch in einem anderen Bereich offen, bleiben ungespeicherter Text und Undo erhalten. Die Trennlinie lässt sich ziehen. **Next region** und **Previous region** wechseln zwischen sichtbaren Seitenleisten, Editorbereichen und Output; **Ctrl+Alt** mit einer Pfeiltaste fokussiert den benachbarten Bereich in dieser Richtung. **Close pane** entfernt den Bereich und übernimmt seine sonst nirgends offenen Tabs in den verbleibenden Bereich.

Tabs lassen sich mit der Maus innerhalb einer Tab-Leiste umsortieren oder in einen anderen Bereich verschieben. Ein Einfügemarker zeigt die Position; auch die Dokumentfläche des Zielbereichs nimmt Tabs an. Wird der letzte Tab aus einem Bereich verschoben, schließt sich dieser Bereich. Der Text, ungespeicherte Änderungen und Undo bleiben erhalten. **Esc** oder Loslassen außerhalb eines Zielbereichs bricht das Ziehen ab.

## Mit Agenten arbeiten

**`dist/vEdit/vedit-cli.exe`** steuert das laufende Fenster und startet vEdit bei `open`, `review MANIFEST.json` oder `workspace`, falls es geschlossen ist. Befehle liefern JSON mit echten Tab-IDs, Revisionen, Ladezustand und Fehlern. Agenten können die offenen Dokumente lesen und bearbeiten, Ansichten anordnen und ihre Ergebnisse zeigen:

```powershell
.\dist\vEdit\vedit-cli.exe open "C:\Code\Projekt\plan.md" --preview
.\dist\vEdit\vedit-cli.exe review "C:\Code\Projekt\review.json"
.\dist\vEdit\vedit-cli.exe review --branch
.\dist\vEdit\vedit-cli.exe tabs
.\dist\vEdit\vedit-cli.exe focus 12 --line 42 --source
.\dist\vEdit\vedit-cli.exe bookmarks
.\dist\vEdit\vedit-cli.exe bookmarks toggle --tab 12 --label "Review this section"
.\dist\vEdit\vedit-cli.exe bookmarks jump 1
.\dist\vEdit\vedit-cli.exe read 12 --line 1 --count 50
.\dist\vEdit\vedit-cli.exe request ".\editor-action.json"
.\dist\vEdit\vedit-cli.exe --help
.\dist\vEdit\vedit-cli.exe read --help
.\dist\vEdit\vedit-cli.exe schema edit
```

`new [TITLE] --file TEXTDATEI` erstellt einen ungespeicherten Entwurf. `edit TAB_ID --file TEXTDATEI --revision N` ersetzt den gesamten Pufferinhalt durch den vorbereiteten Text; für gezielte Bereiche bleibt `request` verfügbar. `save-as TAB_ID PFAD --revision N` speichert unter einem noch nicht vorhandenen Ziel. Text- und JSON-Dateien akzeptieren UTF-8 mit oder ohne BOM sowie UTF-16 mit BOM; Zeilenenden bleiben im Text erhalten. In Windows PowerShell setzt `[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)` die Dekodierung der CLI-Ausgabe auf UTF-8.

`read` und `output` liefern bei weiteren Textteilen `next_line` und `next_column`; diese Werte gehen an `--line` und `--column`. Bei `byte_limit_reached:true` schließen die Textteile direkt aneinander an. Zwischen vollständigen `read`-Zeilenpaketen ist ein Zeilenumbruch einzufügen; `output` enthält seine Trennzeichen bereits. Beim Zusammensetzen eines Puffers muss die Revision gleich bleiben.

Tab- und Bereichs-IDs kommen aus den Antworten und gelten für den laufenden App-Prozess. Die oben gezeigte `12` ist ein Beispiel. `request` führt strukturierte JSON-Aktionen aus; `schema` zeigt deren aktuelle Formen und Vorgaben.

### Mehrere Projekte, Worktrees und Benachrichtigungen

Mehrere Agenten können gleichzeitig in verschiedenen Projekten oder Git-Worktrees arbeiten. Projektbefehle wie `task`, `output`, `search`, `launch`, `git-status` und `review --working` verwenden das mit `--workspace` genannte Projekt, sonst das offene Projekt, in dessen Ordner der Agent den Befehl ausführt, und erst danach das sichtbare Projekt. Ein in ein anderes Projekt verschachtelter Worktree hat dabei Vorrang. Jede Antwort nennt das verwendete Projekt; das sichtbare Projekt wechselt dadurch nicht. Jedes Projekt hat seine eigene Aufgabe und Ausgabe, der Bereich **Output** zeigt die des sichtbaren Projekts.

`open` verwendet bereits offene Dokumente wieder. Neue Dateien landen im tiefsten offenen Projekt, das sie enthält, oder in **Other files**; Repository und übergeordneter Ordner werden dabei nicht automatisch geöffnet. Mit `--background` bleiben bei `open` und `review` der sichtbare Arbeitsbereich, das aktuelle Dokument samt Cursor und der Tastaturfokus unverändert, und das Fenster kommt nicht nach vorn. Die Dateien warten als Tabs in ihrem Arbeitsbereich. Tabs in **Other files** melden `workspace:null` und `workspace_kind:"other_files"`; Dokumentbefehle verwenden weiterhin ihre Tab-ID.

`notify` meldet dem Benutzer, dass ein Agent fertig ist, nicht weiterkommt oder eine Entscheidung braucht:

```powershell
.\dist\vEdit\vedit-cli.exe notify "Alle Tests laufen; bitte die Änderungen ansehen." --title "Parser fertig" --level success --review working
```

Der Projekt-Tab erhält einen farbigen Punkt, neben **For agent** erscheint eine Glocke mit der Zahl ungelesener Meldungen, und bei einem Fenster im Hintergrund blinkt die Taskleistenschaltfläche. Ein Klick auf die Glocke (oder **View → Agent notifications**) zeigt die Meldungen mit Zeit, Projekt, Titel und Text; damit gelten sie als gelesen. Eine Meldung mit `--open PFAD:ZEILE` oder `--review working|branch` wechselt per Klick in ihr Projekt und öffnet die Datei beziehungsweise die Änderungsübersicht. **Clear all** leert die Liste. Wiederholte gleiche Meldungen werden zusammengefasst; die Liste gilt nur bis zum Beenden von vEdit. `status` meldet die ungelesenen Meldungen pro Projekt.

| Bereich | Verfügbare Kontrolle |
| --- | --- |
| Dokumente | Öffnen, neue Entwürfe, offenen Text lesen, Änderungen, Suchen/Ersetzen, Auswahl, Undo/Redo, Speichern und Save as |
| Dateien | Dateien und Ordner umbenennen, neue Ordner erstellen, Abschluss und Fehler der Hintergrundoperation abfragen |
| Ansichten | Fokus, Cursorposition, Source/Preview, Splits erstellen, Tabs sortieren oder zwischen Bereichen verschieben, Dokumente zuweisen, Bereiche fokussieren, schließen und skalieren |
| Editoraktionen | Einrücken, Kommentare umschalten, Zeilen duplizieren/löschen, Treffer wechseln, unveränderte Dateien neu laden |
| Einstellungen | Vim, Leader, Such- und Explorer-Filter, Autosave samt Verzögerung, UI-Zoom, Schrift, Zeilenabstand/-nummern, Cursorzeile, Einrückung, Umbruch und Scrollen lesen oder gezielt ändern |
| Bookmarks | Gespeicherte Stellen auflisten, setzen, benennen, entfernen und anspringen |
| Projektarbeit | Inhaltsuche, Git-Status, Branches und History lesen, Branch wechseln, Diagnosen, Diff zum gespeicherten Text, Build/Test/Check je Projekt starten oder stoppen, Ausgabe lesen |
| Benachrichtigungen | Meldungen mit Titel, Dringlichkeit und einer per Klick zu öffnenden Datei oder Änderungsübersicht |
| Präsentation | Mehrere Dateien mit Hinweisen öffnen, Review-Seitenleiste, Änderungsübersicht für Arbeitsverzeichnis, Commit oder Branch, Markierungen entfernen, Panels zeigen oder ausblenden |

Textänderungen verlangen `expected_revision` aus der vorherigen Antwort. Eine falsche Revision verändert den Text nicht. Mehrere nicht überlappende Edit-Bereiche beziehen sich auf dieselbe Ausgangsfassung und werden gemeinsam als Undo-Schritt angewendet. Edit- und Auswahlbereiche haben ein **exklusives Ende**; Review-Markierungen unten haben ein **inklusives Ende**. Agentenänderungen bleiben zunächst ungespeichert, auch bei eingeschaltetem Autosave. Erst ein explizites Speichern gibt sie für die Datei frei.

`search QUERY [--workspace PFAD]` übernimmt die gespeicherten Suchfilter. Wiederholtes `--exclude PATTERN` ersetzt sie für diese eine Suche, `--no-excludes` entfernt die optionalen Filter; `.gitignore` bleibt wirksam. Die JSON-Form bietet `excludes`, etwa `{"op":"search","query":"needle","excludes":[]}`. Die Antwort nennt die verwendeten Filter und deren Herkunft. Eine Suche im anderen Projekt verändert den sichtbaren Projektfokus nicht. Dauerhaft ändern lassen sich die Filter mit `settings` und `search_excludes`; `settings swap-jk true` tauscht die J/K-Richtung. Weggelassene Einstellungen bleiben unverändert.

`batch` führt bekannte Aktionen in Reihenfolge aus und stoppt standardmäßig beim ersten Fehler. Bereits ausgeführte Schritte werden nicht zurückgerollt; ihre Ergebnisse stehen in der Antwort. Neu erhaltene IDs oder Revisionen müssen ausgelesen werden, bevor abhängige Schritte gesendet werden.

Ein Review-Manifest kann mehrere Dateien und mehrere Hinweise je Datei enthalten. Pfade im Manifest beziehen sich auf dessen Ordner. Zeilen und Unicode-Zeichenspalten beginnen bei 1; ein angegebenes Ende ist inklusive. Ohne Ende wird die ganze Startzeile markiert.

```json
{
  "title": "Plan und wichtige Aenderungen",
  "note": "Bitte zuerst den Plan lesen.",
  "files": [
    { "path": "plan.md", "view": "preview" },
    {
      "path": "src/main.rs",
      "annotations": [
        {
          "start": { "line": 42 },
          "note": "Hier beginnt die neue Behandlung.",
          "severity": "info"
        }
      ]
    }
  ]
}
```

Die Seitenleiste **Review** zeigt die Dateien und Hinweise mit ausgeschriebener Dringlichkeit (**Info**, **Warning**, **Critical**). Ein Klick springt zur betreffenden Stelle; Markierungen stören die Vim-Auswahl nicht. Nach eigenen Änderungen werden veraltete Markierungen ausgeblendet und ein Hinweis angezeigt. **Clear** entfernt die Hinweise und lässt die Tabs offen.

`open` und `review` erhalten ungespeicherten Text. Unveränderte offene Dateien werden dabei im Hintergrund vom Datenträger aktualisiert. `close` lehnt geänderte oder noch speichernde Tabs ab. Öffnen, Laden und Speichern sind getrennte Zustände; eine erfolgreiche Anfrage allein bedeutet nicht, dass eine Datei bereits geladen oder geschrieben wurde. Die Verbindung bleibt lokal und ist pro Sitzung authentifiziert.

Der Agenten-Skill gehört zum Repository und liegt unter [.agents/skills/vedit/SKILL.md](.agents/skills/vedit/SKILL.md). Die portable Anwendung enthält eine Kopie unter `skills/vedit/`. Er erklärt die Bedienung und wie der Agent prüft, ob die angeforderten Änderungen und Dokumente tatsächlich bereit sind; eine persönliche Installation ist nicht erforderlich.

## Speichern und Wiederherstellung

**Ctrl+S** speichert die aktive Datei, **Ctrl+Shift+S** alle geänderten Dateien. Neue Dateien und **File → Save as…** öffnen den Windows-Speicherdialog, auch außerhalb des Projekts. Eine vorhandene Zieldatei wird erst nach der Windows-Bestätigung ersetzt; spätere Änderungen auf dem Datenträger bleiben durch die Konfliktprüfung geschützt. Eine Datei, die bereits in einem anderen Tab offen ist, lässt sich nicht als Ziel überschreiben. Abbrechen erhält den ungespeicherten Text.

Automatisches Speichern schreibt benannte Dateien standardmäßig nach **850 ms Schreibpause**. In Settings lässt sich die Verzögerung auf **300–5000 ms** ändern oder Autosave ausschalten. Wiederherstellungskopien werden unabhängig davon weiterhin nach 850 ms für geänderte Dateien und neue Tabs geschrieben, auch für neu angelegte Agent-Entwürfe ohne weitere Eingabe. Sie liegen unter `%APPDATA%\vEdit\recovery` und werden beim nächsten Start eingelesen. Text, ursprünglicher Dateistand und Zeilenumbruchformat werden zusammen gesichert. Externe Änderungen nach einem Absturz werden durch die wiederhergestellten Edits nicht überschrieben; bei einem Konflikt erhält **Save as…** beide Fassungen. Ältere Wiederherstellungskopien ohne verlässlichen Vergleichsstand bleiben lesbar und schützen die ursprüngliche Datei ebenfalls. Vorübergehend nicht verfügbare Projekte behalten ihre wiederherstellbaren Entwürfe. Beim regulären Schließen fragt vEdit nach ungespeicherten Änderungen.

Jede Sitzung verwendet eigene Dateinamen für Wiederherstellungskopien, sodass eine spätere Sitzung mit derselben Prozessnummer keine fremde Kopie überschreibt. Nach dem Speichern, beim Schließen eines Tabs oder Projekts und wenn eine wiederhergestellte Kopie bereits dem Dateiinhalt entspricht, entfernt vEdit die Kopie im Hintergrund und versucht es bei einer vorübergehend gesperrten Datei einige Male erneut. Beim Start löscht vEdit verwaiste Kopien, die älter als 24 Stunden sind, zu keinem wiederhergestellten Tab gehören und von einer beendeten vEdit-Sitzung stammen. Unlesbare Kopien bleiben zur Prüfung erhalten, und ohne lesbare Sitzungsdatei löscht vEdit beim Start nichts.

Dateien werden über eine temporäre Datei im selben Ordner atomar gespeichert. Bleibt eine solche `.vedit-save-…tmp`-Datei nach einem Absturz zurück, entfernt vEdit sie beim nächsten Speichern in diesem Ordner, sofern die zugehörige vEdit-Sitzung beendet ist; jeder Ordner wird dafür einmal pro Sitzung geprüft. Eine über einen Symlink oder eine Junction geöffnete Datei behält beim Speichern den geöffneten Pfad. Änderungen auf dem Datenträger werden vor dem Ersetzen geprüft; ein Konflikt überschreibt die externe Version nicht. **Reload file from disk** in der Befehlspalette lädt eine unveränderte Datei neu.

Ändert ein Agent oder ein anderes Programm eine offene Datei ohne ungespeicherte Änderungen, übernimmt vEdit die neue Fassung innerhalb von etwa zwei Sekunden als **einen rückgängig machbaren Schritt**: **Ctrl+Z** (in Vim `u`) stellt den vorherigen Text wieder her. Cursor, Auswahl und sichtbarer Ausschnitt jeder Ansicht bleiben beim selben Text, auch wenn darüber Zeilen eingefügt oder gelöscht wurden. Die Statusleiste meldet kurz etwa „Reloaded plan.md from disk: 12 lines changed · Ctrl+Z restores“ und zeigt bis zur nächsten Eingabe **Reloaded**. Bei sehr großen Dateien (zusammen über 4 MiB alter und neuer Text) behalten die Ansichten Zeile und Spalte, ohne Abgleich der Zeilen; auch dann macht Undo die Aktualisierung rückgängig, solange der Schritt in der normalen Undo-Historie liegt.

Hat die Datei ungespeicherte Änderungen, bleibt der Editorinhalt unverändert, und über dem Editor erscheint ein Hinweis mit vier Möglichkeiten:

- **Compare** zeigt die Festplattenfassung und den eigenen Stand als Diff; **Esc** schließt die Ansicht.
- **Use disk version** übernimmt die Festplattenfassung als einen Undo-Schritt; **Ctrl+Z** holt die eigenen Änderungen zurück.
- **Keep my version** behält die eigenen Änderungen. Die Festplattenfassung gilt danach als Vergleichsstand, sodass das nächste Speichern sie ersetzt. Autosave läuft wieder, wenn es eingeschaltet ist und es sich nicht um einen Agent-Entwurf handelt.
- **Save as…** sichert den eigenen Stand in einer anderen Datei; die Festplattenfassung bleibt unverändert.

Bis zur Entscheidung ist nur das Speichern in die ursprüngliche Datei gesperrt; Bearbeiten, Suchen, Sprünge und Lesezeichen funktionieren weiter. Dieselbe Auswahl erscheint, wenn ein Speichern auf eine inzwischen geänderte Datei trifft oder wiederhergestellte Änderungen nach einem Absturz auf eine geänderte Datei treffen.

**Compare with saved** (Befehlspalette, **File**-Menü oder Tab-Kontextmenü) zeigt die ungespeicherten Änderungen der aktiven Datei gegenüber dem gespeicherten Stand. Unterschiede nur im Zeilenende gelten dabei nicht als Änderung. Neue, noch nie gespeicherte Dateien haben keinen gespeicherten Stand; der Befehl ist für sie deaktiviert. Ein Agent-Entwurf endet, sobald die Datei keine ungespeicherten Änderungen mehr hat, etwa nach Undo bis zum gespeicherten Stand. Wird eine Datei mit ungespeicherten Änderungen in den Papierkorb verschoben, bleibt ihr Inhalt als unbenannter Tab erhalten; eigene Änderungen werden dabei nicht zum Agent-Entwurf.

Unterstützt werden **UTF-8-Textdateien bis 8 MiB**, einschließlich UTF-8-BOM und LF-, CRLF-, CR- sowie gemischter Zeilenenden. **Save as…** erhält gemischte Zeilenenden anhand der unveränderten Originaldatei, unabhängig vom bisherigen Inhalt des Ziels. Ist das Original inzwischen geändert oder nicht mehr verfügbar, wird die Kopie mit LF gespeichert; ein Statushinweis erklärt den Wechsel. Binärdateien und andere Textkodierungen werden beim Öffnen mit einer Meldung abgelehnt. Wiederherstellungskopien ergänzen das Speichern und ersetzen keine Projekt-Backups.

## Sprachen und Projektbefehle

**Python, Dart und Rust** haben Diagnosen, Vervollständigung und Navigation zur Definition. Jede Sprache erhält im Projekt ihre eigene Analysesitzung; auch gemischte Projekte funktionieren. **Ctrl+Space** fordert Vorschläge an, **F12** beziehungsweise Vim `gd` springt zur Definition. Die Statusleiste zeigt den Zustand der aktiven Sprache. **Problems** (**Ctrl+Shift+M**) enthält Fehler, Warnungen und Serverzustände. **Restart language analysis** verbindet die aktuelle Sprache neu, ohne eigene Änderungen zu verwerfen.

Statusmeldungen unten links lassen sich anklicken: Ein scrollbares Detailfenster zeigt den vollständigen gespeicherten Text, erlaubt Textauswahl und **Ctrl+C** und bietet **Copy all**. **Rechtsklick → Copy message** kopiert die Meldung direkt einschließlich Zeilenumbrüchen. Der Tooltip zeigt nur eine kurze Vorschau. Bereits geöffnete Details behalten ihre Meldung, auch wenn die Statusleiste inzwischen eine neue zeigt. Sprachserverfehler sind außerdem in **Problems → Error details…** erreichbar. Bei einem beendeten Sprachserver stehen Exitstatus, Projekt und eine erkannte Panic vor der letzten Serverausgabe (bis zu 16 KiB); diese Ausgabe bleibt nur für die laufende Sitzung erhalten.

Die Rust-Analyse schließt den Unterordner `.tmp` des Projekts automatisch aus, damit kurzlebige Testdateien nicht mitanalysiert werden. Das gilt auch für vorhandene Profile; eigene Rust-Ausschlüsse bleiben erhalten.

**Mit der Maus** etwa eine halbe Sekunde über einem Bezeichner verweilen, um Typ und Dokumentation zu sehen; dieselbe Anzeige öffnen Vim `K` und die Palettenaktion **Show hover** an der Cursorposition. Diagnosen an dieser Stelle stehen oben, darunter folgt die Dokumentation mit Markdown und farbigen Codeblöcken. Lange Inhalte scrollen innerhalb der Anzeige; die Maus kann dafür auf die Anzeige wechseln. Sie schließt, sobald die Maus Wort und Anzeige verlässt, der Cursor sich bewegt oder **Esc** gedrückt wird; der Tastaturfokus bleibt im Editor.

**Shift+F12** beziehungsweise Vim `gr` findet alle Referenzen des Symbols unter dem Cursor. Die Ergebnisse erscheinen in der Such-Seitenleiste, nach Dateien gruppiert, die aktuelle Datei zuerst, jeweils mit Zeile und Vorschau. **↑↓** und **Enter** oder ein Klick öffnen die Stelle und heben sie kurz hervor. Sehr viele Treffer werden begrenzt und als solche gekennzeichnet.

In **Quick open** (**Ctrl+P**) listet ein vorangestelltes `@` die Symbole der aktuellen Datei, mit Art und umgebendem Element; weitere Zeichen filtern unscharf, etwa `@prs` für `parse`. Die Aktion **Go to symbol in file** öffnet diese Liste direkt. `#` (**Ctrl+T**, **Go to symbol in workspace**) sucht Symbole im gesamten Projekt; dafür ist ein Suchbegriff nötig.

Nicht jeder Sprachserver bietet alle Funktionen. Fehlt eine, meldet die Statusleiste das, etwa **Rust analysis does not provide references.**; **Problems** bleibt davon unberührt. **Restart language analysis** bricht laufende Anfragen ab. Antworten, die erst nach einer Textänderung oder einem Neustart eintreffen, werden verworfen statt an veralteter Stelle angezeigt.

**Problems** ordnet nach Schweregrad, dann nach Pfad und Zeile. **Error** erscheint rot, **Warning** orange, **Information** blau und **Hint** gedämpft. Hints zählen nicht zur Problemzahl in Statusleiste und Tab und erhalten keinen Punkt am Zeilenrand; der Tooltip der Statusleiste nennt die Anzahl je Schweregrad. Die Spalte zählt Zeichen wie die Statusleiste. Für nicht geöffnete Dateien steht dort die Spalte des Sprachservers mit dem Zusatz **UTF-16 col**, weil Emoji und manche Schriftzeichen dort doppelt zählen. Die Liste zeigt höchstens 2.000 Einträge, die schwersten zuerst, und nennt dann die Gesamtzahl. **F8** / **Shift+F8** beziehungsweise Vim `]d` / `[d` springen zum nächsten oder vorherigen Fehler beziehungsweise zur Warnung: zuerst in der aktiven Datei, danach in der nächsten Datei mit Problemen; am Ende beginnt die Runde von vorn. Information und Hints werden dabei übersprungen.

Die portable Ausgabe bringt **basedpyright und Node** für Python mit. Ein vorhandener Projekt- oder PATH-Sprachserver wird bevorzugt. Unter **Settings → Tasks** lässt sich der Python-Interpreter wählen; automatisch werden `.venv`, `venv`, `env` und anschließend PATH berücksichtigt. Derselbe Interpreter gilt für Ausführung und Analyse. Dart verwendet das installierte Dart- oder Flutter-SDK. Rust verwendet `rust-analyzer` (`rustup component add rust-analyzer rust-src`).

**Settings → Languages** bietet aktivierbare Vorgaben und zusätzliche eigene Sprachserver: Dateiendungen, Sprache, Programm, separate Argumente und bei Bedarf JSON-Einstellungen. Serveränderungen werden mit **Apply and restart language** übernommen. Dateien bleiben auch ohne Server normal editierbar. Syntaxfarben, Dateityperkennung und Kommentare erkennen insbesondere auch `.pyi`, `.pyw` und Großschreibung; Dart besitzt eine eigene Syntaxgrammatik.

**Settings → Editing** steuert Einrückung. Neue Python-Dateien verwenden vier, Dart-Dateien zwei Leerzeichen. Vorhandene Einrückung wird automatisch erkannt; diese Erkennung lässt sich ausschalten. Breite und Tab-Verwendung sind je Sprache überschreibbar. Die Statusleiste zeigt die tatsächlich verwendete Einstellung und öffnet sie per Klick. Einstellungen ändern zukünftige Eingaben; vorhandener Text wird nicht automatisch umformatiert.

**Run → Run active file** (**Ctrl+F5**), **Check project** (**F5**), **Run tests** (**Alt+F6**) und **Build project** passen sich der aktiven Sprache an. Python verwendet den gewählten Interpreter, Dart das SDK, Rust Cargo. Unter **Settings → Tasks** lassen sich Profil und Befehle pro Projekt überschreiben; andere Sprachen können dort eigene Befehle erhalten. Python-Tests verwenden standardmäßig pytest, Python-Builds das Modul build. Vor dem Start müssen betroffene Dokumente gespeichert sein; vEdit zeigt offene Speicherarbeit an und bietet **Save all files**. Agent-Entwürfe werden dabei nicht stillschweigend freigegeben. Jedes Projekt führt seine Aufgabe unabhängig aus; während in einem Projekt Tests laufen, kann ein anderes bauen. **Output** zeigt jeweils die Aufgabe des sichtbaren Projekts.

Fundstellen in **Output** und **Application output** sind anklickbar: etwa `--> src/main.rs:10:5` von Cargo, rustc und Clippy, `tests/test_x.py:12:` von pytest, `File "C:\…\x.py", line 12` aus Python-Tracebacks, `lib/x.dart:12:3` von Dart und `src/x.ts(12,3)` von TypeScript. Beim Überfahren wird die Stelle unterstrichen und der Mauszeiger zur Hand; ein Klick öffnet Datei, Zeile und Spalte im aktiven Editorbereich, und **Alt+←** führt zurück. Relative Pfade gelten zuerst ab Projektwurzel, danach ab dem Arbeitsordner des Befehls beziehungsweise der Startoption. Ziehen markiert weiterhin Text zum Kopieren. Fehlt die Datei, meldet das die Statusleiste.

## Anwendungen starten

Ein eingerichtetes Projekt zeigt unter den Projekttabs **▶ Run** und eine Auswahlliste seiner Startoptionen. Wähle etwa **Web UI**, **Desktop** oder **Headless** und starte mit **Run**. Die Auswahl wird lokal für das Projekt gespeichert. Es muss keine Quelldatei geöffnet sein; ungespeicherte Projektdateien müssen vor dem Start gespeichert werden. **Run → Run application** und **Stop application** sind auch über die Befehlspalette erreichbar und lassen sich in den Tastatureinstellungen belegen. Die bisherigen Kürzel für Prüfen und Ausführen einer Datei bleiben erhalten.

**Application output** zeigt den Zustand, die Ausgabe und nach dem Ende den Exitcode. **Stop** beendet die Anwendung einschließlich ihrer gestarteten Unterprozesse. Pro Projekt läuft eine Variante; andere Projekte und Build-/Testaufgaben können gleichzeitig laufen. Anwendungen haben kein Zehn-Minuten-Limit. Beim Schließen ihres Projekts oder des Editors werden ihre Prozesse beendet. Die Ausgabe behält die letzten 1 MiB beziehungsweise etwa 10.000 Zeilen; Zeilennummern für `launch output` bleiben dabei gültig. Ein laufender Prozess bedeutet noch nicht, dass beispielsweise der Webserver schon bereit ist.

Bitte einen Agenten beispielsweise: „Richte Web UI und Headless als Startoptionen in vEdit ein.“ Der mitgelieferte Skill beschreibt das Ermitteln der Startwege, Schreiben benötigter Skripte und Prüfen von Start, Stop und erneutem Start. Über `vedit-cli launch configure --file CONFIG.json --workspace PROJEKT` hinterlegt er die Optionen revisionsgeschützt in `.vedit/launch.json`. `launch list`, `select`, `start`, `stop` und `output` bedienen dieselben Projektstarts. Die Datei und relative Startskripte können mit dem Projekt versioniert werden; Änderungen lädt vEdit automatisch nach. Ein Projekt wird beim Öffnen nicht automatisch gestartet.

Jede Option enthält einen Namen, eine ID, Programm und separate Argumente sowie optional Arbeitsordner und Umgebungsvariablen. Relative Programm- und Arbeitsordnerpfade beziehen sich auf die Projektwurzel; reine Programmnamen werden über PATH gefunden. Für Skripte wird der Interpreter ausdrücklich angegeben. Startskripte bleiben bis zum Ende ihrer Anwendung aktiv und reichen Fehler weiter. Eingabeaufforderungen benötigen weiterhin das interaktive Terminal. Bereits laufende Dienste und außerhalb des Prozessbaums verwaltete Dienste, etwa Container, werden von Stop nicht beendet.

## Git und Befehle

Ein Repository direkt im geöffneten Projektordner wird automatisch verwendet, auch bei Git-Worktrees. Liegt das Repository in einem übergeordneten Ordner, zeigt Source control dessen Namen und Pfad sowie **Use parent repository**. Erst nach dieser Auswahl erscheinen dessen Änderungen, Verlauf und Git-Aktionen; auch die Änderungsmarkierung im Projekt-Tab bleibt bis dahin aus. Die Auswahl gilt für dieses Projekt und wird mit der Sitzung gespeichert. **Using parent repository** zeigt den verwendeten Pfad dauerhaft; **Stop using parent repository** im Repository-Menü hebt die Auswahl wieder auf. Aktionen und Anzeigen beziehen sich auf das gesamte ausgewählte Repository.

Der obere Projektwähler (**+** beziehungsweise **Ctrl+Alt+P**) zeigt neben den offenen Projekten auch noch ungeöffnete Worktrees des aktiven Repositorys. Einträge mit **worktree** lassen sich per Klick oder mit Pfeiltasten und **Enter** als Projekt öffnen; bereits offene Worktrees erscheinen nur einmal als Projekt. Die Suche berücksichtigt Namen, Pfad und Branch. Branch, uncommittete Änderungen und **↑**/**↓** stehen beim Worktree, vollständiger Pfad, letzter Commit und Details im Tooltip. Die Zeilenzahlen werden beim Überfahren im Hintergrund geladen und bis zu 30 Sekunden zwischengespeichert; sie summieren vorgemerkte und nicht vorgemerkte Änderungen einschließlich neuer Dateien auf dem Datenträger. Ungespeicherte Editoränderungen zählen erst nach dem Speichern. Fehlende Worktree-Ordner und Bare-Repositories sind gekennzeichnet und lassen sich nicht als Arbeitsverzeichnis öffnen. **Copy path** funktioniert auch für ungeöffnete Worktrees; **Close project** gilt nur für offene Projekte. Bei mehreren Arbeitsbäumen zeigt **Source control** zusätzlich oben ein kompaktes Dropdown mit **Main worktree** zuerst und den zugehörigen Worktrees darunter. Die Einträge nennen Name, Branch und Änderungsstatus. Die Auswahl wechselt den gesamten Arbeitsbereich einschließlich Dateien und Git-Ansicht; ungespeicherte Änderungen bleiben erhalten. Bereits offene Worktrees werden wiederverwendet. Worktree-Listen und der Status ungeöffneter Worktrees werden bei sichtbarem Source control oder geöffnetem Projektwähler im aktiven Fenster nachgeladen, höchstens alle 15 beziehungsweise 30 Sekunden ohne ausdrückliche Aktualisierung. Der Tooltip eines Projekt-Tabs nennt weiterhin Branch und **↑**/**↓**.

Oben stehen nur zwei kompakte Zeilen, weil Commits und Pushes meist von Agents kommen und der Überblick wichtiger ist als Bedienknöpfe. Die erste zeigt den Branch und rechts daneben den Abstand zum Tracking-Branch, etwa **↑3 ↓1** (3 Commits zu pushen, 1 zu pullen), **Up to date** oder **No upstream**. Die zweite zeigt die Zahl geänderter Dateien, hinzugefügte und entfernte Zeilen in Grün/Rot und, falls abweichend, den Abstand zum lokalen `main` (ersatzweise `master`), etwa **main ↑12 ↓2**. Die Tooltips nennen den Tracking-Branch, die Aufteilung in gestagte, ungestagte und unversionierte Dateien sowie den Stand des letzten Fetch. Eine teilweise gestagte Datei zählt insgesamt einmal, aber in beiden passenden Gruppen. Die Pfeile zählen Commits, keine uncommitteten Änderungen. **Fetch**, **Pull** und **Push** stehen im **⋯**-Menü des Panels; Fetch aktualisiert den Remote-Stand, ohne lokale Dateien zu ändern. Die Änderungen, eine einzeilige, mitwachsende Commit-Nachricht und der Verlauf stehen darunter. Die Änderungen sind in **Staged**, **Unstaged** und **Untracked** gruppiert; die Schaltflächen **+**/**−** zum Stagen erscheinen beim Überfahren, an der ausgewählten Zeile oder bei Tastaturfokus. **Commit** wird erst orange, wenn gestagte Dateien und eine Nachricht vorliegen. Sind keine Dateiänderungen oder ungespeicherten Dokumente vorhanden und kennt Git ausstehende Commits zum Tracking-Branch, erscheint stattdessen **Push N commits** ohne Nachrichtenfeld. Eine vorhandene Commit-Nachricht bleibt erhalten und erscheint bei neuen Änderungen wieder. Push bleibt zusätzlich im **⋯**-Menü verfügbar.

Git betrachtet den gespeicherten Dateistand. Bei ungespeicherten Dokumenten zeigt Source control deren Anzahl und **Save all files**; auch offene Ordner desselben Repositorys werden berücksichtigt. Nach dem Speichern müssen die gewünschten Änderungen weiterhin gestaged werden. Speichern löst keinen Commit aus.

Automatische Git-Abfragen laufen ohne Ladeanzeige und ohne Bedienelemente zu sperren. Bis ein neues Ergebnis vorliegt, bleibt der zuletzt gelesene Stand sichtbar. Ausdrücklich gestartete Git-Aktionen zeigen Fortschritt und Fehler in der Statusleiste; während dieser Aktionen sind weitere Git-Aktionen gesperrt. Die History nutzt die verfügbare Höhe bis zum unteren Rand und lädt ältere Einträge ohne zusätzliche Ladezeile nach. Ihre Scrollbar ist wie im Explorer schmal und verbreitert sich beim Überfahren mit dezenter Hover-Farbe. Die Beschriftungen der Commit- und Push-Buttons sind zentriert.

Lesende Git-Abfragen brechen nach 15 Sekunden ab. Aktionen, die das Repository verändern (Stage, Commit, Branch-Wechsel, Fetch, Pull, Push), haben kein kurzes Zeitlimit, damit langsame Hooks, große Checkouts oder Netzwerkzugriffe fertig werden; erst nach 30 Minuten stoppt vEdit sie. Solange eine Aktion läuft, stehen in der Statusleiste und in Source control ein Fortschrittshinweis und **Cancel**. **Cancel** beendet Git samt Hooks und gestarteten Hilfsprogrammen; das Repository kann danach teilweise aktualisiert sein. Nach normalem Ende von Git laufen eigenständige Hilfsprogramme wie ein fsmonitor-Dienst weiter; nur Programme, die nach zwei Sekunden noch die Ausgabe von Git offen halten, werden beendet. Bleibt nach einem Abbruch oder Fehler eine `index.lock`-Datei zurück (auch im Verwaltungsordner eines Worktrees), nennt die Meldung ihren Pfad. vEdit löscht sie nicht, weil ein anderes Git-Programm sie noch verwenden könnte; arbeitet keines mehr im Repository, kann sie von Hand gelöscht werden. Während Pull oder Branch-Wechsel Dateien im Arbeitsverzeichnis ersetzen, wartet Autosave für Dateien dieses Repositorys, und ausdrückliches Speichern sowie Speichern durch Agents (Fehlercode `git_checkout_running`) wird abgelehnt; der Text bleibt offen und kann danach gespeichert werden. Git-Einstellungen wie `log.showSignature`, Farben, Pfad-Quoting oder Diff-Präfixe verändern die von vEdit gelesenen Ausgaben nicht; Hooks, Signaturen beim Commit und Zugangsdaten bleiben wirksam.

Liefert der Git-Status zu viele unversionierte Dateien (etwa ein Build-Ordner ohne `.gitignore`-Eintrag), liest vEdit ihn erneut mit zusammengefassten Ordnern: **Untracked** zeigt dann ganze Ordner wie `build/`, und ein Hinweis empfiehlt einen `.gitignore`-Eintrag. Geänderte und gestagte Dateien bleiben einzeln sichtbar.

**View → Source control** zeigt oben den Änderungsstatus, die Dateien und die Commit- beziehungsweise Push-Aktion, darunter **History**. Der Status nennt einmalig **No changes** oder die Anzahl geänderter Dateien; es gibt keine zusätzliche Changes-Überschrift und keinen Erklärungstext für eine leere Liste. Ohne eigene Aufteilung erhält der obere Bereich nur so viel Höhe, wie sein Inhalt braucht, höchstens die Hälfte. Ziehen an der horizontalen Trennlinie verteilt den Platz frei: **History** lässt sich bis knapp unter die Branch-Zeile hochziehen; dabei schrumpft der Änderungsbereich. Die Höhe wird pro Projekt gespeichert. Doppelklick setzt die Aufteilung zurück. Beide Bereiche scrollen unabhängig. Änderungen stehen als Ordnerbaum getrennt nach **Staged**, **Unstaged** und **Untracked**, mit hinzugefügten und entfernten Zeilen pro Datei. Eine teilweise gestagte Datei erscheint in beiden passenden Gruppen mit den jeweiligen Zahlen. Binärdateien werden als solche angezeigt; für große unversionierte Dateien können die Zahlen fehlen.

Ein Klick auf eine Änderung öffnet ihren Diff mit alten und neuen Zeilennummern und lesbarem Funktionskontext. Geänderter Code behält die normale Textfarbe; Hintergrund, Zeilennummern und das **+**/**−** am Zeilenanfang zeigen die Änderung. Lange Zeilen werden auf die Fensterbreite umbrochen; ihre Zeilennummern stehen nur an der ersten Teilzeile. Dateiinformationen und technische Patch-Details stehen getrennt im Kopfbereich; **Copy patch** kopiert den vollständigen Originalpatch. Das Kontextmenü bietet Datei öffnen, Pfad kopieren sowie Stage/Unstage; ganze Ordner lassen sich gemeinsam vorbereiten. Gestagte Änderungen werden mit einer Commit-Nachricht committed. Über den Branch-Namen oben lässt sich ein lokaler Branch auswählen oder aus einem Remote-Branch ein lokaler Tracking-Branch erstellen. Ungespeicherte Dokumente oder Git-Konflikte verhindern den Wechsel; Änderungen werden weder verworfen noch automatisch gestasht. Pull verwendet `--ff-only`; Push verwendet die vorhandene Git-Konfiguration ohne Force-Option. Zugangsdaten werden über die vorhandene Git-Konfiguration verwaltet.

**History** zeigt den Verlauf des ausgewählten Arbeitsbaums: den aktuellen Branch samt konfiguriertem Upstream und vollständiger Merge-Historie. Unabhängige Branches anderer Worktrees und deren Branch-Beschriftungen bleiben ausgeblendet. Bereits eingemergte Seitenzweige gehören weiterhin zum Verlauf. Ohne Branch zeigt sie die Historie von **Detached HEAD**, bei einem neuen Branch ohne Commit bleibt sie leer. Der kompakte Graph zeigt eine Zeile pro Commit: Nachricht, Autor und rechts den aktuellen Branch, seinen Upstream und Tags auf sichtbaren Commits. Der aktuelle Branch (blau) und sein Remote-Upstream (lila) haben gefüllte Badges; Tags sind nur umrandet. Passen nicht alle Badges, zeigt **+N** die übrigen. Die Hauptlinie des aktuellen Branches bleibt auf der linken Spur, auch wenn ein anderer Branch neuere Commits enthält. Seitenzweige rechts davon behalten ihre Spur und Farbe bis zum tatsächlichen gemeinsamen Commit; beim Nachladen bleiben bereits berechnete Verbindungen bestehen. Die Linie des aktuellen Branches ist blau, solange ihre Commits nur lokal existieren, und lila, sobald sie auf einem Remote-Branch liegen. Eingemergte Seitenzweige behalten eigene Farben. HEAD erscheint als großer Ring mit fetter Nachricht, Merge-Commits als Ring mit Punkt. Der Remote-Stand entspricht dem letzten **Fetch**. Im Hover stehen Datum und Uhrzeit mit Zeitzone sowie das relative Alter (etwa **12 minutes ago**), vollständige Metadaten, Branch-Bezeichnungen, die Anzahl geänderter Dateien und hinzugefügte/entfernte Zeilen in Grün/Rot. Diese Zahlen werden im Hintergrund geladen und zwischengespeichert; bei Merge-Commits beziehen sie sich auf den ersten Eltern-Commit. Ein Klick auf einen Commit öffnet Autor, Datum, vollständige Nachricht, Hash, Eltern und einen Dateibaum mit Zeilenzahlen. Beim Scrollen werden ältere Einträge automatisch nachgeladen. Bei sichtbarem Source control im aktiven, nicht minimierten Fenster wird alle drei Sekunden nach Abschluss der vorigen Abfrage auf Änderungen geprüft. Neue Commits, verschobene Branches und der Stand des letzten Fetch erscheinen automatisch; nur bei verändertem Verlauf wird die Commit-Liste neu geladen. Am Listenanfang erscheinen neue Commits direkt. Beim Lesen älterer Einträge bleiben Auswahl und Leseposition am bisherigen Commit erhalten, soweit dieser noch im geladenen Ausschnitt liegt. Bei ausgeblendetem Source control wird die History beim nächsten sichtbaren Prüfzyklus nachgezogen. Im Hintergrund oder minimiert pausieren die automatischen Git-Abfragen; laufende und ausdrücklich ausgelöste Aktionen dürfen fertig werden. Beim Aktivieren oder Wiederherstellen des Fensters startet sofort eine neue Abfrage, ohne auf das reguläre Intervall zu warten. Die Dateien in den Commit-Details öffnen den historischen Diff dieses Commits. Ein Klick außerhalb oder **Esc** schließt jeweils die oberste Ansicht. Alle Git-Abfragen laufen im Hintergrund. Commit-Details werden zwischengespeichert und große Diffs rendern nur ihre sichtbaren Zeilen.

**Änderungen durchsehen.** Die Änderungsübersicht zeigt alle Änderungen in einem durchgehenden Diff anstelle der Editorbereiche, etwa um die Arbeit eines Agenten am Stück zu lesen. Sie vergleicht wahlweise das Arbeitsverzeichnis (gestagte, ungestagte und unversionierte Änderungen) mit HEAD, einen Commit mit seinem ersten Eltern-Commit oder den aktuellen Branch seit seiner Abzweigung vom lokalen `main` (ersatzweise `master`). Geöffnet wird sie über **Review all changes** im **⋯**-Menü von Source control, einen Klick auf **main ↑N ↓M** im Überblick, **Review commit** im Kontextmenü eines History-Eintrags oder einen Doppelklick auf ihn sowie über die Palettenbefehle **Review working tree changes** und **Review branch against main**. Agenten zeigen sie mit `vedit-cli review --working`, `--commit REV` oder `--branch [--base REF]` an.

Die Kopfzeile nennt den Vergleich, die Zahl der Dateien, hinzugefügte und entfernte Zeilen in Grün/Rot und wie viele Dateien schon als gesehen markiert sind. Links steht die Dateiliste nach Ordnern, rechts der Diff. Der Kopf der aktuellen Datei bleibt beim Scrollen oben stehen, und die Liste hebt die aktuelle Datei hervor; ein Klick in der Liste springt zu ihr. **j**/**k** oder die Pfeiltasten scrollen (**Swap J and K** gilt auch hier), **n**/**p** oder **]**/**[** springen zur nächsten bzw. vorherigen Datei, die **Leertaste** markiert die aktuelle Datei als gesehen und springt zur nächsten ungesehenen, **Enter** oder **o** öffnet die Datei an der sichtbaren Zeile. **Esc** oder **×** schließt die Übersicht, und die vorherige Anordnung der Editorbereiche erscheint unverändert. Auch das Öffnen eines Dokuments schließt sie; beim erneuten Öffnen derselben Übersicht bleibt die Leseposition erhalten. Gesehen-Markierungen gelten pro Vergleich und Dateistand: Ändert sich eine Datei erneut, gilt sie wieder als ungesehen. Sie bleiben bis zum Beenden von vEdit erhalten, werden aber nicht in der Sitzung gespeichert. **Refresh** vergleicht neu und behält Markierungen und Position. Große Diffs lädt erst **Load**, Binärdateien erscheinen nur mit einem Hinweis. Die Übersicht ist schreibgeschützt und ändert weder Dateien noch den Index.

Für Rückmeldungen an einen Agenten lassen sich Diffzeilen auswählen: per Klick, mit **Shift**+Klick erweitern oder mit **Shift+↑/↓** von der sichtbaren Zeile aus. **c** oder **Comment for agent…** im Kontextmenü öffnet ein Notizfeld; **Ctrl+Enter** fügt den Kommentar hinzu, **Esc** bricht ab. Auch entfernte Zeilen und vollständig gelöschte Dateien lassen sich kommentieren. **For agent (N)** in der Diff-Kopfzeile öffnet die Sammlung direkt. Sie zeigt Notiz, Dateipfad, Vergleich und die Zeilenbereiche beider Fassungen als **Old … · New …**; **—** bedeutet, dass auf dieser Seite keine Zeile ausgewählt wurde. Im Tooltip stehen die damaligen Pfade und Vergleichsstände, einschließlich Umbenennungen. Diffausschnitt und Vergleichsdaten bleiben nach Refresh und Neustart erhalten, auch ohne geöffnete Datei. Agenten erhalten sie über `vedit-cli agent-marks`.

**Esc** im Diff hebt zuerst die Auswahl auf. Ändert sich das Repository nach dem Laden, erscheint in der Kopfzeile ruhig **Changes since this review was loaded** neben **Refresh**; die Übersicht lädt nie von selbst neu und behält die Leseposition. Ein Commit-Vergleich bleibt unverändert gültig. Ungespeicherte Dokumente im Repository fehlen in einem Vergleich des Arbeitsverzeichnisses; ein Hinweis nennt ihre Anzahl, und **Show** öffnet das jeweilige Dokument.

Auch im Diff-Fenster aus Source control wählt ein Klick eine Zeile; ein Doppelklick oder **Enter** öffnet die Datei an dieser Zeile, ohne Auswahl an der obersten sichtbaren Zeile.

Im Bereich **Output** lassen sich zusätzlich eigene Befehle im aktiven Projektordner starten. Sie verwenden die ausgewählte Python-Umgebung. Ausgabe und Rückgabestatus erscheinen dort; **Stop** beendet einen laufenden Befehl einschließlich seiner Unterprozesse. Eigene Befehle können `VEDIT_ROOT`, `VEDIT_FILE` und `VEDIT_PYTHON` als Umgebungsvariablen verwenden; Dateipfade werden nicht in Shellcode interpoliert.

Der Ausgabebereich ist ein Befehlsstarter mit Ausgabeanzeige. Interaktive Eingabe, Terminal-Anwendungen und Passwortabfragen sind dort nicht unterstützt. Befehle haben ein Zeitlimit von zehn Minuten; die Ausgabe wird begrenzt. **Run → Open terminal** öffnet für interaktive Arbeit ein separates PowerShell-Fenster im aktiven Projektordner. **Open terminal** ist ebenfalls in der Befehlspalette verfügbar. Im Ausgabebereich selbst stehen dafür **Terminal** und **Configure tasks** rechts in der Kopfzeile.

vEdit bietet derzeit keinen Debugger, VS-Code-Erweiterungsmarkt, Mehrfachcursor oder vollständigen VS-Code-Funktionsumfang.

## Prüfen

```powershell
cargo test --all-targets -- --test-threads=8
```

Die Tests decken Bearbeitung, Vim, Dateizugriffe, Wiederherstellung, Git, CLI, Befehlsausführung und die Oberfläche ab. SDK-abhängige Integrationstests und manuelle Benchmarks sind standardmäßig übersprungen. Beispielsweise lässt sich der Test mit einem echten rust-analyzer nach Installation der Rust-Komponenten explizit ausführen:

```powershell
cargo test live_rust_analyzer_diagnostics_completion_and_definition -- --ignored --nocapture
```

Build, gezielte Tests, Bildreferenzen und portable Pakete beschreibt die [Entwicklungsanleitung](docs/development.md). Repository-Agenten beginnen mit [AGENTS.md](AGENTS.md); die Bedienung einer laufenden vEdit-Instanz erklärt der separate [vEdit-Skill](.agents/skills/vedit/SKILL.md).
