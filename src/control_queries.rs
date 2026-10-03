//! Bounded observations of the editor. Project search never reads files on the GUI thread.
use super::*;
use std::sync::atomic::AtomicUsize;

const QUERY_PAYLOAD_BYTES: usize = 262_144;
const TEXT_BYTES: usize = 32_768;
const MAX_SEARCH_WORKERS: usize = 4;
static SEARCH_WORKERS: AtomicUsize = AtomicUsize::new(0);

impl MorrowApp {
    pub(super) fn control_query(&self, request: &Request) -> Option<Response> {
        if !matches!(
            request,
            Request::Diff { .. }
                | Request::Diagnostics { .. }
                | Request::Output { .. }
                | Request::GitStatus { .. }
        ) {
            return None;
        }
        if let Err(error) = cli::validate(request) {
            return Some(Response::failure("invalid_request", error));
        }
        Some(match request {
            Request::Diff { tab_id, max_lines } => self.control_buffer_diff(*tab_id, *max_lines),
            Request::Diagnostics {
                tab_id,
                limit,
                offset,
            } => self.control_diagnostics(*tab_id, *limit, *offset),
            Request::Output {
                start_line,
                line_count,
            } => self.control_output(*start_line, *line_count),
            Request::GitStatus {
                workspace,
                limit,
                offset,
            } => self.control_git_status(workspace.as_deref(), *limit, *offset),
            _ => unreachable!(),
        })
    }

    /// Replies after the bounded filesystem worker completes. No search widget state is changed.
    pub(super) fn control_query_async(&self, request: &Request, reply: Sender<Response>) -> bool {
        let Request::Search {
            query,
            workspace,
            excludes,
            case_sensitive,
            whole_word,
            limit,
        } = request
        else {
            return false;
        };
        if let Err(error) = cli::validate(request) {
            let _ = reply.send(Response::failure("invalid_request", error));
            return true;
        }
        let project = match self.control_query_project(workspace.as_deref()) {
            Ok(project) => project,
            Err(response) => {
                let _ = reply.send(response);
                return true;
            }
        };
        let excludes_source = if excludes.is_some() {
            "request"
        } else {
            "settings"
        };
        let excludes = match cli::normalize_search_excludes(
            excludes
                .as_deref()
                .unwrap_or(&self.settings.search_excludes),
        ) {
            Ok(excludes) => excludes,
            Err(error) => {
                let _ = reply.send(Response::failure("invalid_search_excludes", error));
                return true;
            }
        };
        let permit = match SearchPermit::acquire() {
            Some(permit) => permit,
            None => {
                let _ = reply.send(Response::failure(
                    "search_busy",
                    "Four project searches are already running. No search was started; retry after they finish.",
                ));
                return true;
            }
        };
        let root = crate::paths::display(&project.root);
        let root_path = project.root.clone();
        let query = query.clone();
        let options = workspace::SearchOptions {
            case_sensitive: *case_sensitive,
            whole_word: *whole_word,
            max_matches: *limit,
            max_total_bytes: 32 * 1024 * 1024,
            max_duration: Duration::from_secs(2),
            ..Default::default()
        };
        let worker_reply = reply.clone();
        let spawn = std::thread::Builder::new()
            .name("morrow-cli-search".into())
            .spawn(move || {
                let _permit = permit;
                let response =
                    match workspace::search_with_excludes(&root_path, &query, options, &excludes) {
                        Ok(results) => {
                            search_response(root, query, results, excludes, excludes_source)
                        }
                        Err(error) => Response::failure("search_failed", error),
                    };
                let _ = worker_reply.send(response);
            });
        if let Err(error) = spawn {
            let _ = reply.send(Response::failure("search_failed", error.to_string()));
        }
        true
    }

    fn control_query_project(&self, path: Option<&Path>) -> Result<&Project, Response> {
        if let Some(path) = path {
            return self
                .projects
                .iter()
                .find(|project| same_workspace(&project.root, path))
                .ok_or_else(|| {
                    Response::failure(
                        "unknown_workspace",
                        "The requested workspace is not open. Inspect status and use one of its workspace paths, or open it explicitly first.",
                    )
                });
        }
        self.project().ok_or_else(|| {
            Response::failure(
                "no_workspace",
                "Open a workspace before querying project state.",
            )
        })
    }

    fn control_buffer_diff(&self, id: u64, max_lines: usize) -> Response {
        let Some((project, index)) = self.control_find_tab(id) else {
            return unknown_tab(id);
        };
        let tab = &self.projects[project].tabs[index];
        if let Some(error) = unavailable_document(tab) {
            return error;
        }
        let before: Vec<_> = tab.saved_text.split_inclusive('\n').collect();
        let after: Vec<_> = tab.text.split_inclusive('\n').collect();
        let prefix = before
            .iter()
            .zip(&after)
            .take_while(|(before, after)| before == after)
            .count();
        let suffix = before[prefix..]
            .iter()
            .rev()
            .zip(after[prefix..].iter().rev())
            .take_while(|(before, after)| before == after)
            .count();
        let removed = &before[prefix..before.len() - suffix];
        let added = &after[prefix..after.len() - suffix];
        let (removed_text, removed_returned, removed_truncated, _) =
            bounded_lines(removed, max_lines);
        let (added_text, added_returned, added_truncated, _) = bounded_lines(added, max_lines);
        let changed = !removed.is_empty() || !added.is_empty();
        let hunks = if changed {
            vec![json!({
                "before":{"start_line":prefix+1,"line_count":removed.len()},
                "after":{"start_line":prefix+1,"line_count":added.len()},
                "removed":removed_text,"added":added_text,
                "removed_returned_lines":removed_returned,"added_returned_lines":added_returned,
                "truncated":removed_truncated||added_truncated
            })]
        } else {
            Vec::new()
        };
        Response::success(json!({
            "tab_id":id,"revision":tab.revision,"dirty":tab.dirty,
            "base":"saved_buffer","kind":"contiguous_replacement",
            "changed":changed,"hunks":hunks,
            "truncated":removed_truncated||added_truncated
        }))
    }

    fn control_diagnostics(&self, id: u64, limit: usize, offset: usize) -> Response {
        let Some((project_index, tab_index)) = self.control_find_tab(id) else {
            return unknown_tab(id);
        };
        let project = &self.projects[project_index];
        let tab = &project.tabs[tab_index];
        if let Some(error) = unavailable_document(tab) {
            return error;
        }
        let diagnostics = tab
            .path
            .as_ref()
            .and_then(|path| project.diagnostics.get(path));
        let total = diagnostics.map_or(0, Vec::len);
        let mut items = Vec::new();
        let mut bytes = 0;
        if let Some(diagnostics) = diagnostics {
            let selected: Vec<_> = diagnostics.iter().skip(offset).take(limit).collect();
            let positions = diagnostic_positions(&tab.text, &selected);
            for diagnostic in selected {
                let (start, start_clamped) = &positions[&(
                    diagnostic.range.start.line,
                    diagnostic.range.start.character,
                )];
                let (end, end_clamped) =
                    &positions[&(diagnostic.range.end.line, diagnostic.range.end.character)];
                let mut message = diagnostic.message.clone();
                truncate_utf8(&mut message, 4096);
                let mut source = diagnostic.source.clone();
                if let Some(source) = &mut source {
                    truncate_utf8(source, 512);
                }
                let value = json!({
                    "range":{"start":start,"end":end},
                    "severity":match diagnostic.severity {1=>"error",2=>"warning",3=>"information",_=>"hint"},
                    "message":message,"message_truncated":message.len()<diagnostic.message.len(),"source":source,
                    "location_clamped":*start_clamped||*end_clamped
                });
                let item_bytes =
                    serde_json::to_vec(&value).map_or(QUERY_PAYLOAD_BYTES, |bytes| bytes.len());
                if bytes + item_bytes > QUERY_PAYLOAD_BYTES {
                    break;
                }
                bytes += item_bytes;
                items.push(value);
            }
        }
        let next = offset.saturating_add(items.len());
        Response::success(json!({
            "tab_id":id,"revision":tab.revision,"analysis_ready":tab.path.as_deref().and_then(|path|project.language_for(path)).is_some_and(|session|session.active()),
            "analysis_error":tab.path.as_deref().and_then(|path|project.language_for(path)).and_then(|session|session.error.as_ref()),"column_encoding":"unicode_scalar","end_exclusive":true,
            "items":items,"total":total,"offset":offset,"has_more":next<total,
            "next_offset":if next<total{Some(next)}else{None}
        }))
    }

    fn control_output(&self, start_line: usize, line_count: usize) -> Response {
        let lines: Vec<_> = self.output.split_inclusive('\n').collect();
        // Empty output still has a valid first line, so agents can inspect a newly started task.
        let total = lines.len().max(1);
        if start_line > total {
            return Response::failure(
                "invalid_location",
                format!("Output has {total} lines; requested line {start_line}"),
            );
        }
        let start = (start_line - 1).min(lines.len());
        let (text, returned, truncated, partial_line) = bounded_lines(&lines[start..], line_count);
        let end = start + returned;
        let mut command = self.command.clone();
        truncate_utf8(&mut command, 4096);
        let mut status = self.task_status.clone();
        truncate_utf8(&mut status, 1024);
        Response::success(json!({
            "text":text,"start_line":start_line,"end_line":end,"total_lines":total,
            "truncated":truncated,"byte_limit_reached":partial_line,"next_line":if !partial_line&&end<lines.len(){Some(end+1)}else{None},
            "running":self.task.is_some(),"status":status,"command":command,
            "snapshot":true
        }))
    }

    fn control_git_status(&self, path: Option<&Path>, limit: usize, offset: usize) -> Response {
        let project = match self.control_query_project(path) {
            Ok(project) => project,
            Err(response) => return response,
        };
        let total = project.git_changes.len();
        let mut items = Vec::new();
        let mut bytes = 0;
        for file in project.git_changes.iter().skip(offset).take(limit) {
            let value = json!({
                "path":file.path,"original_path":file.original_path,
                "index":git_change(file.index),"worktree":git_change(file.worktree),
                "staged":file.is_staged(),"unstaged":file.is_unstaged(),
                "untracked":file.is_untracked(),"conflicted":file.is_conflicted(),
                "staged_stats":project.git_stats.staged.get(&file.path).map(|stats| json!({"added":stats.added,"removed":stats.removed,"binary":stats.binary})),
                "worktree_stats":project.git_stats.unstaged.get(&file.path).map(|stats| json!({"added":stats.added,"removed":stats.removed,"binary":stats.binary}))
            });
            let item_bytes =
                serde_json::to_vec(&value).map_or(QUERY_PAYLOAD_BYTES, |bytes| bytes.len());
            if bytes + item_bytes > QUERY_PAYLOAD_BYTES {
                break;
            }
            bytes += item_bytes;
            items.push(value);
        }
        let next = offset.saturating_add(items.len());
        let tracking = project.git_tracking.as_ref().map(|tracking| {
            let mut upstream = tracking.upstream.clone();
            if let Some(upstream) = &mut upstream {
                truncate_utf8(upstream, 1024);
            }
            json!({"upstream":upstream,"ahead":tracking.ahead,"behind":tracking.behind})
        });
        let mut branch = project.git_branch.clone();
        if let Some(branch) = &mut branch {
            truncate_utf8(branch, 1024);
        }
        let mut error = project.git_error.clone();
        if let Some(error) = &mut error {
            truncate_utf8(error, 4096);
        }
        let mut stats_error = project.git_stats_error.clone();
        if let Some(error) = &mut stats_error {
            truncate_utf8(error, 4096);
        }
        Response::success(json!({
            "workspace":crate::paths::display(&project.root),
            "repository":project.git_root.as_deref().map(crate::paths::display),
            "branch":branch,"tracking":tracking,"items":items,
            "total":total,"offset":offset,"has_more":next<total,"next_offset":if next<total{Some(next)}else{None},
            "loading":project.git_loading,"error":error,"stats_error":stats_error,
            "cached":true,"age_ms":project.last_git_refresh.elapsed().as_millis()
        }))
    }
}

fn unavailable_document(tab: &EditorTab) -> Option<Response> {
    if tab.loading {
        Some(Response::failure(
            "loading",
            "The document is still loading. Inspect status and retry after loading is false.",
        ))
    } else {
        tab.error
            .as_ref()
            .map(|error| Response::failure("file_error", error))
    }
}

/// Convert all requested positions with one scan of each affected line. Repeated
/// full-document UTF-16 scans would make a large diagnostics page block the GUI.
fn diagnostic_positions(
    text: &str,
    diagnostics: &[&crate::lsp::Diagnostic],
) -> HashMap<(u32, u32), (Location, bool)> {
    use std::collections::{BTreeMap, BTreeSet};
    let lines: Vec<_> = text.split('\n').collect();
    let mut requested: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for diagnostic in diagnostics {
        for position in [diagnostic.range.start, diagnostic.range.end] {
            requested
                .entry(position.line)
                .or_default()
                .insert(position.character);
        }
    }
    let mut positions = HashMap::new();
    let last_column = lines.last().unwrap().chars().count() + 1;
    for (line, columns) in requested {
        let Some(content) = lines.get(line as usize) else {
            for column in columns {
                positions.insert(
                    (line, column),
                    (
                        Location {
                            line: lines.len(),
                            column: last_column,
                        },
                        true,
                    ),
                );
            }
            continue;
        };
        let content = if (line as usize) + 1 < lines.len() {
            content.strip_suffix('\r').unwrap_or(content)
        } else {
            content
        };
        let mut chars = content.chars().peekable();
        let mut units = 0;
        let mut scalar_column = 1;
        for column in columns {
            while units < column {
                let Some(character) = chars.peek() else {
                    break;
                };
                let length = character.len_utf16() as u32;
                if units + length > column {
                    break;
                }
                units += length;
                scalar_column += 1;
                chars.next();
            }
            positions.insert(
                (line, column),
                (
                    Location {
                        line: line as usize + 1,
                        column: scalar_column,
                    },
                    units != column,
                ),
            );
        }
    }
    positions
}

fn bounded_lines(lines: &[&str], max_lines: usize) -> (String, usize, bool, bool) {
    let mut text = String::new();
    let mut returned = 0;
    let mut partial_line = false;
    for line in lines.iter().take(max_lines) {
        let remaining = TEXT_BYTES - text.len();
        if line.len() > remaining {
            let mut end = remaining;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            text.push_str(&line[..end]);
            partial_line = true;
            break;
        }
        text.push_str(line);
        returned += 1;
    }
    (
        text,
        returned,
        partial_line || returned < lines.len(),
        partial_line,
    )
}

fn same_workspace(left: &Path, right: &Path) -> bool {
    // Components remove harmless `.` and repeated/trailing separators without
    // reading the filesystem or guessing the identity of a symbolic link.
    let left: PathBuf = Path::new(&crate::paths::display(left))
        .components()
        .collect();
    let right: PathBuf = Path::new(&crate::paths::display(right))
        .components()
        .collect();
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    #[cfg(windows)]
    {
        left.replace('/', "\\")
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(right.replace('/', "\\").trim_end_matches('\\'))
    }
    #[cfg(not(windows))]
    {
        left.trim_end_matches('/') == right.trim_end_matches('/')
    }
}

fn git_change(change: git::FileChange) -> &'static str {
    match change {
        git::FileChange::None => "none",
        git::FileChange::Modified => "modified",
        git::FileChange::Added => "added",
        git::FileChange::Deleted => "deleted",
        git::FileChange::Renamed => "renamed",
        git::FileChange::Copied => "copied",
        git::FileChange::Unmerged => "unmerged",
        git::FileChange::Untracked => "untracked",
        git::FileChange::Other(_) => "other",
    }
}

fn search_response(
    root: String,
    query: String,
    results: workspace::SearchResults,
    excludes: Vec<String>,
    excludes_source: &str,
) -> Response {
    let mut items = Vec::new();
    let mut bytes = 0;
    for found in &results.matches {
        let value = json!({
            "path":crate::paths::display(&found.path),
            "location":{"line":found.line,"column":found.column},
            "start_char":found.start_char,"end_char":found.end_char,
            "preview":found.preview
        });
        let item_bytes =
            serde_json::to_vec(&value).map_or(QUERY_PAYLOAD_BYTES, |bytes| bytes.len());
        if bytes + item_bytes > QUERY_PAYLOAD_BYTES {
            break;
        }
        bytes += item_bytes;
        items.push(value);
    }
    let omitted = results.matches.len() - items.len();
    let warnings: Vec<_> = results
        .warnings
        .iter()
        .take(20)
        .map(|warning| {
            let mut warning = warning.clone();
            truncate_utf8(&mut warning, 4096);
            warning
        })
        .collect();
    Response::success(json!({
        "workspace":root,"query":query,"scope":"workspace_files_on_disk",
        "excludes":excludes,"excludes_source":excludes_source,
        "includes_unsaved_buffers":false,"items":items,"truncated":results.truncated||omitted>0,
        "omitted_matches":omitted,"files_searched":results.files_searched,
        "files_skipped":results.files_skipped,"warnings":warnings,
        "column_encoding":"unicode_scalar"
    }))
}

struct SearchPermit;
impl SearchPermit {
    fn acquire() -> Option<Self> {
        SEARCH_WORKERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_SEARCH_WORKERS).then_some(count + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for SearchPermit {
    fn drop(&mut self) {
        SEARCH_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_tab(text: &str, saved: &str) -> (MorrowApp, u64) {
        let mut app = MorrowApp::empty(egui::Context::default());
        let mut project = Project::new(std::env::temp_dir().join("morrow-query-fixture"));
        let mut tab = EditorTab::new(Some(project.root.join("code.rs")), "code.rs".into());
        tab.text = text.into();
        tab.saved_text = saved.into();
        tab.dirty = text != saved;
        let id = tab.id;
        project.tabs.push(tab);
        app.projects.push(project);
        (app, id)
    }

    #[test]
    fn diff_observes_current_unsaved_buffer_without_changing_it() {
        let (app, id) = app_with_tab("same\nnew 😀\nlast\n", "same\nold\nlast\n");
        let response = app
            .control_query(&Request::Diff {
                tab_id: id,
                max_lines: 500,
            })
            .unwrap();
        assert!(response.ok);
        assert_eq!(response.result["hunks"][0]["before"]["start_line"], 2);
        assert_eq!(response.result["hunks"][0]["removed"], "old\n");
        assert_eq!(response.result["hunks"][0]["added"], "new 😀\n");
        assert_eq!(app.tab().unwrap().text, "same\nnew 😀\nlast\n");
        assert!(app.tab().unwrap().dirty);
    }

    #[test]
    fn diff_handles_eof_insertions_empty_buffers_and_explicit_truncation() {
        let (app, id) = app_with_tab("a\nb\nc\n", "");
        let response = app
            .control_query(&Request::Diff {
                tab_id: id,
                max_lines: 1,
            })
            .unwrap();
        assert_eq!(response.result["hunks"][0]["removed"], "");
        assert_eq!(response.result["hunks"][0]["after"]["line_count"], 3);
        assert_eq!(response.result["hunks"][0]["added"], "a\n");
        assert_eq!(response.result["truncated"], true);
        let (same, id) = app_with_tab("", "");
        assert_eq!(
            same.control_query(&Request::Diff {
                tab_id: id,
                max_lines: 500
            })
            .unwrap()
            .result["hunks"],
            json!([])
        );
    }

    #[test]
    fn diagnostics_convert_surrogate_pairs_to_exact_unicode_columns_and_page() {
        let (mut app, id) = app_with_tab("😀hello\nnext\n", "😀hello\nnext\n");
        let path = app.tab().unwrap().path.clone().unwrap();
        let diagnostic = crate::lsp::Diagnostic {
            range: crate::lsp::TextRange {
                start: crate::lsp::Position {
                    line: 0,
                    character: 2,
                },
                end: crate::lsp::Position {
                    line: 0,
                    character: 7,
                },
            },
            severity: 2,
            message: "expected warning".into(),
            source: Some("rust-analyzer".into()),
        };
        let mut stale = diagnostic.clone();
        stale.range.start.character = 1; // Inside the emoji's UTF-16 surrogate pair.
        app.projects[0]
            .diagnostics
            .insert(path, vec![diagnostic, stale]);
        let response = app
            .control_query(&Request::Diagnostics {
                tab_id: id,
                limit: 1,
                offset: 0,
            })
            .unwrap();
        assert_eq!(
            response.result["items"][0]["range"]["start"],
            json!({"line":1,"column":2})
        );
        assert_eq!(
            response.result["items"][0]["range"]["end"],
            json!({"line":1,"column":7})
        );
        assert_eq!(response.result["items"][0]["severity"], "warning");
        assert_eq!(response.result["next_offset"], 1);
        let stale = app
            .control_query(&Request::Diagnostics {
                tab_id: id,
                limit: 1,
                offset: 1,
            })
            .unwrap();
        assert_eq!(
            stale.result["items"][0]["range"]["start"],
            json!({"line":1,"column":1})
        );
        assert_eq!(stale.result["items"][0]["location_clamped"], true);
        assert_eq!(app.tab().unwrap().vim.cursor, 0);
    }

    #[test]
    fn unknown_query_target_does_not_fall_back_to_active_buffer() {
        let (app, _) = app_with_tab("valuable", "saved");
        let response = app
            .control_query(&Request::Diff {
                tab_id: u64::MAX,
                max_lines: 500,
            })
            .unwrap();
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "unknown_tab");
        assert_eq!(app.tab().unwrap().text, "valuable");
    }

    #[test]
    fn production_dispatch_allows_read_only_observations_during_a_user_dialog() {
        let (mut app, id) = app_with_tab("valuable edits", "saved");
        app.settings_open = true;
        app.output = "completed output\n".into();
        let requests = [
            Request::Diff {
                tab_id: id,
                max_lines: 500,
            },
            Request::Diagnostics {
                tab_id: id,
                limit: 100,
                offset: 0,
            },
            Request::Output {
                start_line: 1,
                line_count: 100,
            },
            Request::GitStatus {
                workspace: None,
                limit: 100,
                offset: 0,
            },
        ];
        for request in requests {
            let response = app.control_request(request);
            assert!(response.ok, "{:?}", response.error);
        }
        assert!(app.settings_open);
        assert_eq!(app.tab().unwrap().id, id);
        assert_eq!(app.tab().unwrap().text, "valuable edits");
        assert!(app.tab().unwrap().dirty);
    }

    #[test]
    fn output_pages_current_snapshot_and_git_observation_does_not_refresh() {
        let (mut app, _) = app_with_tab("", "");
        app.output = "first\nsecond\nthird\n".into();
        app.task_status = "Exited with code 0".into();
        let response = app
            .control_query(&Request::Output {
                start_line: 2,
                line_count: 1,
            })
            .unwrap();
        assert_eq!(response.result["text"], "second\n");
        assert_eq!(response.result["next_line"], 3);
        app.projects[0].git_branch = Some("feature".into());
        app.projects[0].git_changes.push(git::FileStatus {
            path: "code.rs".into(),
            original_path: None,
            index: git::FileChange::Modified,
            worktree: git::FileChange::Modified,
        });
        app.projects[0].git_stats.staged.insert(
            "code.rs".into(),
            git::LineStats {
                added: 8,
                removed: 2,
                binary: false,
            },
        );
        app.projects[0].git_stats.unstaged.insert(
            "code.rs".into(),
            git::LineStats {
                added: 3,
                removed: 1,
                binary: false,
            },
        );
        let response = app
            .control_query(&Request::GitStatus {
                workspace: None,
                limit: 100,
                offset: 0,
            })
            .unwrap();
        assert_eq!(response.result["branch"], "feature");
        assert_eq!(response.result["items"][0]["worktree"], "modified");
        assert_eq!(response.result["items"][0]["staged_stats"]["added"], 8);
        assert_eq!(response.result["items"][0]["worktree_stats"]["added"], 3);
        assert!(!app.projects[0].git_loading);
    }

    #[test]
    fn queries_report_unavailable_buffers_and_do_not_read_empty_loading_text() {
        let (mut app, id) = app_with_tab("", "");
        app.tab_mut().unwrap().loading = true;
        let request = Request::Diff {
            tab_id: id,
            max_lines: 500,
        };
        let response = app.control_query(&request).unwrap();
        assert_eq!(response.error.unwrap().code, "loading");
        app.tab_mut().unwrap().loading = false;
        app.tab_mut().unwrap().error = Some("disk unavailable".into());
        let response = app.control_query(&request).unwrap();
        assert_eq!(response.error.unwrap().code, "file_error");
    }

    #[test]
    fn a_large_unicode_output_line_has_no_false_line_continuation() {
        let (mut app, _) = app_with_tab("", "");
        app.output = "😀".repeat(TEXT_BYTES);
        let response = app
            .control_query(&Request::Output {
                start_line: 1,
                line_count: 100,
            })
            .unwrap();
        let text = response.result["text"].as_str().unwrap();
        assert_eq!(text.len(), TEXT_BYTES);
        assert!(text.chars().all(|character| character == '😀'));
        assert_eq!(response.result["byte_limit_reached"], true);
        assert_eq!(response.result["next_line"], Value::Null);
        assert!(serde_json::to_vec(&response).unwrap().len() < QUERY_PAYLOAD_BYTES);
    }

    #[test]
    fn asynchronous_search_rejects_an_unknown_explicit_workspace() {
        let (mut app, _) = app_with_tab("user buffer", "saved");
        app.projects[0].index = Some(Arc::new(workspace::WorkspaceIndex::empty(
            app.projects[0].root.clone(),
        )));
        let (reply, receiver) = mpsc::channel();
        let request = Request::Search {
            query: "needle".into(),
            workspace: Some(std::env::temp_dir().join("unknown-query-workspace")),
            excludes: None,
            case_sensitive: false,
            whole_word: false,
            limit: 10,
        };
        assert!(app.control_query_async(&request, reply));
        let response = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(response.error.unwrap().code, "unknown_workspace");
        assert_eq!(app.tab().unwrap().text, "user buffer");
    }

    #[test]
    fn asynchronous_search_reads_disk_files_without_an_index_and_preserves_gui_search() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "morrow-query-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("code.rs"), "😀 needle\nNeedle\n").unwrap();
        let (mut app, _) = app_with_tab("unsaved replacement", "old");
        app.projects[0].root = directory.clone();
        app.projects[0].index = None;
        app.project_query = "user query".into();
        let (reply, receiver) = mpsc::channel();
        let request = Request::Search {
            query: "needle".into(),
            workspace: Some(directory.join(".")),
            excludes: None,
            case_sensitive: true,
            whole_word: true,
            limit: 10,
        };
        assert!(app.control_query_async(&request, reply));
        let response = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.result["items"].as_array().unwrap().len(), 1);
        assert_eq!(
            response.result["items"][0]["location"],
            json!({"line":1,"column":3})
        );
        assert_eq!(response.result["includes_unsaved_buffers"], false);
        assert_eq!(app.project_query, "user query");
        assert_eq!(app.tab().unwrap().text, "unsaved replacement");
        drop(app);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn asynchronous_search_reports_effective_filters_and_override_keeps_inactive_workspace_background()
     {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "morrow-query-excludes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for folder in ["generated", "ignored", ".git", "other"] {
            fs::create_dir_all(directory.join(folder)).unwrap();
        }
        fs::write(directory.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(directory.join("source.rs"), "needle source\n").unwrap();
        for folder in ["generated", "ignored", ".git"] {
            fs::write(directory.join(folder).join("file.rs"), "needle hidden\n").unwrap();
        }
        let (mut app, id) = app_with_tab("unsaved user document", "saved");
        app.projects[0].root = directory.clone();
        app.bind_current_pane();
        app.projects.push(Project::new(directory.join("other")));
        app.switch_project(1);
        let active_layout = app.layout_status();
        app.settings.search_excludes = vec!["generated".into()];
        app.project_query = "keep user query".into();
        for (override_rules, expected_rules, expected_count, source) in [
            (None, vec!["generated"], 1, "settings"),
            (Some(Vec::new()), Vec::new(), 2, "request"),
            (
                Some(vec!["generated".into(), "source.rs".into()]),
                vec!["generated", "source.rs"],
                0,
                "request",
            ),
        ] {
            let (reply, receiver) = mpsc::channel();
            let request = Request::Search {
                query: "needle".into(),
                workspace: Some(directory.clone()),
                excludes: override_rules,
                case_sensitive: true,
                whole_word: true,
                limit: 10,
            };
            assert!(app.control_query_async(&request, reply));
            let response = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(response.ok, "{:?}", response.error);
            assert_eq!(
                response.result["items"].as_array().unwrap().len(),
                expected_count
            );
            assert_eq!(response.result["excludes"], json!(expected_rules));
            assert_eq!(response.result["excludes_source"], source);
            assert_eq!(response.result["scope"], "workspace_files_on_disk");
            assert_eq!(app.settings.search_excludes, ["generated"]);
            assert_eq!(app.active_project, 1);
            assert_eq!(app.layout_status(), active_layout);
            assert_eq!(app.project_query, "keep user query");
            assert_eq!(app.projects[0].tabs[0].id, id);
            assert_eq!(app.projects[0].tabs[0].text, "unsaved user document");
        }
        let (reply, receiver) = mpsc::channel();
        app.settings.search_excludes = vec!["[broken".into()];
        let request = Request::Search {
            query: "needle".into(),
            workspace: Some(directory.clone()),
            excludes: None,
            case_sensitive: false,
            whole_word: false,
            limit: 10,
        };
        assert!(app.control_query_async(&request, reply));
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .error
                .unwrap()
                .code,
            "invalid_search_excludes"
        );
        assert_eq!(app.layout_status(), active_layout);
        drop(app);
        fs::remove_dir_all(directory).unwrap();
    }
}
