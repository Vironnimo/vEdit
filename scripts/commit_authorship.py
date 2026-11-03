#!/usr/bin/env python
"""Keep AI tool attribution out of the history: commits carry only the user's authorship.

Every commit, merge commit and push in a checkout that enabled the tracked hooks
(``git config core.hooksPath .githooks``) passes through here. Three commands,
one per hook:

- ``identity`` (``pre-commit``, ``pre-merge-commit``) requires author and committer
  to match the repository's configured ``user.name`` and ``user.email``.
- ``message <file>`` (``commit-msg``) removes attribution lines from the message:
  ``Co-Authored-By`` and ``Claude-Session`` trailers and "Generated with" footers.
- ``push <remote>`` (``pre-push``) refuses to push a commit that carries either, so a
  commit made without the hooks, or history merged in from elsewhere, never reaches
  the remote.

``history`` checks every commit reachable from HEAD for local verification. This
repository has no hosted CI; enabled local hooks enforce the commit and push checks.
"""

from __future__ import annotations

import re
import subprocess
import sys
from collections.abc import Iterable
from pathlib import Path
from typing import NamedTuple

# A message line that attributes the commit to an AI tool.
ATTRIBUTION_LINE = re.compile(
    r"^\s*(?:co-authored-by:|claude-session:|"
    r"(?:\U0001f916\s*)?generated (?:with|by)\b)",
    re.IGNORECASE,
)
NO_COMMIT = "0" * 40
FIELD = "\x1f"
RECORD = "\x1e"
REMEDY = (
    "Commits in this repository carry only the user's authorship: no AI tool as author or "
    "committer, no Co-Authored-By or Claude-Session trailers, no 'Generated with' footers."
)


class Commit(NamedTuple):
    sha: str
    author: str
    committer: str
    message: str


class AuthorshipError(Exception):
    """A repository identity that cannot be checked."""


def _git(*args: str) -> str:
    return subprocess.run(
        ["git", *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=True,
    ).stdout


def user_identity() -> str:
    """Return the repository's configured user without author/committer overrides."""
    try:
        name = _git("config", "user.name").strip()
        email = _git("config", "user.email").strip()
    except subprocess.CalledProcessError as exc:
        raise AuthorshipError("configure user.name and user.email locally before committing; "
                              "the repository user identity is missing") from exc
    if not name or not email:
        raise AuthorshipError("configure nonempty user.name and user.email locally before committing")
    return f"{name} <{email}>"


def clean_message(message: str) -> str:
    """Return ``message`` without attribution lines and the blank lines they leave behind."""
    lines = [line for line in message.splitlines() if not ATTRIBUTION_LINE.match(line)]
    while lines and not lines[-1].strip():
        lines.pop()
    return "\n".join(lines) + "\n" if lines else ""


def problems(commit: Commit, expected_identity: str) -> list[str]:
    """Return what makes ``commit`` carry attribution other than the user's."""
    found = [
        f"{role} {ident}"
        for role, ident in (("author", commit.author), ("committer", commit.committer))
        if ident != expected_identity
    ]
    found += [
        f"message line {line.strip()!r}"
        for line in commit.message.splitlines()
        if ATTRIBUTION_LINE.match(line)
    ]
    return found


def commits(*revisions: str) -> list[Commit]:
    """Return the commits that ``git log`` selects with these revision arguments."""
    output = _git(
        "log", f"--format=%H{FIELD}%an <%ae>{FIELD}%cn <%ce>{FIELD}%B{RECORD}", *revisions
    )
    found = []
    for record in output.split(RECORD):
        record = record.lstrip("\n")
        if record:
            sha, author, committer, message = record.split(FIELD, 3)
            found.append(Commit(sha, author, committer, message))
    return found


def _report(offending: Iterable[Commit]) -> int:
    expected = user_identity()
    lines = []
    for commit in offending:
        found = problems(commit, expected)
        if found:
            lines.append(f"  {commit.sha[:12]}: {'; '.join(found)}")
    if not lines:
        return 0
    print(f"These commits differ from the configured user {expected} or carry tool attribution:",
          *lines, sep="\n", file=sys.stderr)
    print(
        f"\n{REMEDY}\nRewrite them with the user's identity and without those lines "
        "only for unpublished commits you own, then try again. Published history "
        "requires the user's approval before rewriting. Do not bypass this check "
        "with --no-verify.",
        file=sys.stderr,
    )
    return 1


def check_identity() -> int:
    expected = user_identity()
    # git var prints "Name <email> <timestamp> <zone>".
    idents = {
        role: _git("var", variable).rsplit(">", 1)[0] + ">"
        for role, variable in (("author", "GIT_AUTHOR_IDENT"), ("committer", "GIT_COMMITTER_IDENT"))
    }
    refused = [f"  {role}: {ident}" for role, ident in idents.items() if ident != expected]
    if not refused:
        return 0
    print("Commit blocked: author and committer must match the repository's configured user.",
          *refused, sep="\n")
    print(
        f"\n{REMEDY}\nConfigured user: {expected}. Unset GIT_AUTHOR_NAME, GIT_AUTHOR_EMAIL, "
        "GIT_COMMITTER_NAME and GIT_COMMITTER_EMAIL when they override this identity, "
        "then commit again. Correct user.name and user.email locally if they name "
        "someone else (never --global)."
    )
    return 1


def strip_message(path: Path) -> int:
    message = path.read_text(encoding="utf-8")
    cleaned = clean_message(message)
    if message.strip() and not cleaned.strip():
        print("Commit blocked: removing attribution would leave an empty message.", file=sys.stderr)
        print(REMEDY, file=sys.stderr)
        return 1
    if cleaned != message:
        with path.open("w", encoding="utf-8", newline="\n") as handle:
            handle.write(cleaned)
    return 0


def check_push(remote: str, updates: str) -> int:
    offending: list[Commit] = []
    for update in updates.splitlines():
        parts = update.split()
        if len(parts) != 4 or parts[1] == NO_COMMIT:
            continue
        offending += commits(parts[1], "--not", f"--remotes={remote}")
    return _report(offending)


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    command, *rest = argv or [""]
    try:
        if command == "identity":
            return check_identity()
        if command == "message" and len(rest) == 1:
            return strip_message(Path(rest[0]))
        if command == "push" and rest:
            return check_push(rest[0], sys.stdin.read())
        if command == "history":
            return _report(commits("HEAD"))
    except AuthorshipError as exc:
        print(f"Authorship check failed: {exc}", file=sys.stderr)
        return 1
    print("usage: commit_authorship.py identity | message <file> | push <remote> | history")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
