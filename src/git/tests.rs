use super::*;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_REPOSITORY: AtomicU64 = AtomicU64::new(0);

struct TestRepository(PathBuf);

impl TestRepository {
    fn empty_directory() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "vedit-git-tests-{}-{timestamp}-{} with spaces",
            std::process::id(),
            NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn new() -> Self {
        let repository = Self::empty_directory();
        initialize(&repository.0).unwrap();
        repository.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
        let hooks = repository.0.join("test-hooks");
        fs::create_dir(&hooks).unwrap();
        // This fixture owns its new repository. Preserve init's config and
        // add all local settings in one write instead of five Git processes.
        let config_path = repository.0.join(".git/config");
        let mut config = fs::read_to_string(&config_path).unwrap();
        let hooks = hooks
            .to_str()
            .unwrap()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        config.push_str(&format!(
            "\n[user]\n\tname = vEdit Git Test\n\temail = vedit-tests@example.invalid\n[commit]\n\tgpgsign = false\n[core]\n\tautocrlf = false\n\thooksPath = \"{hooks}\"\n"
        ));
        fs::write(config_path, config).unwrap();
        fs::write(
            repository.0.join(".gitignore"),
            "test-hooks/\nhook-completed.txt\n",
        )
        .unwrap();
        repository
    }

    fn git(&self, args: &[&str]) -> Output {
        checked_git(&self.0, &string_args(args), "test fixture").unwrap()
    }

    fn write(&self, path: &str, contents: &str) {
        fs::write(self.0.join(path), contents).unwrap();
    }

    fn commit_all(&self, message: &str) {
        stage_all(&self.0).unwrap();
        commit(&self.0, message).unwrap();
    }

    fn head_hash(&self) -> String {
        String::from_utf8(self.git(&["rev-parse", "--verify", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }
}

impl Drop for TestRepository {
    fn drop(&mut self) {
        // Only remove the uniquely named directory created by this fixture.
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vedit-git-tests-")
        );
        for _ in 0..3 {
            if fs::remove_dir_all(&self.0).is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

mod changes;
mod history;
mod process;
#[cfg(windows)]
mod process_witness;
mod repository;
mod review;
