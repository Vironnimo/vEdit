#!/usr/bin/env python
"""Exercise repository Git tooling only in disposable .tmp/worktree-tools fixtures.

Run through the project-temp.ps1 environment, as documented in development/testing.md.
No Rust compilation, application startup, network or real-repository mutation occurs.
Successful fixtures are removed through checked, explicitly owned paths. Command
logs and failed fixtures remain below .tmp for inspection.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import unittest
from pathlib import Path
from uuid import uuid4

sys.dont_write_bytecode = True
from worktree import is_link, validate_removal


ROOT = Path(__file__).resolve().parents[1]
RUN = ROOT / ".tmp" / "worktree-tools" / uuid4().hex[:12]
HOOK = '''from pathlib import Path
import os
import subprocess
import sys
print("Commit check: fixture static check")
root = Path(os.environ["VORCH_TEST_PRIMARY"])
environment = dict(os.environ)
for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"):
    environment.pop(key, None)
if Path.cwd().name.startswith(".landing-"):
    mode = os.environ.get("VORCH_TEST_HOOK", "")
    if mode == "fail":
        sys.exit(1)
    if mode in ("edit-source", "commit-source"):
        source = root / ".worktrees" / "task"
        (source / "concurrent.txt").write_text("concurrent work\\n", encoding="utf-8")
        if mode == "commit-source":
            subprocess.run(["git", "add", "concurrent.txt"], cwd=source, env=environment, check=True)
            subprocess.run(["git", "-c", "core.hooksPath=.tmp/no-hooks", "commit", "-m", "concurrent"], cwd=source, env=environment, check=True)
    if mode == "advance-main" and not (root / ".tmp" / "advanced").exists():
        (root / ".tmp" / "advanced").write_text("once", encoding="utf-8")
        (root / "other.txt").write_text("concurrent main commit\\n", encoding="utf-8")
        subprocess.run(["git", "add", "other.txt"], cwd=root, env=environment, check=True)
        subprocess.run(["git", "-c", "core.hooksPath=.tmp/no-hooks", "commit", "-m", "main advanced"], cwd=root, env=environment, check=True)
'''


class GitToolContracts(unittest.TestCase):
    def setUp(self) -> None:
        case = hashlib.sha256(self._testMethodName.encode("utf-8")).hexdigest()[:8]
        self.root = RUN / case / "repo with spaces and \u00fc"
        self.root.mkdir(parents=True)
        self.owned_roots = [self.root]
        self.command_log: list[str] = [f"Test: {self.id()}"]
        self.addCleanup(self.cleanup_fixture)
        temporary = self.root / ".tmp"
        temporary.mkdir()
        self.env = {
            key: value for key, value in os.environ.items()
            if not key.startswith("GIT_") and key not in ("VORCH_TEST_HOOK",)
        }
        self.env.update({
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": str(temporary / "empty-gitconfig"),
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_CEILING_DIRECTORIES": str(RUN),
            "GIT_AUTHOR_NAME": "Fixture User", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
            "GIT_COMMITTER_NAME": "Fixture User", "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
            "VORCH_TEST_PRIMARY": str(self.root),
            **{key: str(temporary) for key in ("TEMP", "TMP", "TMPDIR")},
        })
        self.run_git("-c", "core.longpaths=true", "init", "-b", "main")
        self.run_git("config", "--local", "core.longpaths", "true")
        self.run_git("config", "--local", "core.autocrlf", "false")
        self.run_git("config", "--local", "user.name", "Fixture User")
        self.run_git("config", "--local", "user.email", "fixture@example.invalid")
        (self.root / "scripts").mkdir()
        for script in ("worktree.py", "commit_authorship.py"):
            shutil.copyfile(ROOT / "scripts" / script, self.root / "scripts" / script)
        shutil.copytree(ROOT / ".githooks", self.root / ".githooks")
        self.write("scripts/commit_check.py", HOOK)
        self.write(".gitignore", ".tmp/\n.worktrees/\n.vorch-worktree\n")
        self.write("task.txt", "base task\n")
        self.write("other.txt", "base other\n")
        self.run_git("add", ".")
        self.run_git("update-index", "--chmod=+x", ".githooks/pre-commit",
                     ".githooks/pre-merge-commit", ".githooks/commit-msg", ".githooks/pre-push")
        self.run_git("commit", "-m", "fixture baseline")
        self.run_git("config", "--local", "core.hooksPath", ".githooks")

    def cleanup_fixture(self) -> None:
        (self.root.parent / "commands.log").write_text(
            "\n".join(self.command_log), encoding="utf-8"
        )
        result = self._outcome.result
        if any(test is self for test, _ in result.errors + result.failures):
            print(f"Failed fixture kept: {self.root}", flush=True)
            return
        for owned in sorted(set(self.owned_roots), key=lambda path: len(path.parts), reverse=True):
            if not owned.exists():
                continue
            resolved = owned.resolve()
            self.assertTrue(resolved.is_relative_to(RUN.resolve()) and resolved != RUN.resolve())
            for part in (owned, *owned.parents):
                self.assertFalse(is_link(part), f"refusing fixture cleanup through a link: {part}")
                if part == RUN:
                    break
            validate_removal(owned)
            def readonly_git_file(function, filename, error_info):
                failure = error_info[1]
                target = Path(filename)
                if (not isinstance(failure, PermissionError)
                        or function not in (os.unlink, os.remove, os.rmdir)
                        or is_link(target)
                        or not target.resolve().is_relative_to(resolved)):
                    raise failure
                target.chmod(target.stat().st_mode | stat.S_IWUSR)
                function(filename)

            shutil.rmtree(owned, onerror=readonly_git_file)

    def invoke(self, command: list[str], cwd: Path | None = None, expected: int = 0):
        result = subprocess.run(command, cwd=cwd or self.root, env=self.env,
                                stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                encoding="utf-8", errors="replace", timeout=30)
        self.command_log.append(
            f"cwd: {cwd or self.root}\ncommand: {command!r}\nexit: {result.returncode}\n"
            f"{result.stdout}{result.stderr}"
        )
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result

    def run_git(self, *args: str, cwd: Path | None = None, expected: int = 0) -> str:
        return self.invoke(["git", *args], cwd, expected).stdout.strip()

    def cli(self, *args: str, expected: int = 0) -> str:
        output = self.invoke([sys.executable, str(self.root / "scripts/worktree.py"), *args],
                             expected=expected).stdout
        if args[0] == "create" and expected == 0:
            self.owned_roots.append(self.root / ".worktrees" / args[1])
        return output

    def write(self, name: str, content: str, root: Path | None = None) -> None:
        (root or self.root).joinpath(name).write_text(content, encoding="utf-8")

    def task(self) -> Path:
        output = self.cli("create", "task")
        self.assertIn("branch: codex/task", output)
        path = self.root / ".worktrees/task"
        self.write("task.txt", "task change\n", path)
        self.run_git("add", "task.txt", cwd=path)
        self.run_git("commit", "-m", "task change", cwd=path)
        return path

    def test_create_list_delete_and_invalid_names(self) -> None:
        self.assertIn("status: empty", self.cli("list"))
        self.assertIn("status: created", self.cli("create", "first"))
        self.cli("create", "second")
        listed = self.cli("list")
        self.assertIn("name: first", listed)
        self.assertIn("name: second", listed)
        marker = json.loads((self.root / ".worktrees/first/.vorch-worktree").read_text())
        self.assertEqual(marker["branch"], "codex/first")
        self.assertIsNone(marker["port"])
        self.cli("create", "../outside", expected=1)
        self.cli("create", "con", expected=1)
        self.cli("delete", "missing", expected=1)
        self.cli("unknown", expected=2)
        self.cli("delete", "first")
        self.cli("delete", "second")
        self.assertIn("status: empty", self.cli("list"))

    def test_merge_preserves_unrelated_primary_edits(self) -> None:
        path = self.task()
        self.write("other.txt", "unsaved primary\n")
        output = self.cli("merge", "task")
        self.assertIn("Commit check:", output)
        self.assertIn("status: merged", output)
        self.assertEqual((self.root / "other.txt").read_text(), "unsaved primary\n")
        self.assertEqual((self.root / "task.txt").read_text(), "task change\n")
        self.assertFalse(path.exists())
        self.assertEqual(len(self.run_git("rev-list", "--parents", "-n", "1", "HEAD").split()), 3)

    def test_overlap_and_failed_hook_leave_primary_unchanged(self) -> None:
        self.task()
        before = self.run_git("rev-parse", "HEAD")
        self.write("task.txt", "unsaved overlapping work\n")
        self.assertIn("could not be fast-forwarded", self.cli("merge", "task", expected=1))
        self.assertEqual(self.run_git("rev-parse", "HEAD"), before)
        self.assertEqual((self.root / "task.txt").read_text(), "unsaved overlapping work\n")
        self.run_git("restore", "task.txt")
        self.env["VORCH_TEST_HOOK"] = "fail"
        self.assertIn("failed the commit check", self.cli("merge", "task", expected=1))
        self.assertEqual(self.run_git("rev-parse", "HEAD"), before)

    def test_conflict_report_does_not_change_primary(self) -> None:
        self.task()
        self.write("task.txt", "conflicting primary commit\n")
        self.run_git("add", "task.txt")
        self.run_git("commit", "-m", "primary change")
        before = self.run_git("rev-parse", "HEAD")
        self.assertIn("conflicted: task.txt", self.cli("merge", "task", expected=2))
        self.assertEqual(self.run_git("rev-parse", "HEAD"), before)
        self.assertEqual(self.run_git("status", "--porcelain"), "")

    def test_concurrent_source_edit_keeps_source(self) -> None:
        path = self.task()
        self.env["VORCH_TEST_HOOK"] = "edit-source"
        self.assertIn("source worktree kept", self.cli("merge", "task"))
        self.assertEqual((path / "concurrent.txt").read_text(), "concurrent work\n")

    def test_concurrent_source_commit_keeps_branch(self) -> None:
        path = self.task()
        self.env["VORCH_TEST_HOOK"] = "commit-source"
        self.assertIn("source worktree kept", self.cli("merge", "task"))
        self.assertEqual(self.run_git("log", "-1", "--format=%s", "codex/task"), "concurrent")
        self.assertTrue(path.exists())

    def test_concurrent_primary_commit_rechecks_merge(self) -> None:
        self.task()
        self.env["VORCH_TEST_HOOK"] = "advance-main"
        output = self.cli("merge", "task")
        self.assertIn("merging again", output)
        self.assertEqual(output.count("Commit check:"), 2)
        self.assertEqual((self.root / "other.txt").read_text(), "concurrent main commit\n")

    def test_delete_protects_pending_work_and_markers(self) -> None:
        path = self.task()
        self.assertIn("unmerged:", self.cli("delete", "task", expected=1))
        self.write("notes.txt", "untracked work\n", path)
        self.cli("delete", "task", expected=1)
        marker_path = path / ".vorch-worktree"
        marker = marker_path.read_text()
        marker_path.write_text('{"name":"task","branch":"main"}', encoding="utf-8")
        self.assertIn("mismatched marker", self.cli("delete", "task", "--force", expected=1))
        marker_path.write_text(marker, encoding="utf-8")
        self.cli("delete", "task", "--force")
        self.assertFalse(path.exists())

    def test_merge_lock_and_unexpected_branches_preserve_work(self) -> None:
        path = self.task()
        lock = self.root / ".worktrees" / ".operations.lock"
        with lock.open("r+b") as handle:
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assertIn("stayed busy", self.cli("merge", "task", "--wait-timeout", "0", expected=1))
        self.run_git("switch", "-c", "other-task", cwd=path)
        self.assertIn("expected 'codex/task'", self.cli("delete", "task", "--force", expected=1))
        self.assertTrue(path.exists())
        self.run_git("switch", "codex/task", cwd=path)
        self.run_git("switch", "-c", "other-primary")
        before = self.run_git("rev-parse", "HEAD")
        self.assertIn("switch it back first", self.cli("merge", "task", expected=1))
        self.assertEqual(self.run_git("rev-parse", "HEAD"), before)

    def test_junction_and_nested_repository_are_not_deleted(self) -> None:
        path = self.task()
        target = self.root / ".tmp" / "outside-target"
        target.mkdir()
        (target / "keep.txt").write_text("keep", encoding="utf-8")
        link = path / "linked"
        if os.name == "nt":
            escaped_link = str(link).replace("'", "''")
            escaped_target = str(target).replace("'", "''")
            self.invoke(["powershell", "-NoProfile", "-Command",
                         f"New-Item -ItemType Junction -Path '{escaped_link}' -Target '{escaped_target}' | Out-Null"])
        else:
            link.symlink_to(target, target_is_directory=True)
        self.assertIn("symlink or junction", self.cli("delete", "task", "--force", expected=1))
        self.assertEqual((target / "keep.txt").read_text(), "keep")
        if os.name == "nt":
            os.rmdir(link)  # Removes the junction itself; never recurse into its target.
        else:
            link.unlink()
        nested = path / "nested"
        nested.mkdir()
        self.owned_roots.append(nested)
        self.run_git("-c", "core.longpaths=true", "init", cwd=nested)
        self.assertIn("nested Git repository", self.cli("delete", "task", "--force", expected=1))
        self.assertTrue((nested / ".git").is_dir())

    def test_hooks_reject_tool_identity_and_clean_message(self) -> None:
        self.write("other.txt", "authorship test\n")
        self.run_git("add", "other.txt")
        self.env["GIT_AUTHOR_NAME"] = "Codex"
        before = self.run_git("rev-parse", "HEAD")
        blocked = self.invoke(["git", "commit", "-m", "blocked"], expected=1)
        self.assertIn("Commit blocked", blocked.stdout + blocked.stderr)
        self.assertEqual(self.run_git("rev-parse", "HEAD"), before)
        self.env["GIT_AUTHOR_NAME"] = "Fixture User"
        message = self.root / ".tmp" / "message.txt"
        message.write_text("real message\n\nCo-Authored-By: Codex <noreply@openai.com>\n", encoding="utf-8")
        self.run_git("commit", "--file", str(message))
        self.assertEqual(self.run_git("log", "-1", "--format=%B"), "real message")
        self.invoke([sys.executable, "scripts/commit_authorship.py", "history"])
        self.env["GIT_AUTHOR_NAME"] = "Codex"
        self.run_git("-c", "core.hooksPath=.tmp/no-hooks", "commit", "--allow-empty", "-m", "fixture violation")
        self.env["GIT_AUTHOR_NAME"] = "Fixture User"
        violation = self.run_git("rev-parse", "HEAD")
        history = self.invoke([sys.executable, "scripts/commit_authorship.py", "history"], expected=1)
        self.assertIn(violation[:12], history.stderr)
        pushed = subprocess.run(
            [sys.executable, "scripts/commit_authorship.py", "push", "origin"],
            cwd=self.root, env=self.env, capture_output=True, text=True,
            input=f"refs/heads/main {violation} refs/heads/main {'0' * 40}\n",
            encoding="utf-8", timeout=30,
        )
        self.assertEqual(pushed.returncode, 1, pushed.stdout + pushed.stderr)
        self.assertIn(violation[:12], pushed.stderr)


if __name__ == "__main__":
    print(f"Git tooling fixtures: {RUN}", flush=True)
    unittest.main(verbosity=2)
