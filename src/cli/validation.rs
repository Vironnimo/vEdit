//! Request invariants shared by local parsing and the GUI receiver.
use super::*;

fn validate_indent_languages<'a>(
    languages: impl Iterator<Item = &'a str>,
    count: usize,
) -> Result<(), String> {
    if count > 64 {
        return Err("Indentation language maps support at most 64 entries".into());
    }
    for language in languages {
        if language.is_empty()
            || language.len() > 32
            || !language
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err("Indentation language keys must be lowercase language IDs (1..32 ASCII letters, digits or underscores), for example python, dart, javascript or shellscript".into());
        }
    }
    Ok(())
}

/// Accepted aliases denote exactly one key; never infer a nearby key spelling.
pub fn normalize_leader_key(value: &str) -> Result<String, String> {
    let normalized = match value {
        "space" => " ",
        "comma" => ",",
        "backslash" => "\\",
        _ => value,
    };
    let mut chars = normalized.chars();
    match (chars.next(), chars.next()) {
        (Some(key), None) if !key.is_control() => Ok(normalized.to_owned()),
        _ => Err("leader_key needs one printable character or space|comma|backslash".into()),
    }
}
/// Compile bounded gitignore-style rules before a request can change settings.
/// This pure helper is also used by the workspace search worker.
pub fn normalize_search_excludes(values: &[String]) -> Result<Vec<String>, String> {
    if values.len() > 128 {
        return Err("search_excludes/excludes accepts at most 128 folder or glob rules".into());
    }
    if values.iter().map(String::len).sum::<usize>() > 16 * 1024 {
        return Err("search_excludes/excludes rules exceed 16 KiB in total".into());
    }
    let mut builder = ignore::gitignore::GitignoreBuilder::new(".");
    builder.allow_unclosed_class(false);
    builder
        .case_insensitive(cfg!(windows))
        .map_err(|error| error.to_string())?;
    let mut normalized = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if value.len() > 1024 {
            return Err(format!("Exclude rule {} exceeds 1024 bytes", index + 1));
        }
        let rule = value.trim().replace('\\', "/");
        if rule.is_empty() {
            continue;
        }
        if rule.starts_with('!') {
            return Err(format!(
                "Exclude rule {} uses unsupported ! negation; remove it or use excludes:[] for this search",
                index + 1
            ));
        }
        if rule.chars().any(char::is_control) {
            return Err(format!(
                "Exclude rule {} must be a single printable folder or glob pattern",
                index + 1
            ));
        }
        let literal_hash = rule.starts_with('#').then(|| format!("\\{rule}"));
        builder.add_line(None, literal_hash.as_deref().unwrap_or(&rule))
            .map_err(|error| format!("Invalid exclude rule {}: {error}; correct the folder/glob pattern before retrying", index + 1))?;
        if !normalized.contains(&rule) {
            normalized.push(rule);
        }
    }
    builder
        .build()
        .map_err(|error| format!("Invalid exclude rules: {error}"))?;
    Ok(normalized)
}
pub fn validate(request: &Request) -> Result<(), String> {
    match request {
        Request::Launch { workspace, action } => {
            if let Some(path) = workspace {
                validate_path(path)?;
            }
            match action {
                launch::Action::Configure { configuration, .. } => configuration.validate()?,
                launch::Action::Select { id } | launch::Action::Start { id: Some(id) } => {
                    launch::validate_id(id)?
                }
                launch::Action::Output {
                    start_line,
                    start_column,
                    line_count,
                } if *start_line == 0 || *start_column == 0 || !(1..=500).contains(line_count) => {
                    return Err("Launch output uses 1-based positions and 1..500 lines.".into());
                }
                _ => {}
            }
        }
        Request::Open { files, .. } | Request::Review { files, .. } => {
            if files.is_empty() || files.len() > MAX_FILES {
                return Err(format!("Supply 1..{MAX_FILES} files"));
            }
            let mut total = 0;
            for file in files {
                if file.path.as_os_str().is_empty() || !file.path.is_absolute() {
                    return Err("File paths must be absolute; CLI resolves relative paths".into());
                }
                if file.view == DocumentView::Preview && !is_markdown(&file.path) {
                    return Err(format!(
                        "Preview requires Markdown: {}",
                        file.path.display()
                    ));
                }
                if let Some(location) = &file.location {
                    validate_location(location)?;
                }
                total += file.annotations.len();
                let mut ids = std::collections::HashSet::new();
                for annotation in &file.annotations {
                    validate_location(&annotation.start)?;
                    if let Some(end) = &annotation.end {
                        validate_location(end)?;
                        if (end.line, end.column) < (annotation.start.line, annotation.start.column)
                        {
                            return Err("Annotation end must not precede its start".into());
                        }
                    }
                    if annotation.note.len() > 16_384 || annotation.id.len() > 256 {
                        return Err("Annotation note/ID exceeds its size limit".into());
                    }
                    if !annotation.id.is_empty() && !ids.insert(&annotation.id) {
                        return Err("Duplicate annotation ID within one file".into());
                    }
                }
            }
            if total > MAX_ANNOTATIONS {
                return Err(format!(
                    "A batch supports at most {MAX_ANNOTATIONS} annotations"
                ));
            }
            if let Request::Review { title, note, .. } = request
                && (title.trim().is_empty() || title.len() > 256 || note.len() > 16_384)
            {
                return Err(
                    "Review needs a nonempty title (max 256 bytes); note max 16 KiB".into(),
                );
            }
        }
        Request::Focus {
            tab_id, location, ..
        } => {
            validate_id(*tab_id)?;
            if let Some(location) = location {
                validate_location(location)?;
            }
        }
        Request::Close { tab_ids } | Request::Save { tab_ids } => {
            if tab_ids.is_empty() {
                return Err("Supply one or more tab IDs".into());
            }
            validate_ids(tab_ids)?;
        }
        Request::ClearMarks { tab_ids } => validate_ids(tab_ids)?,
        Request::Tabs { limit, .. } => {
            if *limit == 0 || *limit > 100 {
                return Err("tabs limit must be 1..100".into());
            }
        }
        Request::Read {
            tab_id,
            start_line,
            line_count,
            start_column,
        } => {
            validate_id(*tab_id)?;
            if *start_line == 0 || *start_column == 0 || *line_count == 0 || *line_count > 500 {
                return Err("read uses positive start_line/start_column and 1..500 lines".into());
            }
        }
        Request::Workspace { path } if !path.is_absolute() => {
            return Err("Workspace path must be absolute".into());
        }
        Request::New { title, text, path } => {
            if title
                .as_ref()
                .is_some_and(|t| t.trim().is_empty() || t.len() > 256)
            {
                return Err("Title must contain 1..256 bytes".into());
            }
            if text.len() > 8 * 1024 * 1024 {
                return Err("Document exceeds 8 MiB".into());
            }
            if let Some(path) = path {
                validate_path(path)?;
            }
        }
        Request::SaveAs { tab_id, path, .. } => {
            validate_id(*tab_id)?;
            validate_path(path)?;
        }
        Request::Edit {
            tab_id,
            edits,
            set_text,
            ..
        } => {
            validate_id(*tab_id)?;
            if set_text.is_some() == !edits.is_empty() {
                return Err(
                    "Supply either set_text (including empty string) or 1..256 edits, never both"
                        .into(),
                );
            }
            if edits.len() > 256 {
                return Err("At most 256 edits in one atomic document change".into());
            }
            let mut bytes = set_text.as_ref().map_or(0, String::len);
            for edit in edits {
                validate_range(&edit.range)?;
                bytes += edit.text.len();
            }
            if bytes > 8 * 1024 * 1024 {
                return Err("Replacement text exceeds 8 MiB".into());
            }
        }
        Request::Selection {
            tab_id,
            range,
            expected_revision,
        } => {
            validate_id(*tab_id)?;
            if let Some(range) = range {
                validate_range(range)?;
                if expected_revision.is_none() {
                    return Err(
                        "Setting selection requires expected_revision from read/tabs".into(),
                    );
                }
            } else if expected_revision.is_some() {
                return Err("expected_revision is only valid with a selection range".into());
            }
        }
        Request::Settings { patch: Some(patch) } => {
            if patch
                .multicursor
                .as_ref()
                .and_then(|p| p.limit)
                .is_some_and(|limit| !(100..=10_000).contains(&limit))
            {
                return Err(
                    "multicursor.limit must be 100..10000; no settings were changed".into(),
                );
            }
            if let Some(bindings) = &patch.keybindings
                && (bindings.len() > 128
                    || bindings.iter().any(|(id, binding)| {
                        id.is_empty()
                            || id.len() > 80
                            || binding.as_ref().is_some_and(|binding| {
                                !(1..=MAX_SHORTCUT_ALIASES).contains(&binding.values().count())
                                    || binding.values().any(|value| {
                                        value.is_empty() || value.len() > MAX_SHORTCUT_BYTES
                                    })
                            })
                    }))
            {
                return Err("keybindings supports at most 128 action IDs (1..80 bytes), each set to null or 1..8 shortcut strings (1..100 bytes each); no settings were changed".into());
            }
            if let Some(excludes) = &patch.search_excludes {
                normalize_search_excludes(excludes)?;
            }
            if let Some(leader_key) = &patch.leader_key {
                normalize_leader_key(leader_key)?;
            }
            if patch
                .font_size
                .is_some_and(|n| !n.is_finite() || !(10.0..=32.0).contains(&n))
            {
                return Err("font_size must be 10..32".into());
            }
            if patch
                .ui_scale
                .is_some_and(|n| !n.is_finite() || !(0.75..=2.0).contains(&n))
            {
                return Err("ui_scale must be 0.75..2.0".into());
            }
            if patch
                .line_height
                .is_some_and(|n| !n.is_finite() || !(1.0..=2.0).contains(&n))
            {
                return Err("line_height must be 1.0..2.0".into());
            }
            if patch
                .autosave_delay_ms
                .is_some_and(|n| !(300..=5000).contains(&n))
            {
                return Err("autosave_delay_ms must be 300..5000".into());
            }
            if patch.tab_width.is_some_and(|n| !(1..=8).contains(&n)) {
                return Err("tab_width must be 1..8".into());
            }
            if let Some(widths) = &patch.language_indent_widths {
                validate_indent_languages(widths.keys().map(String::as_str), widths.len())?;
                if widths.values().any(|width| !(1..=8).contains(width)) {
                    return Err("language_indent_widths values must be 1..8".into());
                }
            }
            if let Some(styles) = &patch.language_indent_with_tabs {
                validate_indent_languages(styles.keys().map(String::as_str), styles.len())?;
            }
        }
        Request::Command { tab_id, .. } => validate_id(*tab_id)?,
        Request::Find {
            tab_id,
            query,
            limit,
            offset,
            ..
        } => {
            validate_id(*tab_id)?;
            validate_query(query)?;
            validate_page(*limit, *offset)?;
        }
        Request::Replace {
            tab_id,
            query,
            replacement,
            ..
        } => {
            validate_id(*tab_id)?;
            validate_query(query)?;
            if replacement.len() > 1024 * 1024 {
                return Err("Replacement exceeds 1 MiB".into());
            }
        }
        Request::Diff { tab_id, max_lines } => {
            validate_id(*tab_id)?;
            if !(1..=1000).contains(max_lines) {
                return Err("max_lines must be 1..1000".into());
            }
        }
        Request::Diagnostics {
            tab_id,
            limit,
            offset,
        } => {
            validate_id(*tab_id)?;
            validate_page(*limit, *offset)?;
        }
        Request::Search {
            query,
            workspace,
            excludes,
            limit,
            ..
        } => {
            validate_query(query)?;
            validate_page(*limit, 0)?;
            if let Some(path) = workspace {
                validate_path(path)?;
            }
            if let Some(excludes) = excludes {
                normalize_search_excludes(excludes)?;
            }
        }
        Request::Output {
            start_line,
            line_count,
            start_column,
            workspace,
        } => {
            if *start_line == 0 || *start_column == 0 || !(1..=500).contains(line_count) {
                return Err("output uses positive start_line/start_column and 1..500 lines".into());
            }
            if let Some(path) = workspace {
                validate_path(path)?;
            }
        }
        Request::Task {
            workspace: Some(path),
            ..
        } => validate_path(path)?,
        Request::Notify {
            message,
            title,
            workspace,
            open,
            review,
            ..
        } => {
            if open.is_some() && review.is_some() {
                return Err("A notification opens one thing when clicked: give --open or --review, not both. Nothing was sent".into());
            }
            if message.trim().is_empty() || message.len() > MAX_NOTIFICATION_MESSAGE {
                return Err(format!(
                    "A notification message needs 1..{MAX_NOTIFICATION_MESSAGE} bytes of text"
                ));
            }
            if message
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
            {
                return Err("A notification message can contain line breaks and tabs, but no other control characters".into());
            }
            if let Some(title) = title
                && (title.trim().is_empty()
                    || title.len() > MAX_NOTIFICATION_TITLE
                    || title.chars().any(char::is_control))
            {
                return Err(format!(
                    "A notification title is one line of 1..{MAX_NOTIFICATION_TITLE} bytes"
                ));
            }
            if let Some(path) = workspace {
                validate_path(path)?;
            }
            if let Some(open) = open {
                validate_path(&open.path)?;
                if let Some(location) = &open.location {
                    validate_location(location)?;
                }
            }
        }
        Request::GitStatus {
            workspace,
            limit,
            offset,
        }
        | Request::GitBranches {
            workspace,
            limit,
            offset,
        }
        | Request::GitHistory {
            workspace,
            limit,
            offset,
        } => {
            validate_page(*limit, *offset)?;
            if let Some(path) = workspace {
                validate_path(path)?;
            }
        }
        Request::ChangeReview {
            source,
            rev,
            base,
            workspace,
            ..
        } => {
            match source {
                ChangeReviewSource::Commit if rev.is_none() => {
                    return Err("A commit review needs rev, e.g. HEAD or a commit hash".into());
                }
                ChangeReviewSource::Working | ChangeReviewSource::Branch if rev.is_some() => {
                    return Err("rev applies only to source commit".into());
                }
                ChangeReviewSource::Working | ChangeReviewSource::Commit if base.is_some() => {
                    return Err("base applies only to source branch".into());
                }
                _ => {}
            }
            for revision in rev.iter().chain(base) {
                // Mirrors git::validate_review_revision; vedit-cli has no Git layer.
                if revision.is_empty()
                    || revision.len() > 256
                    || revision.starts_with('-')
                    || revision.chars().any(char::is_control)
                {
                    return Err(
                        "A revision has 1..256 bytes, no leading '-' and no control characters"
                            .into(),
                    );
                }
            }
            if let Some(path) = workspace {
                validate_path(path)?;
            }
        }
        Request::GitSwitchBranch { target, workspace } => {
            if !target.starts_with("refs/heads/") && !target.starts_with("refs/remotes/")
                || target.contains(['\n', '\r', '\0'])
                || target.len() > 4096
            {
                return Err(
                    "Use an exact refs/heads/... or refs/remotes/... ref from git-branches".into(),
                );
            }
            if let Some(path) = workspace {
                validate_path(path)?;
            }
        }
        Request::Bookmarks { change } => match change {
            BookmarkAction::List { limit, offset } => validate_page(*limit, *offset)?,
            BookmarkAction::Toggle { tab_id, label } => {
                if let Some(tab_id) = tab_id {
                    validate_id(*tab_id)?;
                }
                if let Some(label) = label {
                    validate_bookmark_label(label)?;
                }
            }
            BookmarkAction::Remove { id } | BookmarkAction::Jump { id } => {
                validate_bookmark_id(*id)?
            }
            BookmarkAction::Rename { id, label } => {
                validate_bookmark_id(*id)?;
                validate_bookmark_label(label)?;
            }
        },
        Request::AgentMarks { action } => match action {
            AgentMarksAction::List { limit, .. } | AgentMarksAction::Batch { limit, .. } => {
                validate_agent_mark_page(*limit)?;
                if let AgentMarksAction::Batch {
                    batch_id: Some(id), ..
                } = action
                {
                    validate_agent_mark_id(*id)?;
                }
            }
            AgentMarksAction::Take { request_id, limit } => {
                validate_agent_mark_page(*limit)?;
                if let Some(request_id) = request_id
                    && (request_id.is_empty()
                        || request_id.len() > 128
                        || request_id.chars().any(char::is_control))
                {
                    return Err(
                        "request_id must contain 1..128 UTF-8 bytes without control characters"
                            .into(),
                    );
                }
            }
            AgentMarksAction::Text {
                id,
                batch_id,
                count,
                ..
            } => {
                validate_agent_mark_id(*id)?;
                if let Some(batch_id) = batch_id {
                    validate_agent_mark_id(*batch_id)?;
                }
                if !(1..=65536).contains(count) {
                    return Err("Agent mark text count must be 1..65536 Unicode characters; offset must be inside the captured snippet".into());
                }
            }
            AgentMarksAction::Ack { batch_id } => validate_agent_mark_id(*batch_id)?,
            AgentMarksAction::Restore { batch_id } => {
                if let Some(batch_id) = batch_id {
                    validate_agent_mark_id(*batch_id)?;
                }
            }
            AgentMarksAction::Remove { id } => validate_agent_mark_id(*id)?,
            AgentMarksAction::Note { id, note } => {
                validate_agent_mark_id(*id)?;
                validate_agent_mark_note(note)?;
            }
            AgentMarksAction::Add { tab_id, note } => {
                if let Some(tab_id) = tab_id {
                    validate_id(*tab_id)?;
                }
                if let Some(note) = note {
                    validate_agent_mark_note(note)?;
                }
            }
            AgentMarksAction::Clear {} => {}
        },
        Request::Layout { change } => match change {
            LayoutAction::Status {} => {}
            LayoutAction::Split {
                pane_id, tab_id, ..
            } => {
                if let Some(id) = pane_id {
                    validate_id(*id)?;
                }
                if let Some(id) = tab_id {
                    validate_id(*id)?;
                }
            }
            LayoutAction::Focus { pane_id } | LayoutAction::Close { pane_id } => {
                validate_id(*pane_id)?
            }
            LayoutAction::Assign {
                pane_id, tab_id, ..
            }
            | LayoutAction::CloseTab { pane_id, tab_id } => {
                validate_id(*pane_id)?;
                validate_id(*tab_id)?;
            }
            LayoutAction::MoveTab {
                source_pane_id,
                pane_id,
                tab_id,
                index,
            } => {
                validate_id(*source_pane_id)?;
                validate_id(*pane_id)?;
                validate_id(*tab_id)?;
                if *index > 100_000 {
                    return Err(
                        "Tab index must be 0..100000 and within the target tab group".into(),
                    );
                }
            }
            LayoutAction::Resize { pane_id, ratio } => {
                validate_id(*pane_id)?;
                if !ratio.is_finite() || !(0.15..=0.85).contains(ratio) {
                    return Err("Split ratio must be 0.15..0.85".into());
                }
            }
        },
        Request::File { action } => {
            let (path, name) = match action {
                FileAction::Rename { path, name } => (path, Some(name)),
                FileAction::CreateFolder { parent, name } => (parent, Some(name)),
                FileAction::Delete { path } => (path, None),
            };
            validate_path(path)?;
            if let Some(name) = name
                && (name.trim().is_empty()
                    || name.len() > 255
                    || name.contains(['\\', '/'])
                    || matches!(name.as_str(), "." | ".."))
            {
                return Err(
                    "File action name must be a single new name (1..255 bytes), never a path"
                        .into(),
                );
            }
        }
        Request::FileJob { job_id } => validate_id(*job_id)?,
        Request::Batch { actions, .. } => {
            if actions.is_empty() || actions.len() > 100 {
                return Err("Batch requires 1..100 ordered actions".into());
            }
            for action in actions {
                if matches!(
                    action,
                    Request::Batch { .. }
                        | Request::Search { .. }
                        | Request::File { .. }
                        | Request::GitBranches { .. }
                        | Request::GitHistory { .. }
                        | Request::GitSwitchBranch { .. }
                        | Request::ChangeReview { .. }
                        | Request::Launch { .. }
                ) {
                    return Err("Nested batches, background disk search and asynchronous file actions cannot be inside batch; run search/file/git-branches/git-history/git-switch-branch/change review separately".into());
                }
                validate(action)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn validate_query(query: &str) -> Result<(), String> {
    if query.is_empty() || query.len() > 16_384 {
        Err("Literal search query must contain 1..16384 bytes".into())
    } else {
        Ok(())
    }
}

fn validate_bookmark_label(label: &str) -> Result<(), String> {
    if label.len() > 256 || label.chars().any(char::is_control) {
        Err("Bookmark label must be single-line text of at most 256 UTF-8 bytes".into())
    } else {
        Ok(())
    }
}

fn validate_bookmark_id(id: u64) -> Result<(), String> {
    if id == 0 {
        Err("Bookmark ID must be a positive integer returned by bookmarks".into())
    } else {
        Ok(())
    }
}
fn validate_agent_mark_id(id: u64) -> Result<(), String> {
    if id == 0 {
        Err("Agent mark/batch ID must be a positive integer returned by agent-marks".into())
    } else {
        Ok(())
    }
}
fn validate_agent_mark_note(note: &str) -> Result<(), String> {
    if note.chars().count() > 4096 {
        Err("Agent mark notes accept at most 4096 Unicode characters".into())
    } else {
        Ok(())
    }
}
fn validate_agent_mark_page(limit: usize) -> Result<(), String> {
    if !(1..=500).contains(&limit) {
        Err("Agent marks page limit must be 1..500".into())
    } else {
        Ok(())
    }
}
fn validate_page(limit: usize, offset: usize) -> Result<(), String> {
    if !(1..=500).contains(&limit) || offset > 100_000 {
        Err("limit must be 1..500; offset must be <=100000".into())
    } else {
        Ok(())
    }
}
fn validate_path(path: &std::path::Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        Err("Paths must be absolute; CLI resolves relative paths".into())
    } else {
        Ok(())
    }
}
fn validate_range(range: &TextRange) -> Result<(), String> {
    validate_location(&range.start)?;
    validate_location(&range.end)?;
    if (range.end.line, range.end.column) < (range.start.line, range.start.column) {
        return Err("Edit/selection exclusive end must not precede start".into());
    }
    Ok(())
}
fn validate_location(location: &Location) -> Result<(), String> {
    if location.line == 0 || location.column == 0 {
        Err("Lines and columns are 1-based positive integers".into())
    } else {
        Ok(())
    }
}
fn validate_id(id: u64) -> Result<(), String> {
    if id == 0 {
        Err("Tab ID must be positive".into())
    } else {
        Ok(())
    }
}
fn validate_ids(ids: &[u64]) -> Result<(), String> {
    if ids.len() > MAX_FILES {
        return Err("At most 100 tab IDs per request".into());
    }
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        validate_id(*id)?;
        if !seen.insert(id) {
            return Err("Duplicate tab ID".into());
        }
    }
    Ok(())
}
