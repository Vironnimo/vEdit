//! Small status-only Git refreshes for inactive project tabs.
use super::*;

const REFRESH_INTERVAL: Duration = Duration::from_secs(15);
const MAX_REQUESTS: usize = 2;

impl Project {
    pub(super) fn uncommitted_change_count(&self) -> Option<usize> {
        self.git_indicator_count
    }
}

fn read_uncommitted_count(root: &Path) -> Result<Option<usize>, String> {
    let repository = git::repository_root(root).map_err(|error| error.to_string())?;
    repository
        .map(|repository| {
            git::uncommitted_change_count(&repository).map_err(|error| error.to_string())
        })
        .transpose()
}

impl VEditApp {
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
            project.git_indicator_request = Some(request_id);
            project.last_git_indicator_refresh = Instant::now();
            // Keep removed projects' workers in the budget until they finish.
            self.git_indicator_requests.insert(request_id, root.clone());
            self.worker(move || WorkerResult::GitIndicator {
                result: read_uncommitted_count(&root),
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
