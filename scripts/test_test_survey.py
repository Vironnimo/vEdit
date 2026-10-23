#!/usr/bin/env python3
"""Exercise the survey runner against tiny processes, never the vEdit suite."""

import ctypes
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch


sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("test_survey", Path(__file__).with_name("test_survey.py"))
survey = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(survey)
FIXTURE = '''import os, subprocess, sys, time
from pathlib import Path
if "--list" in sys.argv:
    names = ["optional"] if "--ignored" in sys.argv else ["pass", "fail", "hang", "empty", "optional"]
    for name in names:
        print(name + ": test")
elif "--exact" in sys.argv:
    name = sys.argv[sys.argv.index("--exact") + 1]
    if name == "hang":
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        Path(os.environ["USERPROFILE"]).joinpath("child.pid").write_text(str(child.pid))
        time.sleep(60)
    elif name == "fail":
        print("test result: FAILED. 0 passed; 1 failed; 0 ignored;")
        sys.exit(101)
    elif name == "empty":
        print("test result: ok. 0 passed; 0 failed; 0 ignored;")
    else:
        print("test result: ok. 1 passed; 0 failed; 0 ignored;")
'''


class SurveyTests(unittest.TestCase):
    def setUp(self):
        root = survey.ROOT / ".tmp/test-survey-smoke"
        root.mkdir(parents=True, exist_ok=True)
        self.fixture = tempfile.TemporaryDirectory(dir=root)
        self.directory = Path(self.fixture.name)
        self.program = self.directory / "fake_harness.py"
        self.program.write_text(FIXTURE, encoding="utf-8")
        self.env = survey.checkout_environment(survey.ROOT)

    def tearDown(self):
        self.fixture.cleanup()

    def case(self, name):
        return survey.execute_case({"target": "fake", "name": name, "command": [sys.executable, str(self.program)]},
                                   survey.ROOT, self.env, self.directory, 1.0, threading.Event())

    def test_listing_counts_ignored_without_running_them(self):
        names, ignored = survey.discover_tests([sys.executable, str(self.program)], survey.ROOT, self.env,
                                              self.directory / "discovery")
        self.assertEqual(len(names), 5)
        self.assertEqual(ignored, {"optional"})

    def test_exact_success_failure_and_empty_success(self):
        results = [self.case(name) for name in ("pass", "fail", "empty")]
        self.assertEqual([result["status"] for result in results], ["passed", "failed", "failed"])
        report = {"started": "2026-01-01T00:00:00+00:00", "seconds": 1.0, "tests": results}
        survey.write_report(report, self.directory)
        saved = json.loads((self.directory / "report.json").read_text(encoding="utf-8"))
        self.assertEqual(saved["counts"]["total"], 3)
        self.assertEqual(saved["counts"]["failed"], 2)
        self.assertEqual(len(saved["slowest"]), 3)

    def test_timeout_stops_owned_child_process(self):
        result = self.case("hang")
        self.assertEqual(result["status"], "timeout")
        pid = int(next(self.directory.glob("cases/*/home/child.pid")).read_text())
        if os.name == "nt":
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.OpenProcess.restype = ctypes.c_void_p
            kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
            kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
            kernel.CloseHandle.argtypes = [ctypes.c_void_p]
            handle = kernel.OpenProcess(0x100000, False, pid)
            if handle:
                try:
                    self.assertEqual(kernel.WaitForSingleObject(handle, 5000), 0)
                finally:
                    kernel.CloseHandle(handle)
            for cleanup_error in (OSError("cleanup unavailable"),
                                  subprocess.TimeoutExpired("taskkill", 15)):
                with self.subTest(cleanup_error=type(cleanup_error).__name__):
                    process = Mock(pid=12345, returncode=-9)
                    process.wait.side_effect = [subprocess.TimeoutExpired("fixture", 1),
                                                subprocess.TimeoutExpired("fixture", 5), None]
                    with patch.object(survey.subprocess, "Popen", return_value=process), \
                         patch.object(survey.subprocess, "run", side_effect=cleanup_error):
                        cleanup = survey.run_process(["fixture"], survey.ROOT, self.env,
                                                     self.directory / "cleanup.log", 1)
                    process.kill.assert_called_once_with()
                    self.assertTrue(cleanup["timed_out"])
                    self.assertIn("Process-tree cleanup failed", cleanup["cleanup_error"])
        else:
            stat = Path(f"/proc/{pid}/stat")
            if stat.exists():
                self.assertEqual(stat.read_text().split(") ", 1)[1][0], "Z")

    def test_case_environment_is_private_and_temp_contract_is_preserved(self):
        inherited = dict(self.env, GIT_DIR="wrong", VEDIT_CONTROL_DIR="wrong", XDG_CONFIG_HOME="wrong")
        env = survey.case_environment(inherited, self.directory / "isolation")
        self.assertNotIn("GIT_DIR", env)
        self.assertNotIn("VEDIT_CONTROL_DIR", env)
        self.assertNotIn("XDG_CONFIG_HOME", env)
        self.assertEqual(env["TEMP"], str(survey.ROOT / ".tmp"))
        self.assertEqual(env["GIT_CEILING_DIRECTORIES"], env["TEMP"])
        self.assertEqual(env["HOME"], env["USERPROFILE"])
        self.assertTrue(Path(env["VEDIT_PROFILE_DIR"]).is_relative_to(self.directory))
        self.assertEqual(inherited["VEDIT_CONTROL_DIR"], "wrong")

    def test_cli_help_and_invalid_json_call(self):
        script = str(Path(__file__).with_name("test_survey.py"))
        for flags, code in ((["--help"], 0), (["--jobs", "0", "--json"], 2)):
            result = subprocess.run([sys.executable, script] + flags, capture_output=True, text=True,
                                    encoding="utf-8", env=self.env, timeout=10, check=False)
            self.assertEqual(result.returncode, code)
            self.assertEqual(result.stderr, "")
            if code:
                self.assertEqual(json.loads(result.stdout)["exit_code"], 2)
            else:
                self.assertIn("--timeout", result.stdout)


if __name__ == "__main__":
    unittest.main()
