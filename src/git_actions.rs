//! Git mutations, cancellation and typed completion follow-up.
use super::*;

impl VEditApp {
    pub(super) fn finish_git_action(
        &mut self,
        reply: Option<Sender<crate::cli::Response>>,
        root: PathBuf,
        kind: GitActionKind,
        result: Result<String, String>,
    ) {
        self.git_busy = None;
        self.git_action = None;
        if let Some(reply) = reply {
            let response = match &result {
                Ok(branch) => crate::cli::Response::success(
                    serde_json::json!({"repository":crate::paths::display(&root),"branch":branch,"completed":true}),
                ),
                Err(error) => crate::cli::Response::failure("git_switch_failed", error.clone()),
            };
            let _ = reply.send(response);
        }
        match result {
            Ok(_) => {
                self.status(format!("{} completed", kind.label()), false);
                // Commits, fetches and branch switches move other
                // worktrees' upstream divergence and listings too;
                // staging stays inside one worktree.
                if !matches!(kind, GitActionKind::Stage | GitActionKind::Unstage) {
                    self.repositories.request_refresh();
                }
                let indices: Vec<_> = self
                    .projects
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.git_root.as_ref() == Some(&root) || p.root == root)
                    .map(|(index, _)| index)
                    .collect();
                for i in indices {
                    if kind == GitActionKind::Commit {
                        self.projects[i].commit_message.clear();
                    }
                    if kind == GitActionKind::SwitchBranch {
                        self.projects[i].git_history_refresh_requested = true;
                    }
                    self.refresh_git(i, true);
                    self.index_project(i);
                }
            }
            Err(e) => self.status(e, true),
        }
    }
}

#[derive(Clone)]
pub(super) enum GitAction {
    Initialize,
    StageAll,
    UnstageAll,
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    Commit(String),
    Fetch,
    Pull,
    Push,
    SwitchBranch(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GitActionKind {
    Initialize,
    Stage,
    Unstage,
    Commit,
    Fetch,
    Pull,
    Push,
    SwitchBranch,
}

impl GitActionKind {
    fn label(self) -> &'static str {
        match self {
            Self::Initialize => "Initialize repository",
            Self::Stage => "Stage changes",
            Self::Unstage => "Unstage changes",
            Self::Commit => "Create commit",
            Self::Fetch => "Fetch",
            Self::Pull => "Pull",
            Self::Push => "Push",
            Self::SwitchBranch => "Switch branch",
        }
    }
}

impl GitAction {
    fn kind(&self) -> GitActionKind {
        match self {
            Self::Initialize => GitActionKind::Initialize,
            Self::StageAll | Self::Stage(_) => GitActionKind::Stage,
            Self::UnstageAll | Self::Unstage(_) => GitActionKind::Unstage,
            Self::Commit(_) => GitActionKind::Commit,
            Self::Fetch => GitActionKind::Fetch,
            Self::Pull => GitActionKind::Pull,
            Self::Push => GitActionKind::Push,
            Self::SwitchBranch(_) => GitActionKind::SwitchBranch,
        }
    }
    fn label(&self) -> &'static str {
        self.kind().label()
    }
    /// Actions that rewrite working-tree files. Saving into the repository
    /// while one runs could interleave with Git's checkout.
    fn rewrites_working_tree(&self) -> bool {
        matches!(self, Self::Pull | Self::SwitchBranch(_))
    }
    fn run(&self, root: &Path) -> Result<String, String> {
        match self {
            Self::Initialize => git::initialize(root).map(|_| "Repository initialized".into()),
            Self::StageAll => git::stage_all(root).map(|_| "Changes staged".into()),
            Self::UnstageAll => git::unstage_all(root).map(|_| "Changes unstaged".into()),
            Self::Stage(paths) => git::stage_paths(root, paths).map(|_| "Changes staged".into()),
            Self::Unstage(paths) => {
                git::unstage_paths(root, paths).map(|_| "Changes unstaged".into())
            }
            Self::Commit(message) => git::commit(root, message),
            Self::Fetch => git::fetch(root),
            Self::Pull => git::pull(root),
            Self::Push => git::push(root),
            Self::SwitchBranch(target) => git::switch_branch(root, target),
        }
        .map_err(|e| e.to_string())
    }
}

/// The Git action that currently owns `git_busy`.
pub(super) struct ActiveGitAction {
    pub(super) root: PathBuf,
    pub(super) label: &'static str,
    pub(super) cancellation: git::Cancellation,
    pub(super) rewrites_working_tree: bool,
}

impl ActiveGitAction {
    /// A spinner, the action and its Cancel button. Returns whether the user
    /// asked to stop the action.
    pub(super) fn progress_ui(&self, ui: &mut egui::Ui, size: f32) -> bool {
        let stopping = self.cancellation.is_cancelled();
        let label = if stopping {
            format!("Stopping {}…", self.label.to_lowercase())
        } else {
            format!("{}…", self.label)
        };
        ui.add(egui::Spinner::new().size(size).color(MUTED));
        ui.add(egui::Label::new(RichText::new(label).size(size).color(MUTED)).truncate())
            .on_hover_details(format!(
                "Git is working in {}",
                crate::paths::display(&self.root)
            ));
        !stopping
            && design::status_item(
                ui,
                "Cancel",
                ORANGE,
                "Stop Git and the hooks it started. An interrupted action can leave the repository partly updated.",
            )
            .clicked()
    }
}

impl VEditApp {
    pub(super) fn git_action(&mut self, root: PathBuf, action: GitAction) {
        if let Err(error) = self.start_git_action(root, action, None) {
            self.status(error, true);
        }
    }
    pub(super) fn start_git_action(
        &mut self,
        root: PathBuf,
        action: GitAction,
        reply: Option<Sender<crate::cli::Response>>,
    ) -> Result<(), String> {
        if self.git_busy.is_some() {
            return Err("A Git operation is still running. No new operation was started.".into());
        }
        if matches!(action, GitAction::SwitchBranch(_)) {
            if self.projects.iter().any(|project| {
                project.tabs.iter().any(|tab| {
                    (tab.dirty || tab.saving || tab.loading || tab.agent_unsaved)
                        && tab.path.as_ref().map_or(
                            project.git_root.as_ref() == Some(&root) || project.root == root,
                            |path| context_actions::path_contains(&root, path),
                        )
                })
            }) {
                return Err("Save or close unsaved documents and wait for file operations before switching branches. No branch was switched.".into());
            }
            if self.control_dialog().is_some() {
                return Err(
                    "Finish the open dialog before switching branches. No branch was switched."
                        .into(),
                );
            }
        }
        self.git_busy = Some(root.clone());
        // Status reads use GIT_OPTIONAL_LOCKS=0 and need not hold up an action.
        // Discard their results, which may describe the repository before or
        // halfway through the action. Keep the last complete view until the
        // post-action refresh arrives; late workers cannot overwrite it.
        for project in &mut self.projects {
            if project.git_root.as_ref() == Some(&root) || project.root == root {
                project.git_status_request = None;
                project.git_indicator_request = None;
                project.git_loading = false;
                project.git_refresh_pending = false;
                project.last_git_refresh = Instant::now() - Duration::from_secs(30);
            }
        }
        let kind = action.kind();
        let cancellation = git::Cancellation::default();
        self.git_action = Some(ActiveGitAction {
            root: root.clone(),
            label: action.label(),
            cancellation: cancellation.clone(),
            rewrites_working_tree: action.rewrites_working_tree(),
        });
        // The running action and its Cancel button stay visible until it ends.
        self.status = None;
        let fallback = (root.clone(), kind, reply.clone());
        self.worker_or(
            move || WorkerResult::GitAction {
                result: git::cancellable(&cancellation, || action.run(&root)),
                root,
                kind,
                reply,
            },
            move |error| {
                let (root, kind, reply) = fallback;
                WorkerResult::GitAction {
                    reply,
                    root,
                    kind,
                    result: Err(error),
                }
            },
        );
        Ok(())
    }
    /// Stop the running Git action with its hooks and helpers. Git reports
    /// the result; a partly updated repository stays as it is.
    pub(super) fn cancel_git_action(&mut self) {
        if let Some(action) = &self.git_action
            && !action.cancellation.is_cancelled()
        {
            action.cancellation.cancel();
            let label = action.label;
            self.status(format!("Stopping {}…", label.to_lowercase()), false);
        }
    }
    /// The repository whose working tree a running pull or branch switch is
    /// rewriting. Saves into it wait until Git has finished.
    pub(super) fn git_checkout_root(&self) -> Option<&Path> {
        self.git_action
            .as_ref()
            .filter(|action| action.rewrites_working_tree)
            .map(|action| action.root.as_path())
    }
    /// Why a save to `path` must wait, while Git rewrites its working tree.
    pub(super) fn git_checkout_save_error(&self, path: &Path) -> Option<String> {
        let action = self.git_action.as_ref()?;
        (action.rewrites_working_tree && context_actions::path_contains(&action.root, path)).then(
            || {
                format!(
                    "Git is running {} in this repository. Nothing was saved; save after it finishes. Edits stay open.",
                    action.label.to_lowercase()
                )
            },
        )
    }
}
