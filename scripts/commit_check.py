#!/usr/bin/env python
"""Check staged source and domain maps without changing the index or working files.

The hook runs static checks only. Rustfmt reads one index blob through stdin, so
module traversal cannot format another file. Clippy compiles the whole project
and belongs in the complete push check; running it over this working tree would
check unrelated or partially staged work instead of the proposed commit.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import subprocess
import sys
from pathlib import Path, PurePosixPath

PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))
sys.dont_write_bytecode = True
import domain_maps

TOOLCHAIN = "1.97.1"
IGNORED_ROOTS = ("vendor/",)


def git(root: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", *args], cwd=root, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
    )
    if result.returncode:
        raise RuntimeError(result.stderr.decode("utf-8", errors="replace").strip())
    return result.stdout


def staged_paths(root: Path, all_files: bool = False) -> list[str]:
    """Include files deleted only in the working tree; their index blobs exist."""
    output = (git(root, "ls-files", "--cached", "-z") if all_files else
              git(root, "diff", "--cached", "--name-only", "--diff-filter=ACMR", "-z"))
    return sorted(path for path in output.decode("utf-8").split("\0")
                  if path and not path.startswith(IGNORED_ROOTS))


def rustfmt(path: str, source: bytes, root: Path) -> list[str]:
    environment = dict(os.environ)
    temp = root / ".tmp" / "commit-check"
    temp.mkdir(parents=True, exist_ok=True)
    for key in ("TEMP", "TMP", "TMPDIR", "GIT_CEILING_DIRECTORIES"):
        environment[key] = str(temp)
    try:
        result = subprocess.run(
            ["rustup", "run", TOOLCHAIN, "rustfmt", "--edition", "2024", "--emit", "stdout"],
            input=source, cwd=temp, env=environment, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, check=False, timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return [f"{path}: Rustfmt could not run: {exc}. Install Rust {TOOLCHAIN} "
                "with the rustfmt and clippy components; the hook installs nothing."]
    if result.returncode:
        return [f"{path}: Rustfmt failed:\n" + result.stderr.decode("utf-8", errors="replace")]
    before = source.decode("utf-8").replace("\r\n", "\n")
    after = result.stdout.decode("utf-8").replace("\r\n", "\n")
    if before == after:
        return []
    difference = list(difflib.unified_diff(before.splitlines(), after.splitlines(),
                      fromfile=f"staged/{path}", tofile=f"formatted/{path}", lineterm=""))
    excerpt = "\n".join(difference[:60])
    if len(difference) > 60:
        excerpt += f"\n... {len(difference) - 60} more diff lines"
    return [f"{path}: staged Rust needs formatting:\n{excerpt}"]


def check(root: Path, all_files: bool = False) -> dict:
    staged = staged_paths(root, all_files)
    blocking: list[str] = []
    notes: list[str] = []
    try:
        import tomllib
    except ImportError:
        tomllib = None
    for path in staged:
        suffix = PurePosixPath(path).suffix
        if suffix not in {".rs", ".py", ".json", ".toml"}:
            continue
        source = git(root, "show", f":{path}")
        try:
            if suffix == ".rs":
                blocking.extend(rustfmt(path, source, root))
            elif suffix == ".py":
                compile(source, path, "exec")
            elif suffix == ".json":
                json.loads(source)
            elif tomllib is not None:
                tomllib.loads(source.decode("utf-8"))
            else:
                notes.append(f"{path}: TOML syntax check needs Python 3.11+; "
                             "Cargo validates its manifests in the complete check.")
        except (ValueError, SyntaxError, UnicodeError) as exc:
            blocking.append(f"{path}: {exc}")
    whitespace = subprocess.run(
        ["git", "diff", "--cached", "--check"], cwd=root, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False,
    )
    if whitespace.returncode:
        blocking.append(whitespace.stdout.decode("utf-8", errors="replace").strip())
    for finding in domain_maps.staged_findings(root, staged):
        (blocking if finding.blocking else notes).append(str(finding))
    return {"ok": not blocking, "scope": "index" if all_files else "staged",
            "files": staged, "errors": blocking, "notes": notes}


class Parser(argparse.ArgumentParser):
    json_mode = False

    def error(self, message):
        print(json.dumps({"ok": False, "errors": [message]}) if self.json_mode else
              f"Error: {message}. Nothing was changed. Use --help for accepted arguments.")
        raise SystemExit(2)


def main(argv: list[str] | None = None) -> int:
    parser = Parser(description=__doc__.splitlines()[0],
        epilog="Exit 0: passed; 1: check failed; 2: invalid arguments. No tests or autofixes.")
    parser.json_mode = "--json" in (sys.argv[1:] if argv is None else argv)
    parser.add_argument("--all", action="store_true",
                        help="check every tracked index blob; omit to check staged changes only")
    parser.add_argument("--json", action="store_true", help="print one JSON result document")
    args = parser.parse_args(argv)
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    try:
        result = check(PROJECT_ROOT, args.all)
    except (OSError, RuntimeError, UnicodeError) as exc:
        result = {"ok": False, "scope": "index" if args.all else "staged",
                  "files": [], "errors": [str(exc)], "notes": []}
    if args.json:
        print(json.dumps(result, ensure_ascii=True))
    else:
        print(f"Commit check: {len(result['files'])} {result['scope']} files; "
              f"{'passed' if result['ok'] else 'blocked'}.")
        for note in result["notes"]:
            print(f"Note: {note}")
        for error in result["errors"]:
            print(f"Error: {error}")
        if not result["ok"]:
            print("Working files and index were not changed. Fix and stage only the intended "
                  "changes, then commit again.")
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
