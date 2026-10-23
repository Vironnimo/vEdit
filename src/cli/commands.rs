//! Translate a validated command and its options to the typed request.
use super::*;

pub(super) fn request(
    command: &str,
    positional: Vec<String>,
    options: Options,
) -> Result<Request, String> {
    let request = match command {
        "launch" => Request::Launch {
            workspace: options.workspace.as_ref().map(PathBuf::from),
            action: launch::parse(&positional, &options)?,
        },
        "status" => {
            require_empty(&positional)?;
            Request::Status {}
        }
        "raise" => {
            require_empty(&positional)?;
            Request::Raise {}
        }
        "tabs" => {
            require_empty(&positional)?;
            Request::Tabs {
                limit: options.limit.unwrap_or(100),
                offset: options.offset.unwrap_or(0),
            }
        }
        "open" => {
            if positional.is_empty() {
                return Err("open needs one or more paths".into());
            }
            let location = options.location()?;
            let files = positional
                .into_iter()
                .map(|path| OpenFile {
                    path: absolute(path.into()),
                    location: location.clone(),
                    view: options.view.unwrap_or_default(),
                    annotations: Vec::new(),
                })
                .collect();
            Request::Open {
                files,
                background: options.background,
            }
        }
        "review"
            if options.working
                || options.branch
                || options.commit.is_some()
                || options.base.is_some() =>
        {
            let source = match (options.working, options.commit.is_some(), options.branch) {
                (true, false, false) => ChangeReviewSource::Working,
                (false, true, false) => ChangeReviewSource::Commit,
                (false, false, true) => ChangeReviewSource::Branch,
                (false, false, false) => return Err("--base requires --branch".into()),
                _ => return Err("Choose one of --working, --commit REV or --branch".into()),
            };
            if positional.len() > 1 {
                return Err(
                    "review --working|--commit|--branch takes an optional WORKSPACE".into(),
                );
            }
            Request::ChangeReview {
                source,
                rev: options.commit.clone(),
                base: options.base.clone(),
                workspace: options.workspace_or(&positional)?,
                background: options.background,
            }
        }
        "review" => {
            if options.workspace.is_some() {
                return Err("--workspace applies to review --working|--commit|--branch; a manifest's files choose their projects".into());
            }
            let path = absolute(
                one_argument(&positional, "review needs exactly one manifest file")?.into(),
            );
            let contents = read_json_file(&path)?;
            let mut manifest: ReviewManifest =
                transport::from_unique_json(json_bytes(&contents))
                    .map_err(|e| format!("Invalid review manifest: {e}"))?;
            let base = path.parent().unwrap_or_else(|| std::path::Path::new("."));
            for file in &mut manifest.files {
                if !file.path.is_absolute() {
                    file.path = base.join(&file.path);
                }
            }
            Request::Review {
                title: manifest.title,
                note: manifest.note,
                files: manifest.files,
                background: options.background,
            }
        }
        "focus" => Request::Focus {
            tab_id: tab_id(one_argument(&positional, "focus needs exactly one tab ID")?)?,
            location: options.location()?,
            view: options.view,
        },
        "close" => Request::Close {
            tab_ids: ids(&positional, true)?,
        },
        "clear-marks" => Request::ClearMarks {
            tab_ids: ids(&positional, false)?,
        },
        "edit" => Request::Edit {
            tab_id: tab_id(one_argument(
                &positional,
                "edit needs TAB_ID --file TEXT_FILE --revision N",
            )?)?,
            expected_revision: options.required_revision()?,
            edits: Vec::new(),
            set_text: Some(read_input_file(&absolute(
                options
                    .file
                    .as_ref()
                    .ok_or("edit requires --file TEXT_FILE")?
                    .into(),
            ))?),
        },
        "save-as" => {
            if positional.len() != 2 {
                return Err("save-as needs TAB_ID PATH --revision N".into());
            }
            Request::SaveAs {
                tab_id: tab_id(&positional[0])?,
                expected_revision: options.required_revision()?,
                path: absolute((&positional[1]).into()),
            }
        }
        "save" => Request::Save {
            tab_ids: ids(&positional, true)?,
        },
        "workspace" => Request::Workspace {
            path: absolute(
                one_argument(&positional, "workspace needs exactly one directory")?.into(),
            ),
        },
        "panel" => Request::Panel {
            panel: serde_json::from_value(json!(one_argument(
                &positional,
                "panel needs exactly one panel name"
            )?))
            .map_err(|_| "Unknown panel; use --help")?,
            visible: !options.hide,
        },
        "read" => Request::Read {
            tab_id: tab_id(one_argument(&positional, "read needs exactly one tab ID")?)?,
            start_line: options.line.unwrap_or(1),
            start_column: options.column.unwrap_or(1),
            line_count: options.count.unwrap_or(100),
        },
        "selection" => Request::Selection {
            tab_id: tab_id(one_argument(&positional, "selection needs one tab ID")?)?,
            range: None,
            expected_revision: None,
        },
        "settings" => Request::Settings {
            patch: match positional.as_slice() {
                [] => None,
                [setting, value] if setting == "leader-key" => Some(SettingsPatch {
                    leader_key: Some(normalize_leader_key(value)?),
                    ..Default::default()
                }),
                [setting, value] if setting == "swap-jk" => Some(SettingsPatch {
                    swap_jk: Some(match value.as_str() {
                        "true" => true,
                        "false" => false,
                        _ => return Err("settings swap-jk requires true or false".into()),
                    }),
                    ..Default::default()
                }),
                [setting, value] if setting == "font" => Some(SettingsPatch {
                    editor_font: Some(serde_json::from_value(json!(value)).map_err(
                        |_| "settings font requires jetbrains-mono, consolas, or cascadia-mono",
                    )?),
                    ..Default::default()
                }),
                [path] => Some(
                    transport::from_unique_json::<SettingsPatch>(json_bytes(&read_json_file(
                        &absolute(path.into()),
                    )?))
                    .map_err(|e| format!("Invalid settings patch: {e}"))?,
                ),
                _ => {
                    return Err(
                        "settings takes PATCH.json, font jetbrains-mono|consolas|cascadia-mono, swap-jk true|false, or leader-key space|comma|backslash|CHAR".into(),
                    );
                }
            },
        },
        "bookmarks" => Request::Bookmarks {
            change: parse_bookmarks(&positional, &options)?,
        },
        "agent-marks" => Request::AgentMarks {
            action: parse_agent_marks(&positional, &options)?,
        },
        "command" => {
            if positional.len() != 2 {
                return Err("command needs TAB_ID COMMAND --revision N".into());
            }
            Request::Command {
                tab_id: tab_id(&positional[0])?,
                command: serde_json::from_value(json!(positional[1]))
                    .map_err(|_| "Unknown editor command; use --help")?,
                expected_revision: options.required_revision()?,
            }
        }
        "find" => {
            if positional.len() != 2 {
                return Err("find needs TAB_ID QUERY".into());
            }
            Request::Find {
                tab_id: tab_id(&positional[0])?,
                query: positional[1].clone(),
                case_sensitive: options.case_sensitive,
                whole_word: options.whole_word,
                limit: options.limit.unwrap_or(100),
                offset: options.offset.unwrap_or(0),
            }
        }
        "replace" => {
            if positional.len() != 3 {
                return Err("replace needs TAB_ID QUERY REPLACEMENT --revision N".into());
            }
            Request::Replace {
                tab_id: tab_id(&positional[0])?,
                expected_revision: options.required_revision()?,
                query: positional[1].clone(),
                replacement: positional[2].clone(),
                case_sensitive: options.case_sensitive,
                whole_word: options.whole_word,
                all: !options.first,
            }
        }
        "diff" => Request::Diff {
            tab_id: tab_id(one_argument(&positional, "diff needs one tab ID")?)?,
            max_lines: options.count.unwrap_or(500),
        },
        "diagnostics" => Request::Diagnostics {
            tab_id: tab_id(one_argument(&positional, "diagnostics needs one tab ID")?)?,
            limit: options.limit.unwrap_or(100),
            offset: options.offset.unwrap_or(0),
        },
        "search" => {
            if positional.is_empty() || positional.len() > 2 {
                return Err("search needs QUERY and optional WORKSPACE".into());
            }
            Request::Search {
                query: positional[0].clone(),
                workspace: options.workspace_or(&positional[1..])?,
                excludes: if options.no_excludes {
                    Some(Vec::new())
                } else {
                    options.excludes.clone()
                },
                case_sensitive: options.case_sensitive,
                whole_word: options.whole_word,
                limit: options.limit.unwrap_or(100),
            }
        }
        "output" => {
            require_empty(&positional)?;
            Request::Output {
                start_line: options.line.unwrap_or(1),
                start_column: options.column.unwrap_or(1),
                line_count: options.count.unwrap_or(100),
                workspace: options.workspace_or(&[])?,
            }
        }
        "git-status" | "git-branches" | "git-history" => {
            if positional.len() > 1 {
                return Err(format!("{command} takes an optional WORKSPACE"));
            }
            let workspace = options.workspace_or(&positional)?;
            let limit = options.limit.unwrap_or(100);
            let offset = options.offset.unwrap_or(0);
            match command {
                "git-branches" => Request::GitBranches {
                    workspace,
                    limit,
                    offset,
                },
                "git-history" => Request::GitHistory {
                    workspace,
                    limit,
                    offset,
                },
                _ => Request::GitStatus {
                    workspace,
                    limit,
                    offset,
                },
            }
        }
        "git-switch-branch" => {
            if !(1..=2).contains(&positional.len()) {
                return Err("git-switch-branch needs REF and an optional WORKSPACE; use an exact ref from git-branches".into());
            }
            Request::GitSwitchBranch {
                target: positional[0].clone(),
                workspace: options.workspace_or(&positional[1..])?,
            }
        }

        "new" => {
            if positional.len() > 1 {
                return Err("new takes an optional TITLE, --file TEXT_FILE and --path PATH".into());
            }
            Request::New {
                title: positional.first().cloned(),
                text: options
                    .file
                    .as_ref()
                    .map(|path| read_input_file(&absolute(path.into())))
                    .transpose()?
                    .unwrap_or_default(),
                path: options.path.as_ref().map(|path| absolute(path.into())),
            }
        }
        "task" => Request::Task {
            action: serde_json::from_value(json!(one_argument(
                &positional,
                "task needs check, test, build, run_file or stop"
            )?))
            .map_err(|_| "Unknown task; use --help")?,
            workspace: options.workspace_or(&[])?,
        },
        "notify" => {
            let message = match positional.as_slice() {
                [message] => message.clone(),
                [] => {
                    return Err(
                        "notify needs MESSAGE, e.g. notify \"Tests pass; ready for review\"".into(),
                    );
                }
                _ => return Err("notify takes one MESSAGE; quote text that contains spaces".into()),
            };
            Request::Notify {
                message,
                title: options.title.clone(),
                level: options.level.unwrap_or_default(),
                workspace: options.workspace_or(&[])?,
                open: options.open.as_deref().map(notification_open).transpose()?,
                review: options.review,
            }
        }
        "file" => Request::File {
            action: transport::from_unique_json::<FileAction>(json_bytes(&read_json_file(
                &absolute(one_argument(&positional, "file needs an ACTION.json file")?.into()),
            )?))
            .map_err(|error| format!("Invalid file action: {error}"))?,
        },
        "file-job" => Request::FileJob {
            job_id: one_argument(&positional, "file-job needs one returned job ID")?
                .parse::<u64>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or("File job ID must be a positive integer returned by file/status")?,
        },
        "layout" => Request::Layout {
            change: match positional.as_slice() {
                [] => LayoutAction::Status {},
                [path] => transport::from_unique_json::<LayoutAction>(json_bytes(&read_json_file(
                    &absolute(path.into()),
                )?))
                .map_err(|e| format!("Invalid layout change: {e}"))?,
                _ => return Err("layout takes an optional CHANGE.json".into()),
            },
        },
        "request" => {
            let input = one_argument(&positional, "request needs a JSON file or '-' for stdin")?;
            let contents = if input == "-" {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take(1_048_577)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                bytes
            } else {
                read_json_file(&absolute(input.into()))?
            };
            if contents.len() > 1_048_576 {
                return Err("Request exceeds 1 MiB".into());
            }
            let contents = decode_input(&contents)?;
            let mut request: Request = transport::from_unique_json(contents.as_bytes())
                .map_err(|e| format!("Invalid request: {e}"))?;
            resolve_paths(&mut request);
            request
        }
        _ => return Err(format!("Unknown command {command}; use --help")),
    };
    Ok(request)
}

fn parse_bookmarks(positional: &[String], options: &Options) -> Result<BookmarkAction, String> {
    let change = match positional {
        [] => BookmarkAction::List { limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0) },
        [action] if action == "list" => BookmarkAction::List { limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0) },
        [action] if action == "toggle" => BookmarkAction::Toggle {
            tab_id: options.tab,
            label: options.label.clone(),
        },
        [action, id] if action == "remove" => BookmarkAction::Remove { id: bookmark_id(id)? },
        [action, id] if action == "rename" => BookmarkAction::Rename {
            id: bookmark_id(id)?,
            label: options.label.clone().ok_or("bookmarks rename needs --label TEXT")?,
        },
        [action, id] if action == "jump" => BookmarkAction::Jump { id: bookmark_id(id)? },
        _ => return Err("bookmarks takes list, toggle [--tab ID] [--label TEXT], remove ID, rename ID --label TEXT, or jump ID".into()),
    };
    if options.tab.is_some() && !matches!(change, BookmarkAction::Toggle { .. }) {
        return Err("--tab is supported only by bookmarks toggle".into());
    }
    if options.label.is_some()
        && !matches!(
            change,
            BookmarkAction::Toggle { .. } | BookmarkAction::Rename { .. }
        )
    {
        return Err("--label is supported only by bookmarks toggle or rename".into());
    }
    if (options.limit.is_some() || options.offset.is_some())
        && !matches!(change, BookmarkAction::List { .. })
    {
        return Err("--limit and --offset are supported only by bookmarks list".into());
    }
    Ok(change)
}

/// `PATH` or `PATH:LINE`; a drive letter's colon is never read as a line.
fn notification_open(value: &str) -> Result<NotificationOpen, String> {
    let (path, location) = match value.rsplit_once(':') {
        Some((path, line))
            if !path.is_empty()
                && !line.is_empty()
                && line.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            let line = line
                .parse::<usize>()
                .ok()
                .filter(|line| *line > 0)
                .ok_or("--open LINE must be a positive integer, e.g. src/main.rs:42")?;
            (path, Some(Location { line, column: 1 }))
        }
        _ => (value, None),
    };
    if path.trim().is_empty() {
        return Err("--open needs PATH or PATH:LINE".into());
    }
    Ok(NotificationOpen {
        path: absolute(path.into()),
        location,
    })
}

fn bookmark_id(value: &str) -> Result<u64, String> {
    value
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "Bookmark ID must be a positive integer returned by bookmarks".into())
}

fn parse_agent_marks(positional: &[String], options: &Options) -> Result<AgentMarksAction, String> {
    let action = match positional {
        [] => AgentMarksAction::List {
            limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0),
        },
        [action] if action == "list" => AgentMarksAction::List {
            limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0),
        },
        [action] if action == "take" => AgentMarksAction::Take {
            request_id: options.request_id.clone(), limit: options.limit.unwrap_or(100),
        },
        [action] if action == "batch" => AgentMarksAction::Batch {
            batch_id: None, limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0),
        },
        [action, id] if action == "batch" => AgentMarksAction::Batch {
            batch_id: Some(agent_mark_id(id)?), limit: options.limit.unwrap_or(100), offset: options.offset.unwrap_or(0),
        },
        [action, id] if action == "text" => AgentMarksAction::Text {
            id: agent_mark_id(id)?, batch_id: options.batch,
            offset: options.offset.unwrap_or(0), count: options.count.unwrap_or(4096),
        },
        [action, id] if action == "ack" => AgentMarksAction::Ack { batch_id: agent_mark_id(id)? },
        [action] if action == "restore" => AgentMarksAction::Restore { batch_id: None },
        [action, id] if action == "restore" => AgentMarksAction::Restore { batch_id: Some(agent_mark_id(id)?) },
        [action] if action == "clear" => AgentMarksAction::Clear {},
        [action, id] if action == "remove" => AgentMarksAction::Remove { id: agent_mark_id(id)? },
        [action, id, note] if action == "note" => AgentMarksAction::Note { id: agent_mark_id(id)?, note: note.clone() },
        [action] if action == "add" => AgentMarksAction::Add { tab_id: options.tab, note: options.note.clone() },
        _ => return Err("agent-marks takes list, take, batch [BATCH_ID], text MARK_ID, ack BATCH_ID, restore [BATCH_ID], clear, remove MARK_ID, note MARK_ID TEXT, or add; use --help".into()),
    };
    for (present, allowed, flag) in [
        (
            options.limit.is_some(),
            matches!(
                action,
                AgentMarksAction::List { .. }
                    | AgentMarksAction::Take { .. }
                    | AgentMarksAction::Batch { .. }
            ),
            "--limit",
        ),
        (
            options.offset.is_some(),
            matches!(
                action,
                AgentMarksAction::List { .. }
                    | AgentMarksAction::Batch { .. }
                    | AgentMarksAction::Text { .. }
            ),
            "--offset",
        ),
        (
            options.count.is_some(),
            matches!(action, AgentMarksAction::Text { .. }),
            "--count",
        ),
        (
            options.batch.is_some(),
            matches!(action, AgentMarksAction::Text { .. }),
            "--batch",
        ),
        (
            options.request_id.is_some(),
            matches!(action, AgentMarksAction::Take { .. }),
            "--request-id",
        ),
        (
            options.tab.is_some(),
            matches!(action, AgentMarksAction::Add { .. }),
            "--tab",
        ),
        (
            options.note.is_some(),
            matches!(action, AgentMarksAction::Add { .. }),
            "--note",
        ),
    ] {
        if present && !allowed {
            return Err(format!(
                "{flag} is not supported by this agent-marks action"
            ));
        }
    }
    Ok(action)
}

pub(super) fn agent_mark_id(value: &str) -> Result<u64, String> {
    value.parse().ok().filter(|id| *id > 0).ok_or_else(|| {
        "Agent mark/batch ID must be a positive integer returned by agent-marks".into()
    })
}
