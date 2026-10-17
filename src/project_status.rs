//! Project repository selection and status-only refreshes for inactive tabs.
use super::*;

const REFRESH_INTERVAL: Duration = Duration::from_secs(15);
const MAX_REQUESTS: usize = 2;

impl Project {
    pub(super) fn clear_git_repository_data(&mut self) {
        self.git_root = None;
        self.git_branch = None;
        self.git_branches.clear();
        self.git_tracking = None;
        self.git_main_comparison = Ok(None);
        self.git_changes.clear();
        self.explorer_git = None;
        self.git_stats = git::WorkingTreeStats::default();
        self.git_stats_error = None;
        self.recent_commits.clear();
        self.git_history_snapshot = None;
        self.git_history_loading = false;
        self.git_history_has_more = false;
        self.git_history_generation = 0;
        self.git_history_update_available = false;
        self.git_history_refresh_requested = false;
        self.git_history_error = None;
        let split_ratio = self.git_view.split_ratio;
        self.git_view = git_panel::GitViewState::default();
        self.git_view.split_ratio = split_ratio;
        self.git_error = None;
        self.git_indicator_count = None;
    }

    pub(super) fn uncommitted_change_count(&self) -> Option<usize> {
        self.git_indicator_count
    }
}

fn read_uncommitted_count(
    root: &Path,
    approved_parent: Option<&Path>,
) -> Result<Option<usize>, String> {
    let repository = git::project_repository(root, approved_parent)
        .map_err(|error| error.to_string())?
        .root;
    repository
        .map(|repository| {
            git::uncommitted_change_count(&repository).map_err(|error| error.to_string())
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
                result: read_uncommitted_count(&root, approved_parent.as_deref()),
                root,
                request_id,
            });
        }
    }

    pub(super) fn finish_git_indicator(
        &mut self,
        root: PathBuf,
        request_id: u64,
        result: Result<Option<usize>, String>,
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
        project.git_indicator_count = result.ok().flatten();
    }
}

#[cfg(test)]
#[path = "project_status_tests.rs"]
mod tests;
