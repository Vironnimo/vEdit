//! Native file dialogs, Save As destinations and picker request ownership.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DialogPurpose {
    Projects,
    Files,
    SaveAs(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Picker {
    pub(super) id: u64,
    pub(super) purpose: DialogPurpose,
}

pub(super) struct DialogSelection {
    pub(super) paths: Vec<PathBuf>,
    pub(super) overwrite: Option<workspace::FileFingerprint>,
}

impl VEditApp {
    pub(super) fn pick_file(&mut self) {
        let directory = self
            .tab()
            .and_then(|tab| tab.path.as_deref())
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.active_folder().map(Path::to_path_buf));
        self.start_native_dialog(DialogPurpose::Files, directory, None);
    }
    pub(super) fn pick_project(&mut self) {
        let dir = self
            .project()
            .filter(|p| !p.is_other_files())
            .map(|p| p.root.parent().unwrap_or(&p.root).to_path_buf())
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("C:\\"));
        self.start_native_dialog(DialogPurpose::Projects, Some(dir), None);
    }
    pub(super) fn request_save_as(&mut self, id: u64) {
        if self
            .tab_by_id_mut(id)
            .is_some_and(|tab| tab.loading || tab.saving)
        {
            self.status("Wait for this file to finish loading or saving.", false);
            return;
        }
        let Some((directory, title, unnamed)) = self.projects.iter().find_map(|project| {
            project.tabs.iter().find(|tab| tab.id == id).map(|tab| {
                (
                    tab.path
                        .as_deref()
                        .and_then(Path::parent)
                        .or_else(|| project.folder_root())
                        .map(Path::to_path_buf)
                        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from)),
                    tab.title.clone(),
                    tab.path.is_none(),
                )
            })
        }) else {
            return;
        };
        if self.picker.is_some() {
            if self.save_as != Some(id) {
                if unnamed && !self.save_queue.contains(&id) {
                    self.save_queue.push_back(id);
                } else if !unnamed {
                    self.status("Finish the current file dialog before choosing another Save As destination.", false);
                }
            }
            return;
        }
        self.save_as = Some(id);
        self.start_native_dialog(DialogPurpose::SaveAs(id), directory, Some(title));
    }
    fn start_native_dialog(
        &mut self,
        purpose: DialogPurpose,
        directory: Option<PathBuf>,
        file_name: Option<String>,
    ) {
        if self.picker.is_some() {
            return;
        }
        static NEXT_DIALOG: AtomicU64 = AtomicU64::new(1);
        let request = Picker {
            id: NEXT_DIALOG.fetch_add(1, Ordering::Relaxed),
            purpose,
        };
        self.picker = Some(request);
        self.completions = None;
        self.palette = None;
        let Some(mut dialog) = self.native_dialog_builder.clone() else {
            // In-memory test apps inject the dialog completion without opening an OS window.
            return;
        };
        dialog = dialog.set_title(match purpose {
            DialogPurpose::Projects => "Open project folders",
            DialogPurpose::Files => "Open files",
            DialogPurpose::SaveAs(_) => "Save file as",
        });
        if let Some(directory) = directory {
            dialog = dialog.set_directory(crate::paths::display(&directory));
        }
        if let Some(file_name) = file_name {
            dialog = dialog.set_file_name(file_name);
        }
        self.worker(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let paths = match purpose {
                    DialogPurpose::Projects => dialog.pick_folders(),
                    DialogPurpose::Files => dialog.pick_files(),
                    DialogPurpose::SaveAs(_) => dialog.save_file().map(|path| vec![path]),
                };
                let Some(paths) = paths else { return Ok(None) };
                let paths: Vec<_> = paths
                    .into_iter()
                    .map(|path| fs::canonicalize(&path).unwrap_or(path))
                    .collect();
                let overwrite = if matches!(purpose, DialogPurpose::SaveAs(_)) {
                    match fs::symlink_metadata(&paths[0]) {
                        Ok(_) => Some(workspace::fingerprint(&paths[0], MAX_DOCUMENT)?),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(error) => return Err(error.to_string()),
                    }
                } else {
                    None
                };
                Ok(Some(DialogSelection { paths, overwrite }))
            }))
            .unwrap_or_else(|_| {
                Err("The Windows file dialog could not be opened. Try again.".into())
            });
            WorkerResult::NativeDialog { request, result }
        });
    }
    pub(super) fn finish_native_dialog(
        &mut self,
        request: Picker,
        result: Result<Option<DialogSelection>, String>,
    ) {
        if self.picker != Some(request) {
            return;
        }
        self.picker = None;
        if matches!(request.purpose, DialogPurpose::SaveAs(_)) {
            self.save_as = None;
        }
        match result {
            Ok(Some(selection)) => {
                match request.purpose {
                    DialogPurpose::Projects => {
                        for path in selection.paths {
                            self.add_project(path);
                        }
                    }
                    DialogPurpose::Files => {
                        for path in selection.paths {
                            self.open_argument(path);
                        }
                    }
                    DialogPurpose::SaveAs(id) => {
                        let Some(path) = selection.paths.into_iter().next() else {
                            return;
                        };
                        if self
                            .projects
                            .iter()
                            .flat_map(|project| &project.tabs)
                            .any(|tab| tab.id != id && tab.path.as_ref() == Some(&path))
                        {
                            self.save_queue.clear();
                            self.close_after_save = false;
                            self.status("This file is already open in another tab. Choose a different name.", true);
                            return;
                        }
                        self.save_tab_expected(id, Some(path), selection.overwrite);
                    }
                }
            }
            Ok(None) => {
                if matches!(request.purpose, DialogPurpose::SaveAs(_)) {
                    self.save_queue.clear();
                    self.close_after_save = false;
                }
            }
            Err(error) => {
                self.save_queue.clear();
                self.close_after_save = false;
                self.status(error, true);
            }
        }
        if let Some(tab) = self.tab_mut() {
            tab.focus = tab.agent_view == crate::cli::DocumentView::Source;
        }
    }
}
