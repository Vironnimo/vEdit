#!/usr/bin/env python
"""Manage task worktrees so several agents can work and merge in parallel.

Commands (``.vorch/workflows/worktree-workflow.md`` describes the workflow):

- ``create <name>``: a new branch ``codex/<name>`` from ``main`` in ``.worktrees/<name>``,
  after retrying cleanup of completed tasks and interrupted private merges.
- ``list``: the managed worktrees with branch, port, and pending work.
- ``merge <name>``: merge the branch into ``main`` and remove the worktree. Merges
  are serialized by a lock. The merge commit is made in a private landing checkout,
  where the commit hook checks it; ``main`` only fast-forwards to a checked commit,
  so checks finish before ``main`` changes. Cleanup happens after fast-forward.
- ``delete <name>``: remove an abandoned worktree and its branch.

The script works from any checkout: it finds the primary checkout through Git's
common directory. Configure it for the project below.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import socket
import stat
import subprocess
import sys
import time
from collections.abc import Iterator, Sequence
from contextlib import contextmanager, suppress
from datetime import datetime, timezone
from pathlib import Path
from uuid import uuid4

# --- Project configuration -------------------------------------------------------

PRIMARY_BRANCH = "main"
BRANCH_PREFIX = "codex/"
WORKTREES_DIR_NAME = ".worktrees"
MARKER_FILE_NAME = ".vorch-worktree"
"""Ignored JSON ownership record: name, branch, port (null in vEdit), creation time."""

SETUP_COMMANDS: list[list[str]] = []
"""Commands run in a new worktree, in order; a failure keeps it for diagnosis.
They get VORCH_WORKTREE_NAME, VORCH_WORKTREE_PATH, VORCH_WORKTREE_PORT and
VORCH_PRIMARY_ROOT in their environment. Install each worktree's own dependencies
(for example ``["npm", "ci", "--prefix", "webui"]``); never link folders of another
checkout into a worktree, and never reinstall an editable package into an
environment that other checkouts share."""
# Cargo resolves locked dependencies on demand into this checkout's own target/.
# No ports or global VEDIT_PROFILE_DIR: only an actual test app run needs a profile.

PORT_BASE: int | None = None
"""When set, each worktree gets its own block of ports starting above this one."""
PORTS_PER_WORKTREE = 10

MERGE_WAIT_TIMEOUT_SECONDS = 30 * 60
LANDING_ATTEMPTS = 3
"""How often a merge is remade when main moved while its commit was checked."""

# ---------------------------------------------------------------------------------

NAME_PATTERN = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")
WINDOWS_RESERVED_NAMES = {"con", "prn", "aux", "nul", *(
    f"{prefix}{number}" for prefix in ("com", "lpt") for number in range(1, 10)
)}
CONFLICT_EXIT_CODE = 2


class WorktreeError(Exception):
    """A failure the command reports and exits on."""


def git(*args: str, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    env = {**os.environ, "GIT_TERMINAL_PROMPT": "0", "GCM_INTERACTIVE": "never",
           "GIT_PAGER": "cat", "GIT_MERGE_AUTOEDIT": "no"}
    for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"):
        env.pop(key, None)
    if cwd is not None:
        temporary = cwd / ".tmp" / "worktree"
        temporary.mkdir(parents=True, exist_ok=True)
        env.update({key: str(temporary) for key in ("TEMP", "TMP", "TMPDIR")})
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        stdin=subprocess.DEVNULL,
        check=False,
    )


def git_out(*args: str, cwd: Path | None = None) -> str:
    result = git(*args, cwd=cwd)
    if result.returncode != 0:
        raise WorktreeError(result.stderr.strip() or f"git {' '.join(args)} failed")
    return result.stdout.strip()


def primary_root() -> Path:
    """Return the primary checkout, whichever checkout runs this script."""
    here = Path(__file__).resolve().parents[1]
    common = Path(git_out("rev-parse", "--path-format=absolute", "--git-common-dir", cwd=here))
    root = common.resolve().parent
    if common.name != ".git" or not common.is_dir():
        raise WorktreeError("this command needs a primary checkout with its own .git directory")
    return root


def show(**fields: object) -> None:
    for key, value in fields.items():
        print(f"{key}: {value}")


def posix(path: Path) -> str:
    return path.resolve().as_posix()


# --- Locks -------------------------------------------------------------------------


def _try_lock(handle) -> bool:
    handle.seek(0, os.SEEK_END)
    if handle.tell() == 0:
        handle.write(b"0")
        handle.flush()
    handle.seek(0)
    try:
        if sys.platform == "win32":
            import msvcrt

            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        return False
    return True


def _unlock(handle) -> None:
    handle.seek(0)
    with suppress(OSError):
        if sys.platform == "win32":
            import msvcrt

            msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl

            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


@contextmanager
def file_lock(path: Path, timeout: float, waiting: str) -> Iterator[None]:
    """Hold an exclusive advisory lock on *path*; the OS releases it if the process dies."""
    path.parent.mkdir(parents=True, exist_ok=True)
    if is_link(path):
        raise WorktreeError(f"lock path cannot be a symlink or junction: {path}")
    with path.open("a+b") as handle:
        deadline = time.monotonic() + timeout
        announced = False
        while not _try_lock(handle):
            if time.monotonic() >= deadline:
                raise WorktreeError(f"{path.name} stayed busy for {int(timeout)} s; try again")
            if not announced:
                print(waiting, flush=True)
                announced = True
            time.sleep(0.5)
        try:
            yield
        finally:
            _unlock(handle)


# --- Worktree records --------------------------------------------------------------


class Worktrees:
    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        self.dir = self.root / WORKTREES_DIR_NAME
        self.validate_path(self.dir)

    def validate_path(self, path: Path) -> None:
        """Refuse redirected roots before creating locks, worktrees or deleting files."""
        if path != self.dir and path.parent != self.dir:
            raise WorktreeError(f"path is outside the managed worktree directory: {path}")
        for part in (self.dir, path):
            if is_link(part) or not part.resolve().is_relative_to(self.root):
                raise WorktreeError(f"worktree paths cannot be symlinks or junctions: {part}")

    def lock(self, timeout: float = 60):
        self.validate_path(self.dir)
        return file_lock(self.dir / ".operations.lock", timeout,
                         "waiting for another worktree operation...")

    def path(self, name: str) -> Path:
        if not NAME_PATTERN.fullmatch(name) or name in WINDOWS_RESERVED_NAMES:
            raise WorktreeError("a task name is 1-63 lowercase letters, digits and hyphens, "
                                "starting with a letter or digit; Windows device names are invalid")
        path = self.dir / name
        self.validate_path(path)
        return path

    def marker(self, path: Path) -> dict[str, object]:
        try:
            data = json.loads((path / MARKER_FILE_NAME).read_text(encoding="utf-8"))
        except (OSError, ValueError, UnicodeError):
            return {}
        return data if isinstance(data, dict) else {}

    def managed(self) -> list[Path]:
        if not self.dir.is_dir():
            return []
        return sorted(
            path
            for path in self.dir.iterdir()
            if path.is_dir()
            and not path.name.startswith(".")
            and (path / MARKER_FILE_NAME).is_file()
        )

    def registrations(self) -> list[dict[str, str]]:
        records: list[dict[str, str]] = []
        current: dict[str, str] = {}
        for field in git_out("worktree", "list", "--porcelain", "-z", cwd=self.root).split("\0"):
            if field:
                key, _, value = field.partition(" ")
                current[key] = value
            elif current:
                records.append(current)
                current = {}
        if current:
            records.append(current)
        return records

    def registration(self, path: Path) -> dict[str, str] | None:
        return next((record for record in self.registrations()
                     if Path(record["worktree"]).resolve() == path.resolve()), None)

    def cleanup_file(self, name: str) -> Path:
        self.path(name)
        path = self.dir / f".cleanup-{name}.json"
        self.validate_path(path)
        return path

    def cleanup_record(self, name: str) -> dict[str, str]:
        try:
            record = json.loads(self.cleanup_file(name).read_text(encoding="utf-8"))
        except (OSError, ValueError, UnicodeError) as exc:
            raise WorktreeError(f"cannot read cleanup record for '{name}': {exc}") from exc
        if (not isinstance(record, dict) or record.get("name") != name
                or record.get("branch") != BRANCH_PREFIX + name
                or not isinstance(record.get("tip"), str)
                or not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", record["tip"])):
            raise WorktreeError(f"invalid cleanup record for '{name}'; nothing was removed")
        return record

    def finish_cleanup(self, name: str, *, force: bool = False) -> None:
        """Resume a recorded removal without relying on a surviving checkout."""
        record = self.cleanup_record(name)
        path = self.path(name)
        branch, tip = record["branch"], record["tip"]
        current = git("rev-parse", "--verify", "--quiet", f"refs/heads/{branch}", cwd=self.root)
        if current.returncode not in (0, 1):
            raise WorktreeError(f"cannot check branch '{branch}': {current.stderr.strip()}")
        if current.returncode == 0 and current.stdout.strip() != tip:
            raise WorktreeError(f"branch '{branch}' changed; its worktree and commits were kept")
        if not force:
            git_out("merge-base", "--is-ancestor", tip, PRIMARY_BRANCH, cwd=self.root)
        if path.exists() and not (path / ".git").exists() and not force:
            # Without an index, preserve anything beyond disposable build output.
            if any(entry.name not in ("target", ".tmp", MARKER_FILE_NAME)
                   for entry in path.iterdir()):
                raise WorktreeError(f"worktree '{name}' has files without Git metadata; "
                                    "restore its metadata before retrying")
        registered = self.registration(path)
        if registered is None and (path / ".git").exists():
            raise WorktreeError(f"unregistered Git checkout kept at {path}")
        if registered is not None:
            if (registered.get("HEAD") != tip
                    or registered.get("branch") != f"refs/heads/{branch}"
                    or "locked" in registered):
                raise WorktreeError(f"worktree '{name}' changed or is locked; nothing was removed")
            had_metadata = (path / ".git").exists()
            if not had_metadata and path.exists():
                # Git repairs the backlink from its surviving administrative record.
                # It can return nonzero after making that repair; verify the resulting
                # checkout below instead of trusting its exit code alone.
                repaired = git("worktree", "repair", str(path), cwd=self.root)
                if not (path / ".git").is_file():
                    raise WorktreeError(f"cannot restore worktree metadata: {repaired.stderr.strip()}")
            if (path / ".git").exists():
                marker = path / MARKER_FILE_NAME
                if not marker.exists():
                    with marker.open("x", encoding="utf-8") as handle:
                        json.dump({"name": name, "branch": branch}, handle)
                self.require(name)
                if had_metadata and not force and self.pending(path, branch)[0]:
                    raise WorktreeError(f"source worktree kept: '{name}' has uncommitted files")
            validate_removal(path)
            if Path.cwd().resolve().is_relative_to(path.resolve()):
                os.chdir(self.root)
            flags = ["--force"] if force or not had_metadata else []
            removed = git("worktree", "remove", *flags,
                          str(path), cwd=self.root)
            # Git can deregister a Windows checkout before failing to delete locked files.
            if removed.returncode != 0 and self.registration(path) is not None:
                raise WorktreeError(f"source worktree kept: {removed.stderr.strip()}")
        if path.exists():
            validate_removal(path)
            remove_directory_tree(path)
        if any(item.get("branch") == f"refs/heads/{branch}" for item in self.registrations()):
            raise WorktreeError(f"branch '{branch}' is checked out elsewhere; it was kept")
        if current.returncode == 0:
            git_out("update-ref", "-d", f"refs/heads/{branch}", tip, cwd=self.root)
        self.cleanup_file(name).unlink()

    def sweep_cleanup(self) -> None:
        """Retry explicit completed-task removals; never select active tasks by age."""
        for record in sorted(self.dir.glob(".cleanup-*.json")):
            name = record.name[len(".cleanup-"):-len(".json")]
            try:
                self.finish_cleanup(name)
                show(name=name, status="cleanup-completed")
            except (WorktreeError, OSError) as exc:
                print(f"note: cleanup pending for '{name}': {exc}")

    def landing_file(self, path: Path) -> Path:
        self.validate_path(path)
        record = path.with_name(path.name + ".json")
        self.validate_path(record)
        return record

    def owns_landing(self, path: Path) -> bool:
        try:
            marker = json.loads(self.landing_file(path).read_text(encoding="utf-8"))
        except (OSError, ValueError, UnicodeError):
            return False
        return marker == {"kind": "landing", "root": posix(self.root)}

    def remove_landing(self, path: Path) -> None:
        if not self.owns_landing(path):
            raise WorktreeError(f"private checkout ownership is unknown: {path}")
        registered = self.registration(path)
        if registered is None and (path / ".git").exists():
            raise WorktreeError(f"unregistered Git checkout kept at {path}")
        if registered is not None and ("detached" not in registered or "locked" in registered):
            raise WorktreeError(f"private checkout changed or is locked; kept at {path}")
        validate_removal(path)
        if registered is not None:
            if path.exists() and not (path / ".git").exists():
                git("worktree", "repair", str(path), cwd=self.root)
            removed = git("worktree", "remove", "--force", str(path), cwd=self.root)
            if removed.returncode != 0 and self.registration(path) is not None:
                raise WorktreeError(f"private checkout kept at {path}: {removed.stderr.strip()}")
        if path.exists():
            remove_directory_tree(path)
        self.landing_file(path).unlink()

    def sweep_landings(self) -> None:
        # Every landing runs under the same OS lock as this sweep. A killed
        # process releases that lock; push checkouts have a separate lifecycle.
        for record in sorted(self.dir.glob(".landing-*.json")):
            path = record.with_name(record.name[:-len(".json")])
            if self.owns_landing(path):
                self.remove_landing(path)

    def require(self, name: str) -> tuple[Path, str]:
        """Return a managed worktree's path and checked-out branch."""
        path = self.path(name)
        if not (path / MARKER_FILE_NAME).is_file():
            raise WorktreeError(f"no managed worktree '{name}' in {posix(self.dir)}")
        marker = self.marker(path)
        expected = BRANCH_PREFIX + name
        if marker.get("name") != name or marker.get("branch") != expected:
            raise WorktreeError(f"worktree '{name}' has a mismatched marker; nothing was removed")
        common = Path(git_out("rev-parse", "--path-format=absolute", "--git-common-dir", cwd=path))
        top = Path(git_out("rev-parse", "--show-toplevel", cwd=path))
        if common.resolve() != (self.root / ".git").resolve() or top.resolve() != path.resolve():
            raise WorktreeError(f"'{path}' is not a worktree of this repository")
        branch = git_out("branch", "--show-current", cwd=path)
        if branch != expected:
            raise WorktreeError(f"worktree '{name}' is on '{branch}', expected '{expected}'; "
                                "nothing was removed")
        return path, branch

    def pending(self, path: Path, branch: str) -> tuple[list[str], int]:
        """Return the worktree's uncommitted entries and its commits that main lacks."""
        status = [
            line
            for line in git_out("status", "--porcelain", cwd=path).splitlines()
            if line and line != f"?? {MARKER_FILE_NAME}"
        ]
        ahead = int(git_out("rev-list", "--count", f"{PRIMARY_BRANCH}..{branch}", cwd=self.root))
        return status, ahead

    def free_port(self) -> int | None:
        if PORT_BASE is None:
            return None
        used = {self.marker(path).get("port") for path in self.managed()}
        for index in range(1, 500):
            port = PORT_BASE + index * PORTS_PER_WORKTREE
            if port in used:
                continue
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
                if probe.connect_ex(("127.0.0.1", port)) != 0:
                    return port
        raise WorktreeError("no free port block found")

    def remove(self, path: Path, branch: str, *, force: bool, expected: str) -> None:
        """Remove the worktree directory and its branch.

        The directory goes with its ignored and untracked files, so callers check for
        pending work first. *force* also deletes a branch that main does not contain.
        """
        self.validate_path(path)
        self.require(path.name)
        validate_removal(path)
        if git_out("rev-parse", branch, cwd=self.root) != expected:
            raise WorktreeError(f"branch '{branch}' changed; its worktree and commits were kept")
        if not force and git("merge-base", "--is-ancestor", expected, PRIMARY_BRANCH,
                             cwd=self.root).returncode != 0:
            raise WorktreeError(f"branch '{branch}' has unmerged commits; its worktree was kept")
        record = self.cleanup_file(path.name)
        temporary = self.root / ".tmp" / "worktree" / f".record-{uuid4().hex}.tmp"
        temporary.parent.mkdir(parents=True, exist_ok=True)
        temporary.write_text(json.dumps({"name": path.name, "branch": branch, "tip": expected}),
                             encoding="utf-8")
        try:
            os.replace(temporary, record)
        finally:
            temporary.unlink(missing_ok=True)
        self.finish_cleanup(path.name, force=force)


def is_link(path: Path) -> bool:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return False
    return stat.S_ISLNK(info.st_mode) or bool(
        getattr(info, "st_file_attributes", 0) & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    )


def validate_removal(path: Path) -> None:
    """Git for Windows can follow junctions when removing a worktree."""
    def unreadable(error: OSError) -> None:
        raise WorktreeError(f"cannot inspect worktree before removal: {error}")

    for directory, dirs, files in os.walk(path, followlinks=False, onerror=unreadable):
        for name in dirs + files:
            entry = Path(directory) / name
            if is_link(entry):
                raise WorktreeError(f"refusing removal with a symlink or junction: {entry}")
            if name == ".git" and entry.parent != path:
                raise WorktreeError(f"refusing removal with a nested Git repository: {entry}")


def remove_directory_tree(path: Path) -> None:
    """Retry transient Windows locks and read-only build files, within the checked tree."""
    def readonly(function, filename, error_info):
        target = Path(filename)
        failure = error_info[1]
        if (not isinstance(failure, PermissionError)
                or function not in (os.unlink, os.remove, os.rmdir) or is_link(target)
                or not target.resolve().is_relative_to(path.resolve())):
            raise failure
        target.chmod(target.stat().st_mode | stat.S_IWUSR)
        function(filename)

    for attempt in range(3):
        try:
            validate_removal(path)
            shutil.rmtree(path, onerror=readonly)
            return
        except FileNotFoundError:
            if not path.exists():
                return
            raise
        except PermissionError:
            if attempt == 2:
                raise
            time.sleep(0.2)


# --- Commands ----------------------------------------------------------------------


def create(trees: Worktrees, args: argparse.Namespace) -> int:
    name: str = args.name
    branch = BRANCH_PREFIX + name
    path = trees.path(name)
    trees.sweep_landings()
    trees.sweep_cleanup()
    if trees.cleanup_file(name).exists():
        raise WorktreeError(f"cleanup of '{name}' is pending; run "
                            f"python scripts/worktree.py delete {name} before reusing its name")
    if path.exists():
        raise WorktreeError(f"worktree '{name}' already exists: {posix(path)}")
    if (
        git("rev-parse", "--verify", "--quiet", f"refs/heads/{branch}", cwd=trees.root).returncode
        == 0
    ):
        raise WorktreeError(f"branch '{branch}' already exists; choose another task name")

    port = trees.free_port()
    git_out("worktree", "add", "-b", branch, str(path), PRIMARY_BRANCH, cwd=trees.root)
    marker = {
        "name": name,
        "branch": branch,
        "port": port,
        "created": datetime.now(timezone.utc).isoformat(timespec="seconds"),
    }
    (path / MARKER_FILE_NAME).write_text(json.dumps(marker, indent=2) + "\n", encoding="utf-8")

    env = {
        **os.environ,
        "VORCH_WORKTREE_NAME": name,
        "VORCH_WORKTREE_PATH": str(path),
        "VORCH_WORKTREE_PORT": "" if port is None else str(port),
        "VORCH_PRIMARY_ROOT": str(trees.root),
    }
    temporary = path / ".tmp" / "worktree"
    temporary.mkdir(parents=True, exist_ok=True)
    env.update({key: str(temporary) for key in ("TEMP", "TMP", "TMPDIR")})
    for command in SETUP_COMMANDS:
        print(f"setup: {' '.join(command)}", flush=True)
        executable = shutil.which(command[0]) or command[0]
        result = subprocess.run([executable, *command[1:]], cwd=path, env=env,
                                stdin=subprocess.DEVNULL, check=False)
        if result.returncode != 0:
            raise WorktreeError(f"setup command failed; worktree kept at {path}: {command}")

    show(status="created", name=name, branch=branch, path=posix(path),
         **({"port": port} if port is not None else {}))
    return 0


def list_worktrees(trees: Worktrees, _args: argparse.Namespace) -> int:
    paths = trees.managed()
    pending_names = {path.name[len(".cleanup-"):-len(".json")]
                     for path in trees.dir.glob(".cleanup-*.json")}
    if not paths and not pending_names:
        show(status="empty")
        return 0
    for index, name in enumerate(sorted({path.name for path in paths} | pending_names)):
        if index:
            print()
        if name in pending_names:
            record = trees.cleanup_record(name)
            show(name=name, branch=record["branch"], path=posix(trees.path(name)),
                 status="cleanup-pending")
            continue
        path = trees.path(name)
        path, branch = trees.require(path.name)
        status, ahead = trees.pending(path, branch)
        port = trees.marker(path).get("port")
        show(
            name=path.name,
            branch=branch,
            path=posix(path),
            **({"port": port} if port is not None else {}),
            uncommitted=len(status),
            commits=ahead,
        )
    return 0


def _merge_conflicts(root: Path, branch: str) -> list[str]:
    """Return the paths merging *branch* into main conflicts on, without touching main."""
    result = git(
        "merge-tree",
        "--write-tree",
        "--name-only",
        "--no-messages",
        PRIMARY_BRANCH,
        branch,
        cwd=root,
    )
    if result.returncode == 0:
        return []
    if result.returncode != 1:
        raise WorktreeError(f"conflict check failed; main is unchanged: {result.stderr.strip()}")
    return [line for line in result.stdout.splitlines()[1:] if line.strip()]


def _fast_forward(root: Path, commit: str) -> subprocess.CompletedProcess[str]:
    require_primary_branch(root)
    result = git("merge", "--ff-only", "-q", commit, cwd=root)
    for _ in range(5):
        if result.returncode == 0 or "index.lock" not in result.stderr:
            break
        time.sleep(1)
        require_primary_branch(root)
        result = git("merge", "--ff-only", "-q", commit, cwd=root)
    return result


@contextmanager
def _landing_checkout(trees: Worktrees, name: str) -> Iterator[Path]:
    landing = trees.dir / f".landing-{name}-{uuid4().hex[:12]}"
    trees.validate_path(landing)
    record = trees.landing_file(landing)
    if landing.exists() or record.exists():
        raise WorktreeError(f"private checkout path already exists: {landing}")
    record.write_text(json.dumps({"kind": "landing", "root": posix(trees.root)}), encoding="utf-8")
    try:
        git_out("worktree", "add", "--detach", str(landing), PRIMARY_BRANCH, cwd=trees.root)
        yield landing
    finally:
        trees.remove_landing(landing)


def _land(trees: Worktrees, name: str, branch: str, tip: str, message: str) -> str:
    """Make, check and land the merge commit; return the commit main now points to."""
    root = trees.root
    with _landing_checkout(trees, name) as landing:
        for _attempt in range(LANDING_ATTEMPTS):
            require_primary_branch(root)
            base = git_out("rev-parse", PRIMARY_BRANCH, cwd=root)
            git_out("reset", "-q", "--hard", base, cwd=landing)
            merged = git("merge", "--no-ff", "--no-commit", tip, cwd=landing)
            if merged.returncode != 0:
                conflicts = git_out("diff", "--name-only", "--diff-filter=U", cwd=landing)
                git("merge", "--abort", cwd=landing)
                for path in conflicts.splitlines():
                    print(f"conflicted: {path}")
                raise WorktreeError(
                    f"merging '{branch}' into {PRIMARY_BRANCH} failed; main is unchanged: "
                    f"{merged.stderr.strip()}\nhint: in the worktree run `git rebase "
                    f"{PRIMARY_BRANCH}`, resolve, commit, and merge again"
                )
            if git("rev-parse", "-q", "--verify", "MERGE_HEAD", cwd=landing).returncode != 0:
                return base  # main already contains the branch

            print("checking the merge commit (no output until done)...", flush=True)
            if not (landing / ".githooks" / "pre-commit").is_file():
                raise WorktreeError("the landing checkout lacks .githooks/pre-commit; main is unchanged")
            committed = git("-c", "core.hooksPath=.githooks", "commit", "-m", message, cwd=landing)
            print((committed.stdout + committed.stderr).strip())
            commit = git_out("rev-parse", "HEAD", cwd=landing)
            if committed.returncode != 0 or commit == base:
                raise WorktreeError(
                    "the merge commit failed the commit check; main is unchanged\n"
                    f"hint: in the worktree run `git rebase {PRIMARY_BRANCH}`, fix the problems "
                    "above, commit, and merge again"
                )

            landed = _fast_forward(root, commit)
            if landed.returncode == 0:
                return commit
            if git_out("rev-parse", PRIMARY_BRANCH, cwd=root) == base:
                raise WorktreeError(
                    f"main could not be fast-forwarded: {landed.stderr.strip()}\n"
                    "hint: uncommitted changes in the primary checkout overlap the merge; "
                    "they must be committed or set aside before the merge can land"
                )
            print("main moved while the merge commit was checked; merging again...", flush=True)
    raise WorktreeError(
        f"main moved during each of {LANDING_ATTEMPTS} checks; main is unchanged by this merge; "
        "run the merge again"
    )


def require_primary_branch(root: Path) -> None:
    current = git_out("branch", "--show-current", cwd=root)
    if current != PRIMARY_BRANCH:
        raise WorktreeError(
            f"the primary checkout is on '{current}', not '{PRIMARY_BRANCH}'; switch it back first"
        )


def merge(trees: Worktrees, args: argparse.Namespace) -> int:
    name: str = args.name
    trees.path(name)
    trees.sweep_landings()
    if trees.cleanup_file(name).exists() and not (trees.path(name) / ".git").exists():
        trees.finish_cleanup(name)
        show(name=name, status="removed")
        return 0
    path, branch = trees.require(name)
    require_primary_branch(trees.root)
    tip = git_out("rev-parse", branch, cwd=trees.root)
    status, ahead = trees.pending(path, branch)
    if status:
        for line in status:
            print(f"uncommitted: {line}")
        raise WorktreeError(
            "the worktree has uncommitted or untracked files; commit or remove them"
        )

    if ahead == 0:
        print(f"'{branch}' has no commits that {PRIMARY_BRANCH} lacks; removing the worktree")
    else:
        conflicts = _merge_conflicts(trees.root, tip)
        if conflicts:
            for conflict in conflicts:
                print(f"conflicted: {conflict}")
            print(
                f"hint: in the worktree run `git rebase {PRIMARY_BRANCH}`, resolve, commit, "
                "and merge again"
            )
            print(f"error: merging '{branch}' into {PRIMARY_BRANCH} conflicts; main is unchanged")
            return CONFLICT_EXIT_CODE

    message = args.message or f"merge: {name}"
    commit = git_out("rev-parse", PRIMARY_BRANCH, cwd=trees.root)
    try:
        if ahead:
            commit = _land(trees, name, branch, tip, message)
        trees.remove(path, branch, force=False, expected=tip)
    except (WorktreeError, OSError) as exc:
        if git("merge-base", "--is-ancestor", tip, PRIMARY_BRANCH,
               cwd=trees.root).returncode != 0:
            raise
        commit = git_out("rev-parse", PRIMARY_BRANCH, cwd=trees.root)
        show(name=name, status="merged-cleanup-pending", commit=commit[:12], branch=branch)
        print(f"error: merge completed; cleanup pending: {exc}")
        if path.exists():
            print(f"note: source worktree kept at {posix(path)}")
        print(f"hint: resolve the cleanup error, leave the worktree directory, then run from "
              f"{posix(trees.root)}: python scripts/worktree.py merge {name}")
        return 1
    show(name=name, status="merged" if ahead else "removed", commit=commit[:12], branch=branch)
    return 0


def delete(trees: Worktrees, args: argparse.Namespace) -> int:
    name: str = args.name
    trees.path(name)
    trees.sweep_landings()
    if trees.cleanup_file(name).exists() and not (trees.path(name) / ".git").exists():
        trees.finish_cleanup(name, force=args.force)
        show(name=name, status="deleted")
        return 0
    path, branch = trees.require(name)
    tip = git_out("rev-parse", branch, cwd=trees.root)
    if not args.force:
        status, ahead = trees.pending(path, branch)
        if status or ahead:
            for line in status:
                print(f"uncommitted: {line}")
            if ahead:
                print(f"unmerged: {ahead} commits that {PRIMARY_BRANCH} lacks")
            raise WorktreeError("the worktree holds work; merge it, or delete with --force")
    trees.remove(path, branch, force=args.force, expected=tip)
    show(name=name, status="deleted")
    return 0


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("create", help="retry recorded cleanup, then create a task worktree").add_argument("name")
    commands.add_parser("list", help="list task worktrees")
    merging = commands.add_parser("merge", help="merge and remove a task; incomplete cleanup exits 1",
                                  description="Merge a task into main and remove its checkout and branch. "
                                  "Exit 0 means both completed; exit 1 includes pending cleanup. "
                                  "Repeat after resolving the reported error to finish cleanup.")
    merging.add_argument("name")
    merging.add_argument("-m", "--message", help="merge commit message (default: merge: <name>)")
    merging.add_argument(
        "--wait-timeout",
        type=float,
        default=MERGE_WAIT_TIMEOUT_SECONDS,
        help="seconds to wait for another worktree operation; omit for 1800 seconds",
    )
    deleting = commands.add_parser("delete", help="remove a task worktree and its branch")
    deleting.add_argument("name")
    deleting.add_argument(
        "--force", action="store_true", help="discard this task's uncommitted or unmerged work; "
        "omit to preserve pending work (ignored build files are removed either way)"
    )
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    args = parse_args(sys.argv[1:] if argv is None else argv)
    commands = {"create": create, "list": list_worktrees, "merge": merge, "delete": delete}
    try:
        trees = Worktrees(primary_root())
        with trees.lock(getattr(args, "wait_timeout", 60)):
            return commands[args.command](trees, args)
    except (WorktreeError, OSError) as exc:
        print(f"error: {exc}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
