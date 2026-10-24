#!/usr/bin/env python
"""Validate committed main in a private checkout, then push exactly that commit.

Use --no-push for local validation, including repositories without a remote.
Checks run to completion with progress and persistent logs under .tmp/push/.
Each private checkout has its own target/; no build cache is shared or redirected.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from uuid import uuid4

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import worktree

PRIMARY_BRANCH = "main"
REMOTE = "origin"
TOOLCHAIN = "1.97.1"
TAIL_LINES = 30
CARGO = ["rustup", "run", TOOLCHAIN, "cargo"]


@dataclass(frozen=True)
class Step:
    name: str
    command: list[str]
    requires: tuple[str, ...] = ()


STEPS = [
    Step("dependencies", [*CARGO, "fetch", "--locked"]),
    Step("rust-format", [*CARGO, "fmt", "--", "--check"], ("dependencies",)),
    Step("rust-tests", [sys.executable, "scripts/test_survey.py", "--jobs", "8"],
         ("dependencies",)),
    Step("clippy", [*CARGO, "clippy", "--locked", "--all-targets", "--", "-D", "warnings"],
         ("dependencies",)),
    Step("source-static", [sys.executable, "-B", "scripts/commit_check.py", "--all"]),
    Step("python-syntax", [sys.executable, "-B", "-c",
         "from pathlib import Path; "
         "[compile(p.read_bytes(), str(p), 'exec') for p in Path('scripts').glob('*.py')]"]),
    Step("tooling-tests", [sys.executable, "-B", "-m", "unittest", "discover", "-s", "scripts",
                           "-p", "test_*.py"], ("python-syntax",)),
    Step("domain-maps", [sys.executable, "-B", "scripts/domain_maps.py"]),
    Step("whitespace", ["git", "show", "--format=", "--check", "HEAD"]),
    Step("project-temp", ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass",
                          "-File", "scripts/test-project-temp.ps1"]),
    Step("installer", ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass",
                       "-File", "scripts/test-install.ps1"]),
]


def environment(root: Path) -> dict[str, str]:
    env = dict(os.environ)
    for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR",
                "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX",
                "GIT_CONFIG", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT",
                "GIT_IMPLICIT_WORK_TREE", "GIT_GRAFT_FILE", "GIT_REPLACE_REF_BASE",
                "GIT_SHALLOW_FILE", "GIT_NAMESPACE", "GIT_QUARANTINE_PATH"):
        env.pop(key, None)
    for key in list(env):
        if key.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_")):
            env.pop(key)
    temp = root / ".tmp"
    temp.mkdir(parents=True, exist_ok=True)
    for key in ("TEMP", "TMP", "TMPDIR", "GIT_CEILING_DIRECTORIES"):
        env[key] = str(temp)
    env.update({"GIT_TERMINAL_PROMPT": "0", "GCM_INTERACTIVE": "never",
                "GIT_NO_REPLACE_OBJECTS": "1",
                "GIT_PAGER": "cat", "PAGER": "cat", "PYTHONDONTWRITEBYTECODE": "1",
                "RUSTUP_TOOLCHAIN": TOOLCHAIN})
    env.setdefault("WGPU_BACKEND", "gl")
    env.pop("CARGO_TARGET_DIR", None)
    for key in ("UPDATE_SNAPSHOTS", "INSTA_UPDATE", "EGUI_KITTEST_UPDATE_SNAPSHOTS",
                "VEDIT_SNAPSHOT_DIR"):
        env.pop(key, None)
    return env


def git(*args: str, cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args], cwd=cwd, env=environment(cwd), stdin=subprocess.DEVNULL,
        capture_output=True, text=True, encoding="utf-8", errors="replace", check=False,
    )


def git_output(*args: str, cwd: Path) -> str:
    result = git(*args, cwd=cwd)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    return result.stdout.strip()


def primary_root() -> Path:
    here = Path(__file__).resolve().parents[1]
    common = git_output("rev-parse", "--path-format=absolute", "--git-common-dir", cwd=here)
    return Path(common).parent


def remote_is_behind(root: Path, commit: str) -> tuple[bool, str | None]:
    """Use the fetched commit even when no remote-tracking ref is configured.

    A remote without the branch, such as a new empty repository, contains
    nothing that local main could lack; the push then creates the branch.
    """
    listed = git("ls-remote", "--exit-code", "--heads", REMOTE, f"refs/heads/{PRIMARY_BRANCH}", cwd=root)
    if listed.returncode == 2:
        return True, None
    if listed.returncode:
        raise RuntimeError(listed.stderr.strip() or listed.stdout.strip())
    git_output("fetch", "-q", REMOTE, PRIMARY_BRANCH, cwd=root)
    remote = git_output("rev-parse", "FETCH_HEAD", cwd=root)
    contained = git("merge-base", "--is-ancestor", remote, commit, cwd=root).returncode == 0
    return contained, remote


def run_step(step: Step, checkout: Path, log: Path) -> bool:
    with log.open("w", encoding="utf-8", errors="replace") as handle:
        try:
            result = subprocess.run(
                step.command, cwd=checkout, env=environment(checkout),
                stdin=subprocess.DEVNULL, stdout=handle, stderr=subprocess.STDOUT, check=False,
            )
            return result.returncode == 0
        except OSError as exc:
            handle.write(f"{step.command[0]} could not run: {exc}\n")
            return False


def validate(root: Path, no_push: bool, emit=print) -> dict:
    commit = git_output("rev-parse", "--verify", f"{PRIMARY_BRANCH}^{{commit}}", cwd=root)
    result = {"ok": False, "commit": commit, "pushed": False, "steps": [],
              "errors": [], "notes": [], "logs": None}
    if not no_push:
        contained, remote = remote_is_behind(root, commit)
        if not contained:
            result["errors"].append(f"{REMOTE}/{PRIMARY_BRANCH} has commits that local main "
                                    "lacks. Nothing was checked or pushed.")
            return result
        if remote == commit:
            result.update(ok=True, notes=[f"{REMOTE}/{PRIMARY_BRANCH} already has {commit}."])
            return result
    run = root / ".tmp" / "push" / f"{time.strftime('%Y%m%d-%H%M%S')}-{uuid4().hex[:8]}"
    for path in (root / ".tmp", root / ".tmp" / "push", run):
        if worktree.is_link(path):
            raise RuntimeError(f"Private checkout path cannot use a symlink or junction: {path}")
    checkout = run / "checkout"
    logs = run / "logs"
    logs.mkdir(parents=True)
    result["logs"] = str(logs)
    git_output("worktree", "add", "--detach", str(checkout), commit, cwd=root)
    emit(f"Checking {commit[:12]} in {checkout}; logs: {logs}")
    results: dict[str, bool] = {}
    try:
        for step in STEPS:
            if any(not results.get(required) for required in step.requires):
                result["steps"].append({"name": step.name, "status": "skipped"})
                emit(f"SKIP  {step.name} (requires {', '.join(step.requires)})")
                continue
            started = time.monotonic()
            emit(f"RUN   {step.name}")
            log = logs / f"{step.name}.log"
            passed = run_step(step, checkout, log)
            results[step.name] = passed
            entry = {"name": step.name, "status": "passed" if passed else "failed",
                     "seconds": round(time.monotonic() - started, 2), "log": str(log)}
            result["steps"].append(entry)
            emit(f"{'PASS' if passed else 'FAIL'}  {step.name} ({entry['seconds']:.0f} s)")
            if not passed:
                result["errors"].append(f"{step.name} failed; complete log: {log}")
                emit("\n".join(log.read_text(encoding="utf-8", errors="replace")
                               .splitlines()[-TAIL_LINES:]))
        if git_output("status", "--porcelain", "--untracked-files=no", cwd=checkout):
            result["errors"].append("A check changed tracked files in the private checkout.")
    finally:
        survey = checkout / ".tmp" / "test-survey"
        preserved = True
        try:
            for path in (root / ".tmp", run.parent, run, checkout):
                if worktree.is_link(path):
                    raise worktree.WorktreeError(f"path is a symlink or junction: {path}")
            worktree.validate_removal(checkout)
        except (OSError, worktree.WorktreeError) as exc:
            preserved = False
            result["errors"].append(f"Unsafe private-checkout cleanup: {exc}. "
                                    f"Private checkout remains at {checkout}.")
        if preserved and survey.is_dir():
            try:
                shutil.copytree(survey, logs / "test-survey")
            except OSError as exc:
                preserved = False
                result["errors"].append(f"Could not preserve survey reports: {exc}. "
                                        f"Private checkout remains at {checkout}.")
        if preserved:
            removed = git("worktree", "remove", "--force", str(checkout), cwd=root)
            if removed.returncode:
                result["notes"].append(f"Private checkout remains at {checkout}: "
                                       f"{removed.stderr.strip()}")
    if result["errors"] or any(step["status"] == "skipped" for step in result["steps"]):
        return result
    if no_push:
        result.update(ok=True, notes=[*result["notes"], "Checks only; nothing pushed."])
        return result
    contained, remote = remote_is_behind(root, commit)
    if not contained:
        result["errors"].append(f"{REMOTE}/{PRIMARY_BRANCH} moved past the checked commit; "
                                "nothing pushed.")
        return result
    pushed = git("push", REMOTE, f"{commit}:refs/heads/{PRIMARY_BRANCH}", cwd=root)
    if pushed.returncode:
        result["errors"].append("Push did not report success; remote outcome is unknown. "
                                "Inspect the remote before retrying. " + pushed.stderr.strip())
        return result
    result.update(ok=True, pushed=True)
    return result


class Parser(argparse.ArgumentParser):
    json_mode = False

    def error(self, message):
        print(json.dumps({"ok": False, "pushed": False, "errors": [message]}) if self.json_mode else
              f"Error: {message}. Nothing was changed. Use --help for accepted arguments.")
        raise SystemExit(2)


def main(argv: list[str] | None = None) -> int:
    parser = Parser(description=__doc__.splitlines()[0],
        epilog="Exit 0: passed/already pushed; 1: failed; 2: invalid arguments. "
               "Requires Windows, installed Rust 1.97.1 (rustfmt/clippy), Git and Python. "
               "Checks can take more than 30 minutes; progress and logs remain available.")
    parser.json_mode = "--json" in (sys.argv[1:] if argv is None else argv)
    parser.add_argument("--no-push", action="store_true",
                        help="validate local main without contacting a remote or pushing")
    parser.add_argument("--json", action="store_true", help="print one final JSON result document")
    args = parser.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    try:
        result = validate(primary_root(), args.no_push,
                          (lambda message: None) if args.json else
                          (lambda message: print(message, flush=True)))
    except (OSError, RuntimeError) as exc:
        result = {"ok": False, "pushed": False, "errors": [str(exc)], "notes": []}
    except KeyboardInterrupt:
        result = {"ok": False, "pushed": False, "errors": ["Interrupted; no push attempted "
                  "unless interruption occurred during the final push. Inspect the remote "
                  "if that push had started."], "notes": []}
    if args.json:
        print(json.dumps(result, ensure_ascii=True))
    else:
        state = "pushed" if result.get("pushed") else "passed" if result["ok"] else "failed"
        print(f"Push check {state}: {result.get('commit', 'no commit checked')}.")
        for note in result["notes"]:
            print(f"Note: {note}")
        for error in result["errors"]:
            print(f"Error: {error}")
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
