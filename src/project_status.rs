//! Project repository selection and status-only refreshes for inactive tabs.
use super::*;

const REFRESH_INTERVAL: Duration = Duration::from_secs(15);
const MAX_REQUESTS: usize = 2;

/// Branch, upstream divergence and change totals of a project's repository.
/// Full and status-only refreshes both keep it current, so project tabs and
/// the repository overview never start Git for an open project themselves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GitSummary {
    /// The repository's top-level working tree as reported by Git.
    pub(super) repository: PathBuf,
    pub(super) status: git::WorktreeStatus,
}

impl GitSummary {
    /// Summarize a full refresh. Its history snapshot already names HEAD, so
    /// this adds no Git process to the active project's refresh.
    pub(super) fn from_snapshot(snapshot: &GitSnapshot) -> Option<Self> {
        let repository = snapshot.root.clone()?;
        let head = snapshot.history.as_ref().ok().and_then(|history| {
            history
                .snapshot
                .refs
                .iter()
                .find(|reference| reference.current)
                .map(|reference| reference.hash.clone())
        });
        let tracking = snapshot.tracking.clone().unwrap_or(git::BranchInfo {
            name: snapshot.branch.clone(),
            upstream: None,
            ahead: 0,
            behind: 0,
            divergence_known: false,
        });
        Some(Self {
            repository,
            status: git::WorktreeStatus {
                head,
                tracking,
                changes: snapshot.files.len(),
                conflicts: snapshot
                    .files
                    .iter()
                    .filter(|file| file.is_conflicted())
                    .count(),
            },
        })
    }
}

impl Project {
    pub(super) fn clear_git_repository_data(&mut self) {
        self.git_root = None;
        self.git_branch = None;
        self.git_branches.clear();
        self.git_tracking = None;
        self.git_main_comparison = Ok(None);
        self.git_changes.clear();
        self.git_untracked_collapsed = false;
        self.explorer_git = None;
        self.git_stats = git::WorkingTreeStats::default();
        self.git_stats_error = None;
        self.recent_commits.clear();
        self.git_history_snapshot = None;
        self.git_history_loading = false;
        self.git_history_has_more = false;
        self.git_history_generation = 0;
        self.git_history_refresh_requested = false;
        self.git_history_error = None;
        let split_ratio = self.git_view.split_ratio;
        self.git_view = git_panel::GitViewState::default();
        self.git_view.split_ratio = split_ratio;
        self.git_error = None;
        self.git_indicator_count = None;
        self.git_summary = None;
        self.git_head = None;
        self.git_fingerprint = None;
    }

    pub(super) fn uncommitted_change_count(&self) -> Option<usize> {
        self.git_indicator_count
    }

    /// Branch and upstream line for the project tab tooltip, from the same
    /// refreshes as its change dot.
    pub(super) fn git_tab_summary(&self) -> Option<String> {
        let status = &self.git_summary.as_ref()?.status;
        let tracking = &status.tracking;
        let mut line = match (&tracking.name, &status.head) {
            (Some(branch), _) => {
                let upstream = match &tracking.upstream {
                    None => "no upstream".to_owned(),
                    Some(_) if !tracking.divergence_known => "↑? ↓?".to_owned(),
                    Some(_) if tracking.ahead == 0 && tracking.behind == 0 => {
                        "up to date".to_owned()
                    }
                    Some(_) => format!("↑{} ↓{}", tracking.ahead, tracking.behind),
                };
                format!("Branch {branch} · {upstream}")
            }
            (None, Some(head)) => format!("Detached at {}", head.get(..7).unwrap_or(head)),
            (None, None) => return None,
        };
        if status.conflicts > 0 {
            line.push_str(&format!(
                " · {} {}",
                status.conflicts,
                if status.conflicts == 1 {
                    "conflict"
                } else {
                    "conflicts"
                }
            ));
        }
        Some(line)
    }
}

/// One status process per refresh: the change count of the tab's dot plus
/// the branch and upstream divergence shown in tooltips and the overview.
fn read_git_summary(
    root: &Path,
    approved_parent: Option<&Path>,
) -> Result<Option<GitSummary>, String> {
    let repository = git::project_repository(root, approved_parent)
        .map_err(|error| error.to_string())?
        .root;
    repository
        .map(|repository| {
            git::worktree_status(&repository)
                .map(|status| GitSummary { repository, status })
                .map_err(|error| error.to_string())
        })
        .transpose()
}

impl VEditApp {
    pub(super) fn set_git_parent_repository(&mut self, index: usize, use_parent: bool) {
        if self.git_busy.is_some() {
            return;
        }
        let Some(project) = self.projects.get_mut(index) else {
            return;
        };
        let Some(parent) = project.git_parent_root.clone() else {
            return;
        };
        project.git_approved_parent = use_parent.then_some(parent);
        project.clear_git_repository_data();
        // Old workers must not restore the previous repository after this choice.
        project.git_status_request = None;
        project.git_indicator_request = None;
        project.git_loading = false;
        project.git_refresh_pending = false;
        self.persist_session();
        self.refresh_git(index, true);
    }

    fn inactive_git_indicator_candidates(&self) -> Vec<usize> {
        if self.git_busy.is_some() {
            return Vec::new();
        }
        let mut candidates: Vec<_> = self
            .projects
            .iter()
            .enumerate()
            .filter(|(index, project)| {
                *index != self.active_project
                    && !project.git_loading
                    && project.git_indicator_request.is_none()
                    && project.last_git_refresh.elapsed() >= REFRESH_INTERVAL
                    && project.last_git_indicator_refresh.elapsed() >= REFRESH_INTERVAL
            })
            .map(|(index, project)| (project.last_git_indicator_refresh, index))
            .collect();
        // Oldest first: many projects cannot starve behind a fixed index order.
        candidates.sort_unstable();
        candidates
            .into_iter()
            .take(MAX_REQUESTS.saturating_sub(self.git_indicator_requests.len()))
            .map(|(_, index)| index)
            .collect()
    }

    pub(super) fn refresh_inactive_git_indicators(&mut self) {
        static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
        for index in self.inactive_git_indicator_candidates() {
            let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
            let project = &mut self.projects[index];
            let root = project.root.clone();
            let approved_parent = project.git_approved_parent.clone();
            project.git_indicator_request = Some(request_id);
            project.last_git_indicator_refresh = Instant::now();
            // Keep removed projects' workers in the budget until they finish.
            self.git_indicator_requests.insert(request_id, root.clone());
            self.worker(move || WorkerResult::GitIndicator {
                result: read_git_summary(&root, approved_parent.as_deref()),
                root,
                request_id,
            });
        }
    }

    pub(super) fn finish_git_indicator(
        &mut self,
        root: PathBuf,
        request_id: u64,
        result: Result<Option<GitSummary>, String>,
    ) {
        if self.git_indicator_requests.remove(&request_id).as_ref() != Some(&root) {
            return;
        }
        let Some(project) = self.projects.iter_mut().find(|project| {
            project.root == root && project.git_indicator_request == Some(request_id)
        }) else {
            return;
        };
        project.git_indicator_request = None;
        project.last_git_indicator_refresh = Instant::now();
        // Failure means unknown, rather than a misleading clean status. A later
        // refresh retries without replacing history, file rows or diff statistics.
        let summary = result.ok().flatten();
        project.git_indicator_count = summary.as_ref().map(|summary| summary.status.changes);
        project.git_summary = summary;
    }
}

#[cfg(test)]
#[path = "project_status_tests.rs"]
mod tests;
