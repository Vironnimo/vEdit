#!/usr/bin/env python3
"""Run stable Rust tests in separate processes with deadlines and timing reports."""

import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone


ROOT = Path(__file__).resolve().parent.parent
SUCCESS = re.compile(r"test result: ok\. 1 passed; 0 failed; 0 ignored;")


def run_process(command, cwd, env, log, timeout):
    """Keep output on disk; kill only this command's process tree on timeout."""
    started = time.monotonic()
    with log.open("wb") as output:
        options = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {"start_new_session": True}
        process = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                   stdout=output, stderr=subprocess.STDOUT, **options)
        timed_out = False
        cleanup_error = None
        try:
            process.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            timed_out = True
            if os.name == "nt":
                # /T follows this owned process's descendants, never image names.
                try:
                    cleanup = subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                                             stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT,
                                             timeout=15, check=False, creationflags=subprocess.CREATE_NO_WINDOW)
                    if cleanup.returncode:
                        cleanup_error = f"Process-tree cleanup returned {cleanup.returncode}; stopping the owned parent."
                except (OSError, subprocess.TimeoutExpired) as error:
                    cleanup_error = f"Process-tree cleanup failed; stopping the owned parent: {error}"
                if cleanup_error:
                    output.write(("\n" + cleanup_error + "\n").encode("utf-8", errors="replace"))
            else:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        result = {"returncode": process.returncode, "timed_out": timed_out,
                  "seconds": round(time.monotonic() - started, 6), "log": str(log)}
        if cleanup_error:
            result["cleanup_error"] = cleanup_error
        return result


def checkout_environment(root):
    env = os.environ.copy()
    temporary = root / ".tmp"
    temporary.mkdir(exist_ok=True)
    for key in ("TEMP", "TMP", "TMPDIR", "GIT_CEILING_DIRECTORIES"):
        env[key] = str(temporary)
    env.update({"GIT_TERMINAL_PROMPT": "0", "GCM_INTERACTIVE": "never", "CARGO_TERM_COLOR": "never"})
    return env


def case_environment(env, directory):
    """Isolate runtime user state without hiding Rustup/Cargo during the build."""
    env = env.copy()
    home = directory / "home"
    home.mkdir(parents=True)
    for key in list(env):
        if key.startswith("GIT_CONFIG_") or key in {
            "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "XDG_CONFIG_HOME", "VEDIT_CONTROL_DIR", "MORROW_CONTROL_DIR",
        }:
            env.pop(key)
    empty_git_config = home / ".gitconfig"
    empty_git_config.touch()
    env.update({"HOME": str(home), "USERPROFILE": str(home),
                "APPDATA": str(home / "AppData/Roaming"),
                "LOCALAPPDATA": str(home / "AppData/Local"),
                "VEDIT_PROFILE_DIR": str(directory / "profile"),
                "GIT_CONFIG_GLOBAL": str(empty_git_config), "GIT_CONFIG_NOSYSTEM": "1"})
    return env


def parse_test_names(output):
    names = []
    for line in output.splitlines():
        if line.endswith(": test"):
            names.append(line[:-6])
        elif line.endswith(": benchmark"):
            raise ValueError("benchmark harnesses need a separate explicit run")
    if len(names) != len(set(names)):
        raise ValueError("the test harness listed duplicate test names")
    return names


def discover_tests(command, root, env, directory):
    directory.mkdir(parents=True, exist_ok=True)
    discovered = []
    for label, flags in (("all", []), ("ignored", ["--ignored"])):
        log = directory / (label + ".log")
        result = run_process(command + ["--list", "--format", "terse"] + flags,
                             root, env, log, 30)
        if result["returncode"] != 0 or result["timed_out"]:
            raise ValueError("test discovery failed; read " + str(log))
        discovered.append(parse_test_names(log.read_text(encoding="utf-8", errors="replace")))
    names, ignored = discovered
    if not set(ignored).issubset(names):
        raise ValueError("ignored tests are missing from the complete harness listing")
    return names, set(ignored)


def execute_case(case, root, env, run_directory, timeout, cancelled):
    if cancelled.is_set():
        return dict(case, status="cancelled", seconds=None, log=None)
    identity = case["target"] + "::" + case["name"]
    directory = run_directory / "cases" / hashlib.sha256(identity.encode()).hexdigest()[:20]
    case_env = case_environment(env, directory)
    log = directory / "output.log"
    result = run_process(case["command"] + ["--exact", case["name"], "--nocapture", "--test-threads=1", "--color", "never"],
                         root, case_env, log, timeout)
    if result["timed_out"]:
        status = "timeout"
    elif result["returncode"] != 0:
        status = "failed"
    elif SUCCESS.search(log.read_text(encoding="utf-8", errors="replace")):
        status = "passed"
    else:
        status = "failed"
        result["error"] = "Harness did not confirm exactly one passing test."
    return dict(case, status=status, **result)


def summarize(report):
    results = report["tests"]
    report["counts"] = {status: sum(case["status"] == status for case in results)
                        for status in ("passed", "failed", "timeout", "ignored", "cancelled")}
    report["counts"]["total"] = len(results)
    timed = [case for case in results if case.get("seconds") is not None]
    report["slowest"] = sorted(timed, key=lambda case: case["seconds"], reverse=True)[:20]


def write_report(report, directory):
    summarize(report)
    (directory / "report.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    counts = report["counts"]
    lines = ["# Rust test survey", "", "Started: " + report["started"], "",
             f"Total wall time: {report['seconds']:.3f} seconds; build: {report.get('build_seconds', 0):.3f} seconds; test execution: {report.get('test_seconds', 0):.3f} seconds.",
             f"Tests: {counts['total']}; passed: {counts['passed']}; failed: {counts['failed']}; timed out: {counts['timeout']}; ignored: {counts['ignored']}; cancelled: {counts['cancelled']}.", "",
             "Durations include per-test process startup and fresh process caches. They are not timings from one native Cargo test run.", "",
             "## Slowest 20", "", "| Seconds | Status | Target | Test |", "|---:|---|---|---|"]
    for case in report["slowest"]:
        lines.append(f"| {case['seconds']:.3f} | {case['status']} | {case['target']} | {case['name']} |")
    lines += ["", "## Failures, timeouts and ignored tests", ""]
    for case in report["tests"]:
        if case["status"] != "passed":
            lines.append(f"- {case['status']}: {case['target']}::{case['name']}" + ("; log: " + case["log"] if case.get("log") else ""))
    if report.get("error"):
        lines += ["", "Error: " + report["error"]]
    (directory / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def positive_number(value):
    result = float(value)
    if not 0 < result < float("inf"):
        raise argparse.ArgumentTypeError("use a positive finite number of seconds")
    return result


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise ValueError(message)


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    parser = Parser(description="Build and survey all non-ignored Rust tests with per-test process deadlines.",
                    epilog="Reports and logs stay in .tmp/test-survey/<run>/. Exit: 0 passed, 1 failed, 2 invalid call. Direct cargo test keeps the normal shared-process harness.")
    parser.add_argument("--jobs", type=int, default=8, help="parallel test processes, 1 to 8. Omit for 8.")
    parser.add_argument("--timeout", type=positive_number, default=30.0, help="per-test deadline in seconds. Omit for 30.")
    parser.add_argument("--build-timeout", type=positive_number, default=1800.0, help="Cargo build deadline in seconds. Omit for 1800.")
    parser.add_argument("--json", action="store_true", help="print one JSON report instead of progress and the summary.")
    report = None
    run_directory = None
    try:
        args = parser.parse_args(argv)
        if not 1 <= args.jobs <= 8:
            raise ValueError("--jobs must be between 1 and 8")
        if os.environ.get("CARGO_TARGET_DIR"):
            raise ValueError("unset CARGO_TARGET_DIR; this checkout builds in its own target directory")
        if any(key in os.environ for key in ("UPDATE_SNAPSHOTS", "VEDIT_SNAPSHOT_DIR")):
            raise ValueError("unset UPDATE_SNAPSHOTS and VEDIT_SNAPSHOT_DIR before a verification survey")
    except ValueError as error:
        result = {"error": str(error), "exit_code": 2}
        print(json.dumps(result) if "--json" in argv else "Error: " + str(error))
        return 2
    started = time.monotonic()
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    run_directory = ROOT / ".tmp/test-survey" / (stamp + "-" + str(os.getpid()))
    run_directory.mkdir(parents=True)
    report = {"started": datetime.now(timezone.utc).isoformat(), "root": str(ROOT), "directory": str(run_directory),
              "jobs": args.jobs, "timeout": args.timeout, "tests": [], "error": None}
    env = checkout_environment(ROOT)
    if os.name == "nt":
        env.setdefault("WGPU_BACKEND", "dx12")
    report["wgpu_backend_request"] = env.get("WGPU_BACKEND")
    cancelled = threading.Event()
    try:
        if not args.json:
            print("Building Rust test executables; logs: " + str(run_directory), flush=True)
        toolchain = run_process(["rustc", "--version"], ROOT, env, run_directory / "toolchain.log", 30)
        if toolchain["returncode"] != 0 or toolchain["timed_out"]:
            raise ValueError("Rust toolchain unavailable; read " + toolchain["log"])
        report["rustc"] = (run_directory / "toolchain.log").read_text(encoding="utf-8").strip()
        build_log = run_directory / "build.log"
        build_command = ["cargo", "test", "--locked", "--all-targets", "--no-run", "--message-format=json"]
        report["build_command"] = build_command
        build = run_process(build_command, ROOT, env, build_log, args.build_timeout)
        report["build_seconds"] = build["seconds"]
        if build["returncode"] != 0 or build["timed_out"]:
            raise ValueError("Cargo test build failed or timed out; read " + str(build_log))
        executables = {}
        for line in build_log.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if message.get("reason") == "compiler-artifact" and message.get("profile", {}).get("test") and message.get("executable"):
                executable = Path(message["executable"]).resolve()
                if not executable.is_relative_to(ROOT / "target"):
                    raise ValueError("Cargo emitted a test executable outside this checkout's target/: " + str(executable))
                executables[str(executable)] = message["target"]["name"]
        if not executables:
            raise ValueError("Cargo produced no Rust test executables; read " + str(build_log))
        runnable = []
        for index, (executable, target) in enumerate(sorted(executables.items())):
            names, ignored = discover_tests([executable], ROOT, env, run_directory / "discovery" / str(index))
            for name in names:
                case = {"target": target, "name": name, "command": [executable]}
                if name in ignored:
                    report["tests"].append(dict(case, status="ignored", seconds=None, log=None))
                else:
                    runnable.append(case)
        if not runnable:
            raise ValueError("No runnable tests were discovered")
        if not args.json:
            print(f"Running {len(runnable)} tests with {args.jobs} workers and a {args.timeout:g}-second deadline; {len(report['tests'])} ignored.", flush=True)
        test_started = time.monotonic()
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as workers:
            futures = [workers.submit(execute_case, case, ROOT, env, run_directory, args.timeout, cancelled) for case in runnable]
            try:
                for index, future in enumerate(concurrent.futures.as_completed(futures), 1):
                    result = future.result()
                    report["tests"].append(result)
                    if not args.json and (index % 100 == 0 or result["status"] != "passed"):
                        print(f"{index}/{len(runnable)}: {result['status']} {result['target']}::{result['name']}", flush=True)
            except BaseException:
                cancelled.set()
                raise
        report["test_seconds"] = round(time.monotonic() - test_started, 6)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
    except KeyboardInterrupt:
        report["error"] = "Survey interrupted; pending tests were cancelled."
    report["seconds"] = round(time.monotonic() - started, 6)
    report["tests"].sort(key=lambda case: (case["target"], case["name"]))
    write_report(report, run_directory)
    counts = report["counts"]
    exit_code = int(bool(report["error"] or counts["failed"] or counts["timeout"] or counts["cancelled"]))
    if args.json:
        print(json.dumps(report, ensure_ascii=False))
    else:
        print(("Error: " if exit_code else "Survey passed: ") + f"{counts['passed']} passed, {counts['failed']} failed, {counts['timeout']} timed out, {counts['ignored']} ignored; {report['seconds']:.3f} seconds.")
        if report["error"]:
            print("Error: " + report["error"])
        print("Report: " + str(run_directory / "report.md"))
    return exit_code


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
