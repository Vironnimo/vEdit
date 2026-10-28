#!/usr/bin/env python
"""Regression checks for staged-content safety and exact-commit push validation."""

from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import push


class RepositoryFixture(unittest.TestCase):
    def setUp(self):
        base = ROOT / ".tmp" / "commit-push"
        base.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=base)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.home = self.root / ".tmp" / "home"
        self.home.mkdir(parents=True)
        env = dict(os.environ)
        for key in list(env):
            if key.startswith("GIT_"):
                env.pop(key)
        env.pop("XDG_CONFIG_HOME", None)
        env.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                    "RUSTUP_HOME": str(Path(os.environ.get("RUSTUP_HOME", Path.home() / ".rustup"))),
                    "HOME": str(self.home), "USERPROFILE": str(self.home),
                    "TEMP": str(self.home), "TMP": str(self.home), "TMPDIR": str(self.home),
                    "GIT_CEILING_DIRECTORIES": str(base), "PYTHONDONTWRITEBYTECODE": "1"})
        self.env = env
        self.run_git("-c", "core.longpaths=true", "init", "-q", "-b", "main")
        self.run_git("config", "user.name", "Fixture User")
        self.run_git("config", "user.email", "fixture@example.invalid")
        self.run_git("config", "core.autocrlf", "false")
        self.run_git("config", "core.longpaths", "true")
        self.run_git("config", "core.hooksPath", "unused-hooks")
        self.write(".gitignore", ".tmp/\n")
        for name in ("commit_check.py", "domain_maps.py", "push.py", "worktree.py"):
            self.write("scripts/" + name, (ROOT / "scripts" / name).read_text(encoding="utf-8"))
        self.run_git("add", ".")
        self.run_git("commit", "-qm", "fixture")

    def run_git(self, *args):
        result = subprocess.run(["git", *args], cwd=self.root, env=self.env,
                                capture_output=True, check=True)
        return result.stdout

    def write(self, path, text):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open("w", encoding="utf-8", newline="") as handle:
            handle.write(text)

    def checker(self):
        result = subprocess.run([sys.executable, "-B", "scripts/commit_check.py", "--json"],
                                cwd=self.root, env=self.env, capture_output=True, check=False)
        self.assertEqual(result.stderr, b"")
        return result.returncode, json.loads(result.stdout)

    def test_partial_python_checks_index_and_preserves_both_versions(self):
        self.write("example.py", "value = 1\n")
        self.run_git("add", "example.py")
        self.write("example.py", "broken syntax !\n")
        before = self.run_git("show", ":example.py")
        self.assertEqual(self.checker()[0], 0)
        self.assertEqual(self.run_git("show", ":example.py"), before)
        self.assertEqual((self.root / "example.py").read_text(), "broken syntax !\n")
        self.run_git("add", "example.py")
        self.write("example.py", "value = 2\n")
        self.assertEqual(self.checker()[0], 1)
        self.assertFalse(list(self.root.rglob("__pycache__")))

    def test_rust_stdin_does_not_follow_or_rewrite_modules(self):
        self.write("src/main.rs", "mod child;\nfn main() {}\n")
        self.run_git("add", "src/main.rs")
        self.write("src/child.rs", "fn    untouched( ){}\n")
        self.write("src/main.rs", "mod child;\nfn unfinished(\n")
        before = self.run_git("show", ":src/main.rs")
        code, report = self.checker()
        self.assertEqual(code, 0, report)
        self.assertEqual(self.run_git("show", ":src/main.rs"), before)
        self.assertEqual((self.root / "src/child.rs").read_text(), "fn    untouched( ){}\n")
        self.write("src/main.rs", "fn   main( ){}\n")
        self.run_git("add", "src/main.rs")
        self.write("src/main.rs", "fn main() {}\n")
        self.assertEqual(self.checker()[0], 1)

    def test_index_map_references_ignore_untracked_and_deleted_working_files(self):
        self.write(".vorch/domain-maps/example.md", "# Example\n\n## References\n`notes.md`\n")
        self.run_git("add", ".vorch/domain-maps/example.md")
        self.write("notes.md", "# Notes\n")
        self.assertEqual(self.checker()[0], 1)
        self.run_git("add", "notes.md")
        (self.root / "notes.md").unlink()
        self.write(".vorch/domain-maps/example.md", "uncommitted \u2014 prose\n")
        self.assertEqual(self.checker()[0], 0)

    def test_maps_corpus_returns_failure_for_broken_reference(self):
        self.write(".vorch/domain-maps/example.md", "# Example\n\n## References\n`missing.md`\n")
        result = subprocess.run([sys.executable, "-B", "scripts/domain_maps.py"],
                                cwd=self.root, env=self.env, capture_output=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn(b"missing.md", result.stdout)

    def test_removed_or_renamed_reference_blocks_unchanged_map(self):
        self.write(".vorch/domain-maps/example.md", "# Example\n\n## References\n`notes.md`\n")
        self.write("notes.md", "# Notes\n")
        self.run_git("add", ".vorch/domain-maps/example.md", "notes.md")
        self.run_git("commit", "-qm", "valid reference")
        self.run_git("mv", "notes.md", "moved.md")
        code, report = self.checker()
        self.assertEqual(code, 1)
        self.assertTrue(any("notes.md" in error for error in report["errors"]))
        self.run_git("mv", "moved.md", "notes.md")
        self.run_git("rm", "notes.md")
        code, report = self.checker()
        self.assertEqual(code, 1)
        self.assertEqual(report["files"], [])
        self.assertTrue(any("notes.md" in error for error in report["errors"]))

    def test_no_push_checks_exact_main_without_remote_or_uncommitted_files(self):
        self.write("marker.txt", "committed")
        self.run_git("add", "marker.txt")
        self.run_git("commit", "-qm", "committed marker")
        commit = self.run_git("rev-parse", "HEAD").decode().strip()
        self.write("marker.txt", "replacement object")
        self.run_git("add", "marker.txt")
        tree = self.run_git("write-tree").decode().strip()
        replacement = self.run_git("commit-tree", tree, "-m", "replacement").decode().strip()
        self.run_git("replace", commit, replacement)
        self.write("marker.txt", "unfinished")
        command = [sys.executable, "-c", "from pathlib import Path; "
                   "assert Path('marker.txt').read_text() == 'committed'; "
                   "assert not Path('untracked.txt').exists()"]
        self.write("untracked.txt", "other work")
        config = (self.root / ".git/config").read_bytes()
        with patch.dict(os.environ, self.env, clear=True), patch.object(push, "STEPS", [push.Step("probe", command)]):
            result = push.validate(self.root, True, lambda _: None)
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["commit"], commit)
        self.assertFalse(result["pushed"])
        self.assertEqual((self.root / ".git/config").read_bytes(), config)
        self.assertEqual((self.root / "marker.txt").read_text(), "unfinished")
        self.assertEqual(len(self.run_git("worktree", "list", "--porcelain").split(b"worktree ")) - 1, 1)
        self.assertTrue(Path(result["logs"]).is_dir())

    def test_failed_setup_skips_dependants_and_runs_independent_check(self):
        steps = [push.Step("setup", [sys.executable, "-c", "raise SystemExit(1)"]),
                 push.Step("dependent", [sys.executable, "-c", "pass"], ("setup",)),
                 push.Step("independent", [sys.executable, "-c", "pass"])]
        with patch.dict(os.environ, self.env, clear=True), patch.object(push, "STEPS", steps):
            result = push.validate(self.root, True, lambda _: None)
        self.assertFalse(result["ok"])
        self.assertEqual([entry["status"] for entry in result["steps"]],
                         ["failed", "skipped", "passed"])

    def test_divergent_remote_prevents_checks_and_push(self):
        with patch.dict(os.environ, self.env, clear=True), \
             patch.object(push, "remote_is_behind", return_value=(False, "remote-hash")), \
             patch.object(push, "run_step") as runner:
            result = push.validate(self.root, False, lambda _: None)
        self.assertFalse(result["ok"])
        self.assertFalse(result["pushed"])
        runner.assert_not_called()
        self.assertIsNone(result["logs"])

    def test_push_creates_main_on_an_empty_remote_and_then_reports_it_present(self):
        remote = self.home / "remote.git"
        self.run_git("init", "-q", "--bare", str(remote))
        self.run_git("remote", "add", "origin", str(remote))
        commit = self.run_git("rev-parse", "HEAD").decode().strip()
        steps = [push.Step("probe", [sys.executable, "-c", "pass"])]
        with patch.dict(os.environ, self.env, clear=True), patch.object(push, "STEPS", steps):
            with patch.object(push, "primary_root", return_value=self.root), \
                 redirect_stdout(io.StringIO()) as output:
                self.assertEqual(push.main(["--json"]), 0)
            first = json.loads(output.getvalue())
            self.assertTrue(first["ok"], first)
            self.assertIn("main now tracks origin/main.", first["notes"])
            self.assertEqual(self.run_git("rev-parse", "--abbrev-ref", "main@{upstream}").strip(),
                             b"origin/main")
            self.run_git("branch", "--unset-upstream", "main")
            again = push.validate(self.root, False, lambda _: None)
            self.assertEqual(self.run_git("rev-parse", "--abbrev-ref", "main@{upstream}").strip(),
                             b"origin/main")
            # Keep explicit tracking, even if its ref is gone or it is incomplete.
            for merge in ("refs/heads/main", "refs/heads/missing", None):
                self.run_git("config", "branch.main.remote", ".")
                if merge is None:
                    self.run_git("config", "--unset", "branch.main.merge")
                else:
                    self.run_git("config", "branch.main.merge", merge)
                config = (self.root / ".git/config").read_bytes()
                preserved = push.validate(self.root, False, lambda _: None)
                self.assertTrue(preserved["ok"], preserved)
                self.assertEqual((self.root / ".git/config").read_bytes(), config)
        self.assertTrue(first["pushed"], first)
        self.assertEqual(self.run_git("--git-dir", str(remote), "rev-parse", "main").decode().strip(), commit)
        self.assertTrue(again["ok"], again)
        self.assertFalse(again["pushed"])

    def test_tracking_failure_preserves_the_successful_push_outcome(self):
        remote = self.home / "remote.git"
        self.run_git("init", "-q", "--bare", str(remote))
        self.run_git("remote", "add", "origin", str(remote))
        real_git = push.git

        def locked_config(*args, cwd):
            if args[0] == "branch":
                return subprocess.CompletedProcess(args, 1, "", "config is locked")
            return real_git(*args, cwd=cwd)

        with patch.dict(os.environ, self.env, clear=True), \
             patch.object(push, "STEPS", [push.Step("probe", [sys.executable, "-c", "pass"])]), \
             patch.object(push, "git", side_effect=locked_config), \
             patch.object(push, "primary_root", return_value=self.root), \
             redirect_stdout(io.StringIO()) as output:
            self.assertEqual(push.main(["--json"]), 1)
        result = json.loads(output.getvalue())
        self.assertFalse(result["ok"], result)
        self.assertTrue(result["pushed"], result)
        self.assertEqual(self.run_git("--git-dir", str(remote), "rev-parse", "main").decode().strip(),
                         result["commit"])
        self.assertIn("git branch --set-upstream-to=origin/main main", result["errors"][0])

    def test_report_survives_checkout_cleanup(self):
        command = [sys.executable, "-c", "from pathlib import Path; "
                   "p = Path('.tmp/test-survey/probe'); p.mkdir(parents=True); "
                   "(p / 'report.json').write_text('{}')"]
        with patch.dict(os.environ, self.env, clear=True), \
             patch.object(push, "STEPS", [push.Step("report", command)]):
            result = push.validate(self.root, True, lambda _: None)
        self.assertTrue(result["ok"], result)
        self.assertTrue((Path(result["logs"]) / "test-survey/probe/report.json").is_file())

    def test_cli_usage_error_is_json_without_stderr(self):
        for script in ("commit_check.py", "push.py"):
            result = subprocess.run([sys.executable, "-B", f"scripts/{script}",
                                     "--json", "--invalid-option"],
                                    cwd=self.root, env=self.env, capture_output=True, check=False)
            self.assertEqual(result.returncode, 2)
            self.assertEqual(result.stderr, b"")
            self.assertFalse(json.loads(result.stdout)["ok"])

    def test_cleanup_preserves_nested_repository(self):
        command = [sys.executable, "-c", "from pathlib import Path; "
                   "Path('.tmp/nested/.git').mkdir(parents=True)"]
        with patch.dict(os.environ, self.env, clear=True), \
             patch.object(push, "STEPS", [push.Step("nested", command)]):
            result = push.validate(self.root, True, lambda _: None)
        self.assertFalse(result["ok"])
        checkout = Path(result["logs"]).parent / "checkout"
        self.assertTrue((checkout / ".tmp/nested/.git").is_dir())
        # The fixture owns this empty nested repository; remove it before normal cleanup.
        (checkout / ".tmp/nested/.git").rmdir()
        self.run_git("worktree", "remove", "--force", str(checkout))

    def test_push_subprocesses_ignore_inherited_git_routing(self):
        injected = {**self.env, "GIT_DIR": str(self.home), "GIT_WORK_TREE": str(self.home),
                    "GIT_INDEX_FILE": str(self.home / "index"),
                    "GIT_COMMON_DIR": str(self.home), "GIT_OBJECT_DIRECTORY": str(self.home),
                    "GIT_NAMESPACE": "unrelated", "GIT_SHALLOW_FILE": str(self.home / "missing")}
        with patch.dict(os.environ, injected, clear=True):
            resolved = push.git_output("rev-parse", "--show-toplevel", cwd=self.root)
        self.assertEqual(Path(resolved).resolve(), self.root.resolve())

    @unittest.skipUnless(shutil.which("powershell.exe"), "requires Windows PowerShell")
    def test_windows_powershell_loads_its_own_modules_when_parent_uses_another_edition(self):
        self.write("payload.txt", "release payload")
        modules = self.root / ".tmp" / "foreign-modules"
        module = ".tmp/foreign-modules/Microsoft.PowerShell.Utility/"
        self.write(module + "Microsoft.PowerShell.Utility.psd1",
                   "@{ ModuleVersion = '7.0'; PowerShellVersion = '3.0'; "
                   "RootModule = 'Utility.psm1'; FunctionsToExport = @('Get-FileHash') }")
        self.write(module + "Utility.psm1", "function Get-FileHash { throw 'Wrong edition' }")
        command = ["powershell.exe", "-NoProfile", "-Command",
                   "$ErrorActionPreference = 'Stop'; (Get-FileHash -LiteralPath payload.txt).Hash"]
        log = self.root / ".tmp" / "powershell.log"
        module_path = str(modules) + os.pathsep + str(Path(shutil.which("powershell.exe")).parent / "Modules")
        with patch.dict(os.environ, {**self.env, "PSModulePath": module_path}, clear=True):
            passed = push.run_step(push.Step("powershell", command), self.root, log)
            self.assertEqual(os.environ["PSModulePath"], module_path)
        output = log.read_text(encoding="utf-8")
        self.assertTrue(passed, output)
        self.assertIn(hashlib.sha256(b"release payload").hexdigest().upper(), output)

    def test_successful_worktree_tool_fixtures_allow_private_checkout_cleanup(self):
        for name in ("test_worktree_tools.py", "commit_authorship.py"):
            self.write("scripts/" + name, (ROOT / "scripts" / name).read_text(encoding="utf-8"))
        shutil.copytree(ROOT / ".githooks", self.root / ".githooks")
        self.run_git("add", "scripts", ".githooks")
        self.run_git("commit", "-qm", "fixture worktree tooling")
        command = [sys.executable, "-B", "scripts/test_worktree_tools.py",
                   "GitToolContracts.test_merge_preserves_unrelated_primary_edits"]
        # Exercise the real tool fixture and the exact push cleanup guard without
        # nesting another push checkout inside this disposable Git repository.
        # Git for Windows also has an internal GIT_DIR path-length limit.
        with patch.dict(os.environ, self.env, clear=True):
            result = subprocess.run(command, cwd=self.root, env=push.environment(self.root),
                                    capture_output=True, check=False)
        self.assertEqual(result.returncode, 0,
                         (result.stdout + result.stderr).decode("utf-8", errors="replace"))
        push.worktree.validate_removal(self.root)


if __name__ == "__main__":
    unittest.main()
