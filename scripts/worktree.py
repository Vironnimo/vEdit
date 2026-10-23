#!/usr/bin/env python
"""Manage task worktrees so several agents can work and merge in parallel.

Commands (``.vorch/workflows/worktree-workflow.md`` describes the workflow):

- ``create <name>``: a new branch ``codex/<name>`` from ``main`` in ``.worktrees/<name>``,
  with a marker file, an optional port, and the project's setup commands.
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
        except (OSError, ValueError):
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
        if Path.cwd().resolve().is_relative_to(path.resolve()):
            os.chdir(self.root)
        removed = git("worktree", "remove", *(["--force"] if force else []), str(path), cwd=self.root)
        if removed.returncode != 0:
            raise WorktreeError(
                f"the worktree could not be removed: {removed.stderr.strip()}\n"
                f"hint: leave the directory, then run from {posix(self.root)}: "
                f"python scripts/worktree.py delete {path.name}"
            )
        deleted = git("update-ref", "-d", f"refs/heads/{branch}", expected, cwd=self.root)
        if deleted.returncode != 0:
            print(f"note: branch '{branch}' kept: {deleted.stderr.strip()}")


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


# --- Commands ----------------------------------------------------------------------


def create(trees: Worktrees, args: argparse.Namespace) -> int:
    name: str = args.name
    branch = BRANCH_PREFIX + name
    path = trees.path(name)
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
    if not paths:
        show(status="empty")
        return 0
    for index, path in enumerate(paths):
        if index:
            print()
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
    git_out("worktree", "add", "--detach", str(landing), PRIMARY_BRANCH, cwd=trees.root)
    try:
        yield landing
    finally:
        try:
            trees.validate_path(landing)
            validate_removal(landing)
            removed = git("worktree", "remove", "--force", str(landing), cwd=trees.root)
            if removed.returncode != 0:
                print(f"note: private landing checkout kept at {landing}: {removed.stderr.strip()}")
        except (WorktreeError, OSError) as exc:
            print(f"note: private landing checkout kept at {landing}: {exc}")


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
        trees.remove(path, branch, force=False, expected=tip)
        show(name=name, status="removed")
        return 0

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
    commit = _land(trees, name, branch, tip, message)

    show(name=name, status="merged", commit=commit[:12], branch=branch)
    try:
        trees.remove(path, branch, force=False, expected=tip)
    except (WorktreeError, OSError) as exc:
        print(f"note: merge completed; source worktree kept: {exc}")
    return 0


def delete(trees: Worktrees, args: argparse.Namespace) -> int:
    name: str = args.name
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
    commands.add_parser("create", help="create .worktrees/<name> on codex/<name>").add_argument("name")
    commands.add_parser("list", help="list task worktrees")
    merging = commands.add_parser("merge", help="merge a task into main and remove it")
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
