//! Command discovery and option ownership share this catalog with the parser.

pub struct CommandHelp {
    pub name: &'static str,
    pub usage: &'static str,
    pub summary: &'static str,
    pub details: &'static str,
    pub flags: &'static [&'static str],
}

macro_rules! command {
    ($name:literal, $usage:literal, $summary:literal, $details:literal, [$($flag:literal),*]) => {
        CommandHelp { name: $name, usage: $usage, summary: $summary, details: $details, flags: &[$($flag),*] }
    };
}

pub const COMMANDS: &[CommandHelp] = &[
    command!(
        "windows",
        "",
        "List running and saved editor windows without changing them.",
        "windows contains IDs, PIDs and recovery directories. saved_windows contains closed window IDs.\nTarget a running window with vedit-cli --window ID COMMAND. Restore a saved window with vedit --window ID.\nWithout --window, commands use the only running window; several windows require an explicit ID.\nThe --window ID prefix goes before COMMAND. IDs belong to windows; tab IDs remain local to each process.",
        []
    ),
    command!(
        "launch",
        "list | configure --file CONFIG.json | select ID | start [ID] | stop | output",
        "Configure, select and run named project applications; read their state/output.",
        "Definitions live in .vedit/launch.json: profiles:[{id,name,program,args,cwd,env}], optional default:ID. JSON: schema launch.\nconfigure replaces definitions atomically; preserve existing profiles. Omit --revision only to create; for updates use revision from list.\nlist reads definitions and runtime state. select remembers this machine's choice. start uses ID or the selected/default/first profile; no active file required.\nArguments are passed literally. Relative cwd/program paths use the project root; bare executable names use PATH. Environment inherits vEdit plus profile env and VEDIT_ROOT.\nstart requires saved project buffers, returns Starting, and keeps the process tree until it exits or stop completes; no runtime timeout. Each project has its own run. Check output.run.running/status/error; running is not application readiness.\nstop requests termination of this project's tree. output pages the retained log; line numbers count from the run's first line and stay valid while old lines are trimmed (first_line, skipped_lines). Launches have closed stdin; use Run > Open terminal for interactive programs.",
        [
            "--workspace",
            "--file",
            "--revision",
            "--line",
            "--column",
            "--count"
        ]
    ),
    command!(
        "open",
        "PATH...",
        "Open files in the window; preserves unsaved text.",
        "Clean open buffers refresh from disk. loading:true is pending; inspect tabs until loading:false and error:null.\nOmit view flags for source. Omit --line to preserve the cursor or reveal the first annotation.\nReuses an existing buffer. Otherwise opens in the deepest open project containing the file, or Other files without opening a folder. Tabs in Other files have workspace:null and workspace_kind:other_files.\nRelative paths use the working directory. Starts the sibling vedit.exe if needed, except with --background.",
        [
            "--line",
            "--column",
            "--preview",
            "--source",
            "--background"
        ]
    ),
    command!(
        "review",
        "MANIFEST.json | --working|--commit REV|--branch [--base REF] [--workspace PATH]",
        "Replace review annotations, or show the user a change review.",
        "Manifest: title, optional note, files:[{path,view,location,annotations}]. Relative file paths use the manifest directory.\nAnnotations: start:{line,column}, optional inclusive end, note, severity:info|warning|critical. Omit end for the whole start line.\nReturns per-file outcomes and tab IDs; file loads can remain pending. Unsaved buffers stay intact.\nStarts the sibling vedit.exe if needed. JSON examples: vedit-cli schema review.\nWith a source flag, shows every change as one read-only diff in place of the editor panes, for the user to read and tick off. The project must be open with a repository. Returns workspace, title, file count, line totals and visible once loaded. Never starts vEdit. JSON: schema change-review.",
        [
            "--working",
            "--commit",
            "--branch",
            "--base",
            "--workspace",
            "--background"
        ]
    ),
    command!(
        "status",
        "",
        "Read session, projects, pending work and visible layout.",
        "Includes a bounded tab page, file jobs, dialogs and task state. Use tabs for further pages.\nTab/pane IDs last for this app process. Each project and Other files keeps its own layout; active_workspace is the visible folder path or null. active_workspace_kind is project or other_files; workspaces lists folder projects.",
        []
    ),
    command!(
        "tabs",
        "",
        "Read open buffers across projects, with IDs and revisions.",
        "Returns items, workspace, dirty/loading/saving/error/disk_conflict state and next_offset. Use actual IDs in subsequent commands.\ndisk_conflict:true: the file changed on disk while the buffer has unsaved edits; read/edit work, save waits for the user.",
        ["--limit", "--offset"]
    ),
    command!(
        "focus",
        "TAB_ID",
        "Reveal a tab in its project's window layout.",
        "Omit view flags to retain this pane's source/preview choice. Omit --line to retain its cursor.\nReturns the focused tab; a file load can remain pending.",
        ["--line", "--column", "--preview", "--source"]
    ),
    command!(
        "close",
        "TAB_ID...",
        "Close clean documents in every pane; refuses dirty/saving tabs.",
        "Returns per-tab outcomes. Successful closes stay applied if another tab fails.\nFor one pane's tab use layout close_tab through a JSON change file.",
        []
    ),
    command!(
        "clear-marks",
        "[TAB_ID...]",
        "Remove agent-authored review annotations; keeps documents open.",
        "Omit IDs to clear all review annotations and the review summary. User agent-marks remain intact.",
        []
    ),
    command!(
        "workspace",
        "DIRECTORY",
        "Add/select a project and restore its saved layout.",
        "Returns the selected workspace and indexing state. Requires an existing directory.\nStarts the sibling vedit.exe if needed.",
        []
    ),
    command!(
        "panel",
        "NAME",
        "Show or hide a panel in the window.",
        "NAME: explorer, search, source_control, review, left_sidebar, right_sidebar, output, problems, settings.\nSidebar targets preserve content assignments. Content targets select their saved side.",
        ["--hide"]
    ),
    command!(
        "read",
        "TAB_ID",
        "Read in-memory text, including unsaved edits, with its revision.",
        "Returns text and next_line/next_column for continuation, including inside long lines.\nKeep the same revision across pages; if it changes, reread the affected text before editing.\nLine pages omit the separator after their last complete line; join those pages with a newline. Byte continuations concatenate directly.",
        ["--line", "--column", "--count"]
    ),
    command!(
        "edit",
        "TAB_ID --file TEXT_FILE --revision N",
        "Replace the complete buffer with file text as an unsaved Undo step.",
        "Use only when the full desired text is known; for range edits use request and schema edit.\nReturns the resulting tab/revision; a revision conflict changes nothing. Autosave is paused until an explicit save.\nInput accepts UTF-8 (with/without BOM) or UTF-16 with BOM, at most 1 MiB encoded/decoded; line endings and payload are preserved.",
        ["--file", "--revision"]
    ),
    command!(
        "save",
        "TAB_ID...",
        "Save named buffers with external-change protection.",
        "Returns per-tab outcomes. saving:true is pending; require saving:false, dirty:false and error:null in tabs before claiming disk completion.\nUntitled buffers need save-as. Successful saves release agent drafts for autosave.\nError disk_conflict: the file changed on disk after loading and nothing was written. Keep the buffer with save-as, or ask the user to choose a version in the editor.",
        []
    ),
    command!(
        "save-as",
        "TAB_ID PATH --revision N",
        "Save a buffer to a nonexistent destination and update its path.",
        "Refuses an existing destination or stale revision. saving:true is pending; require saving:false, dirty:false and error:null.\nThe source determines encoding and line endings; PATH resolves against the working directory.",
        ["--revision"]
    ),
    command!(
        "new",
        "[TITLE]",
        "Create an unsaved draft and return its tab ID/revision.",
        "Creation does not write a file. Omit TITLE for the editor's default title. Named drafts use the containing open project or Other files. Untitled drafts use the visible area, or Other files when none is open.\nText files accept UTF-8 (with/without BOM) or UTF-16 with BOM, at most 1 MiB encoded/decoded. Omit --file for an empty draft.",
        ["--file", "--path"]
    ),
    command!(
        "selection",
        "TAB_ID",
        "Read selected text, range, revision and Undo state.",
        "Text can be truncated. Read the buffer for more. To set a range use request with expected_revision (schema selection).\nCoordinates are 1-based Unicode scalar positions; selection ends are exclusive.",
        []
    ),
    command!(
        "settings",
        "[PATCH.json] | leader-key VALUE | swap-jk true|false | font NAME",
        "Read preferences or persist a supplied patch.",
        "Omit arguments to read settings and the effective keybinding catalog. PATCH.json changes supplied fields; null preserves them.\nleader-key: space, comma, backslash or one printable character. font: jetbrains-mono, consolas, cascadia-mono.\nKeybinding/indentation maps replace their whole override map: merge existing entries when retaining them.\nmulticursor patches merge supplied fields: modifier alt|ctrl, spread_paste/match_case/whole_word booleans, limit 100..10000.\nReturns applied settings; invalid patches change nothing. Full fields: vedit-cli schema settings.",
        []
    ),
    command!(
        "bookmarks",
        "[list] | toggle | remove ID | jump ID | rename ID --label TEXT",
        "Read or change persistent bookmarked locations; leaves text intact.",
        "Omit action for list. list accepts --limit/--offset and returns next_offset.\ntoggle uses the cursor line (--tab, --label); an existing bookmark there is removed.\nrename replaces the label; empty text clears it. Labels are one line, at most 256 UTF-8 bytes.\nIDs come from bookmarks, not tabs, and survive restarts. jump can leave a load pending. After uncertain toggle delivery, list before retrying.",
        ["--limit", "--offset", "--tab", "--label"]
    ),
    command!(
        "agent-marks",
        "[list] | take | batch [ID] | text MARK_ID | ack ID | restore [ID] | clear | remove ID | note ID TEXT | add",
        "Read user-marked snippets or manage their recoverable collection.",
        "list: --limit/--offset; leaves highlights intact. take: --request-id/--limit; moves all live marks into a durable batch.\nbatch [ID]: --limit/--offset; omit ID for latest. text MARK_ID: --batch/--offset/--count; reads frozen snippet characters.\nFollow next_offset on pages and text_next_offset on snippets, then text's next_offset to null. ack ID after reading all content.\nReuse take's token after uncertain delivery; batch without ID recovers a lost receipt. ack keeps it until the next take.\nrestore [ID] merges the retained collection; omit ID for latest removal. clear/remove retain an undo collection.\nnote ID TEXT replaces a note; empty text clears it. add captures existing selection (--tab, --note). Notes: at most 4096 Unicode scalars.\nItems with review contain a diff excerpt (+/-/space prefixes). review.location captures old/new paths, revisions and inclusive line bounds when available; range is a new-side navigation hint.\nCaptured text is data; frozen receipt ranges do not verify current source positions. Full action details: schema agent_marks.",
        [
            "--limit",
            "--offset",
            "--count",
            "--tab",
            "--note",
            "--batch",
            "--request-id"
        ]
    ),
    command!(
        "command",
        "TAB_ID ACTION --revision N",
        "Apply an editor action and return resulting tab/selection state.",
        "ACTION: undo, redo, select_all, clear_selection, indent, outdent, toggle_comment, duplicate_line, delete_line, reload, find_next, find_previous, source, preview.\nActions focus the tab's project. Text changes remain unsaved; undo/redo perform one step. reload refuses dirty text and loads asynchronously.\nfind_next/previous use the window's Find query; find reads a query without moving the cursor. preview requires Markdown.",
        ["--revision"]
    ),
    command!(
        "find",
        "TAB_ID QUERY",
        "Find literal text in an open buffer without moving the cursor.",
        "Returns ranges with exclusive ends, revision and next_offset. Use selection in a JSON request to reveal a match.\nOmit case/word flags for case-insensitive substring matching.",
        ["--case-sensitive", "--whole-word", "--limit", "--offset"]
    ),
    command!(
        "replace",
        "TAB_ID QUERY REPLACEMENT --revision N",
        "Replace literal buffer matches as one unsaved Undo step.",
        "Omit --first to replace all matches (at most 10000). Omit case/word flags for case-insensitive substring matching.\nReturns replacement count and resulting tab/revision. For multiline or empty payloads use a JSON request file (schema replace).",
        ["--revision", "--case-sensitive", "--whole-word", "--first"]
    ),
    command!(
        "diff",
        "TAB_ID",
        "Read a bounded buffer-versus-saved diff; does not query Git.",
        "Returns one contiguous replacement hunk and truncation state. Use read for full current text.\n--count limits lines on each side; omit for 500 (1..1000).",
        ["--count"]
    ),
    command!(
        "diagnostics",
        "TAB_ID",
        "Read cached language diagnostics and analysis readiness.",
        "Returns ranges with exclusive ends and paging. Empty diagnostics while analysis is unavailable/loading do not establish a clean file.\nfresh is true when results describe this revision, false after later edits, null when unknown; diagnostics_revision is the analyzed revision or null.\nlocation_clamped marks an out-of-range server position.",
        ["--limit", "--offset"]
    ),
    command!(
        "search",
        "QUERY [WORKSPACE]",
        "Search saved workspace files in a bounded background walk.",
        "Searching never changes the visible project. The result names its workspace.\nOmit filter flags to inherit saved exclusions; --exclude replaces them, --no-excludes clears optional filters.\n.gitignore, VCS directories and symlink guards still apply. Unsaved text is not searched; use find for that.\nReturns actual filters, files searched, warnings and truncation. No continuation page: narrow a truncated query.",
        [
            "--case-sensitive",
            "--whole-word",
            "--limit",
            "--exclude",
            "--no-excludes",
            "--workspace"
        ]
    ),
    command!(
        "output",
        "",
        "Read a project's bounded task output and running/status state.",
        "Each project keeps its own task output. Returns workspace, text and next_line/next_column, including inside long lines; concatenate returned text directly.\nOutput is a current snapshot. Task dispatch is not completion; require running:false and inspect status for its exit result.",
        ["--line", "--column", "--count", "--workspace"]
    ),
    command!(
        "git-status",
        "[WORKSPACE]",
        "Read cached Git branch, changed files and cache readiness.",
        "Returns workspace, next_offset, loading, cache age and errors; does not force a refresh.\nGit reads saved files, not agent drafts.",
        ["--limit", "--offset", "--workspace"]
    ),
    command!(
        "git-branches",
        "[WORKSPACE]",
        "Read local and fetched remote branches with exact refs.",
        "Returns workspace, next_offset and refs usable by git-switch-branch.\nDoes not fetch; remote state reflects the last fetch.",
        ["--limit", "--offset", "--workspace"]
    ),
    command!(
        "git-history",
        "[WORKSPACE]",
        "Read the editor's Git history with hashes and remote reachability.",
        "Returns workspace, next_offset, date/time, Unix timestamp and on_remote.\nUses the visible history snapshot when available; remote state reflects the last fetch.",
        ["--limit", "--offset", "--workspace"]
    ),
    command!(
        "git-switch-branch",
        "REF [WORKSPACE]",
        "Switch Git branch and wait for its resulting state.",
        "Use the exact refs/heads/... or refs/remotes/... value from git-branches.\nA remote ref creates a local tracking branch. Unsaved documents and Git conflicts block switching. No force or automatic stash.",
        ["--workspace"]
    ),
    command!(
        "task",
        "check|test|build|run_file|stop",
        "Start a configured project task or request cancellation.",
        "Each project runs one task at a time, with its own output; the result names the workspace. run_file uses that project's active document.\nRequires saved buffers and no task running in the project; does not save them itself.\nReturns dispatch state. Inspect output (same --workspace) for running:false and the exit result; stop can still be pending.\nConfigure unavailable commands in Settings > Tasks in the window.",
        ["--workspace"]
    ),
    command!(
        "notify",
        "MESSAGE",
        "Tell the user something in the window's agent inbox; changes nothing else.",
        "Use it when work is finished, blocked or needs a decision. The project's tab gets a dot and the toolbar an unread count; an unfocused window flashes its taskbar button.\nThe message never switches projects or takes focus. An identical unread message counts as a repeat (repeat_count). Messages last until vEdit exits.\nThe project is --workspace, else the open project containing the current directory, else the one containing --open, else the visible one.\nMESSAGE: 1..2000 bytes; quote it. Returns id, workspace, unread counts and flashed. Never starts vEdit.",
        ["--title", "--level", "--workspace", "--open", "--review"]
    ),
    command!(
        "file",
        "ACTION.json",
        "Start a tracked rename, folder creation or Recycle Bin deletion.",
        "ACTION.json uses action plus fields: rename (path, name), create_folder (parent, name), delete (path). Exact JSON examples: schema file.\nPaths must belong to a registered workspace; names are basenames. No overwrite or permanent-delete fallback.\nReturns job_id with pending:true, not filesystem completion. Inspect file-job until complete:true, applied:true and error:null.\nDeletion refuses unsaved/busy documents; restore deleted items through the Windows Recycle Bin.",
        []
    ),
    command!(
        "file-job",
        "JOB_ID",
        "Read a file operation's completion, destination and error.",
        "Use the job_id from file or status. A successful query can report a failed job; inspect applied and error.\nJobs expire after app restart or retention eviction. After uncertain delivery inspect status.file_jobs and the target.",
        []
    ),
    command!(
        "layout",
        "[CHANGE.json]",
        "Read active-project panes or apply a layout change.",
        "Omit the file to read layout. JSON actions: split, assign, focus, move_tab, resize, close_tab, close.\nvertical means side by side; horizontal means top/bottom. Use returned pane IDs and tabs from the active project.\nclose_tab removes one local reference; close transfers unrepresented documents to a surviving pane.\nReturns the updated layout; use that result. Exact action shapes: vedit-cli schema layout.",
        []
    ),
    command!(
        "raise",
        "",
        "Bring the running editor window to the front.",
        "Does not edit or save documents.",
        []
    ),
    command!(
        "request",
        "JSON_FILE | -",
        "Execute one structured action, including ordered batches.",
        "Reads a file; '-' explicitly reads stdin. UTF-8 with/without BOM and UTF-16 with BOM are accepted, at most 1 MiB encoded/decoded.\nUse files in PowerShell to preserve multiline/non-ASCII text. Relative request paths use the working directory.\nUnknown/duplicate fields are rejected. Batch stops on failure by default and does not roll back successful actions.\nSearch, file and asynchronous Git requests run outside batches. Exact shapes: vedit-cli schema OP.",
        []
    ),
    command!(
        "schema",
        "[OP]",
        "Print JSON request examples and rules without contacting the app.",
        "Omit OP for the full catalog. OP uses JSON spelling, e.g. edit, save_as, agent_marks, batch; hyphens are accepted too.\nExamples contain illustrative IDs/paths: substitute values from the task and current responses.",
        []
    ),
];

pub fn command(name: &str) -> Option<&'static CommandHelp> {
    COMMANDS.iter().find(|entry| entry.name == name)
}

pub fn takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--line"
            | "--workspace"
            | "--column"
            | "--limit"
            | "--offset"
            | "--count"
            | "--revision"
            | "--tab"
            | "--label"
            | "--note"
            | "--batch"
            | "--request-id"
            | "--exclude"
            | "--file"
            | "--path"
            | "--commit"
            | "--base"
            | "--title"
            | "--level"
            | "--open"
            | "--review"
    )
}

pub fn render(name: Option<&str>) -> String {
    let mut text = String::new();
    if let Some(entry) = name.and_then(command) {
        text.push_str(&format!(
            "{}\nUsage: vedit-cli {} {}\n\n{}\n",
            entry.summary, entry.name, entry.usage, entry.details
        ));
        for flag in entry.flags {
            let description = match *flag {
                "--workspace" if entry.name == "notify" => {
                    "PATH: open project the message is about. Omit to choose as described above."
                }
                "--workspace" => {
                    "PATH: open project to target. Omit for the open project containing the current directory, else the visible project; never switches projects."
                }
                "--file" if entry.name == "launch" => {
                    "CONFIG.json: launch definitions. Required for configure; accepts UTF-8 or UTF-16 with BOM."
                }
                "--revision" if entry.name == "launch" => {
                    "N: configuration revision from list/configure. Omit for 0, which creates only if absent."
                }
                "--line" => {
                    "N: first line, 1-based. Omit for line 1 when reading; navigation preserves position."
                }
                "--column" => {
                    "N: Unicode scalar column on --line, 1-based. Omit for column 1; requires --line."
                }
                "--count" if entry.name == "diff" => continue,
                "--count" if entry.name == "agent-marks" => {
                    "N: text chunk size in Unicode scalars, 1..65536. Omit for 4096."
                }
                "--count" => "N: number of lines, 1..500. Omit for 100.",
                "--limit" if entry.name == "tabs" => "N: page size, 1..100. Omit for 100.",
                "--limit" => "N: result/page size, 1..500. Omit for 100.",
                "--offset" if entry.name == "agent-marks" => {
                    "N: zero-based item index, or scalar index for text. Omit for 0; use the returned continuation."
                }
                "--offset" => {
                    "N: zero-based item index. Omit for 0; continue with returned next_offset."
                }
                "--revision" => {
                    "N: required current revision from read/tabs or a mutation result; nonnegative integer."
                }
                "--preview" => "Display Markdown preview; conflicts with --source.",
                "--source" => "Display editable source; conflicts with --preview.",
                "--hide" => "Hide the named panel. Omit to show it.",
                "--file" => {
                    "TEXT_FILE: complete input text file. Required for edit; omit for an empty new draft."
                }
                "--path" => {
                    "PATH: nonexistent destination for a new draft; the draft joins the project that will contain it. Omit for untitled; does not write it."
                }
                "--case-sensitive" => {
                    "Match exact letter case. Omit for case-insensitive matching."
                }
                "--whole-word" => "Match complete words. Omit for substring matching.",
                "--first" => "Replace the first document match. Omit to replace all matches.",
                "--exclude" => {
                    "PATTERN: repeat for replacement folder/glob filters; conflicts with --no-excludes."
                }
                "--no-excludes" => {
                    "Clear optional filters for this search. Omit both filter flags to inherit settings."
                }
                "--tab" if entry.name == "agent-marks" => {
                    "ID: tab whose selection add captures. Omit for the active document; only for add."
                }
                "--tab" => {
                    "ID: tab whose cursor line toggle uses. Omit for the active document; only for toggle."
                }
                "--label" => {
                    "TEXT: bookmark label. Required for rename; omit for toggle's default label."
                }
                "--note" => "TEXT: note for add. Omit for no note.",
                "--batch" => "ID: retained batch for text. Omit to read the live mark.",
                "--request-id" => {
                    "TOKEN: take retry identity, 1..128 UTF-8 bytes. Omit to generate one; reuse only for the same take."
                }
                "--working" => "Review staged, unstaged and untracked changes against HEAD.",
                "--commit" => {
                    "REV: review one commit against its first parent, e.g. HEAD or a hash."
                }
                "--branch" => {
                    "Review the current branch since it left local main, or master without main."
                }
                "--base" => "REF: compare --branch with this branch or revision instead.",
                "--background" if entry.name == "review" => {
                    "Keep the user's visible project, panes and keyboard. Omit to show the result now. Another project shows it when the user switches there; a change review for the visible project waits for the user's next review command."
                }
                "--background" => {
                    "Keep the user's visible project, document and keyboard; the window is not raised. Omit to show the files now. A document the user is viewing keeps its cursor."
                }
                "--title" => {
                    "TEXT: one-line heading, at most 120 bytes. Omit to head the entry with the message."
                }
                "--level" => {
                    "info|success|warning|error: color and order of the entry. Omit for info."
                }
                "--open" => {
                    "PATH[:LINE]: file the entry opens when the user clicks it. Omit for no file."
                }
                "--review" => {
                    "working|branch: change review the entry opens when clicked; conflicts with --open. Omit for none."
                }
                _ => unreachable!("undocumented flag"),
            };
            text.push_str(&format!("  {flag} {description}\n"));
        }
    } else {
        text.push_str("vEdit CLI — control the editor; responses are JSON.\nUsage: vedit-cli COMMAND [arguments]\n\n");
        for entry in COMMANDS {
            text.push_str(&format!("  {:18} {}\n", entry.name, entry.summary));
        }
        text.push_str("\nUse vedit-cli COMMAND --help for arguments; schema OP for structured request examples.\n");
    }
    text.push_str("\nWindow selection: vedit-cli --window ID COMMAND; omit for the only running window. List IDs with windows.\nGlobal: --no-start prevents startup; -- ends options for literal arguments. --help is offline.\nResults: one JSON document on stdout; ok/result/error. Exit 0: applied/read; 1: failure; 2: partial execution.\nExit 0 does not prove pending loads, saves, jobs or tasks finished. Inspect reported pending/error state.\n");
    text
}
