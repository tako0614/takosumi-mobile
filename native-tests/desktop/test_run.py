#!/usr/bin/env python3
"""Linux raw-fixture protocol controls; fake binaries supply no engine evidence."""

import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
import run as fixture_run

ROOT = Path(__file__).resolve().parents[2]


class CapturedSessionTests(unittest.TestCase):
    def test_a_failing_session_is_replayed_from_its_file(self):
        with tempfile.TemporaryDirectory(prefix="sumi-capture-") as temporary:
            path = Path(temporary) / "session.log"
            replayed = io.StringIO()
            with contextlib.redirect_stdout(replayed), self.assertRaises(subprocess.CalledProcessError):
                fixture_run.run_captured(
                    [sys.executable, "-c",
                     "import sys; print('session wrote this', flush=True); "
                     "print('and this to stderr', file=sys.stderr, flush=True); sys.exit(3)"],
                    dict(os.environ), path)
            self.assertIn("session wrote this\n", replayed.getvalue())
            self.assertIn("and this to stderr\n", replayed.getvalue())
            self.assertEqual(path.read_text(), replayed.getvalue())

    def test_a_timed_out_session_is_replayed_with_its_bound_unchanged(self):
        def timed_out(command, env, timeout, *, stdout, stderr):
            self.assertEqual(timeout, 50)
            self.assertIs(stderr, subprocess.STDOUT)
            stdout.write("written before the bound\n")
            stdout.flush()
            raise subprocess.TimeoutExpired(command, timeout)

        with tempfile.TemporaryDirectory(prefix="sumi-capture-") as temporary:
            replayed = io.StringIO()
            with mock.patch.object(fixture_run, "run_desktop_session", side_effect=timed_out), \
                    contextlib.redirect_stdout(replayed), self.assertRaises(subprocess.TimeoutExpired):
                fixture_run.run_captured(["synthetic-session"], {}, Path(temporary) / "session.log")
            self.assertEqual(replayed.getvalue(), "written before the bound\n")

    def test_a_successful_session_is_replayed_and_returned(self):
        with tempfile.TemporaryDirectory(prefix="sumi-capture-") as temporary:
            replayed = io.StringIO()
            with contextlib.redirect_stdout(replayed):
                captured = fixture_run.run_captured([sys.executable, "-c", "print('done', flush=True)"],
                                                    dict(os.environ), Path(temporary) / "session.log")
            self.assertEqual(captured, "done\n")
            self.assertEqual(replayed.getvalue(), "done\n")


class ErrorPageProtocolTests(unittest.TestCase):
    def test_an_old_fixture_cannot_qualify_profile_reset(self):
        with tempfile.TemporaryDirectory(prefix="sumi-raw-reset-protocol-") as temporary:
            root = Path(temporary)
            binary = root / "old-fixture"
            binary.write_text("#!/usr/bin/env python3\nprint('Cookie/storage observations passed: 6, readback=false')\n")
            binary.chmod(0o700)
            result = subprocess.run(
                [sys.executable, "-B", str(ROOT / "native-tests/desktop/run.py"),
                 "--binary", str(binary), "--data", str(root / "data"), "--profile-reset"],
                env=dict(os.environ), text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30,
            )
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn("Native writer did not report: PASS: completed native profile reset", result.stdout)

    def test_missing_document_cannot_hide_an_abnormal_native_helper(self):
        with tempfile.TemporaryDirectory(prefix="sumi-raw-negative-helper-") as temporary:
            root = Path(temporary)
            binary = root / "fake-fixture"
            binary.write_text(
                "#!/usr/bin/env python3\nimport ctypes, os, sys, time\nfrom pathlib import Path\n"
                "Path(os.environ['TAKOSUMI_FIXTURE_DATA']).mkdir(exist_ok=True)\n"
                "negative = '--missing-document' in sys.argv\n"
                "pid = os.fork()\nif pid == 0:\n"
                "    ctypes.CDLL(None).prctl(15, b'WebKitNetworkPr', 0, 0, 0)\n"
                "    time.sleep(0.3)\n    os._exit(7 if negative else 0)\n"
                "time.sleep(0.1)\n"
                "if negative: print('expected fixture document not ready', flush=True)\n"
                "sys.exit(1 if negative else 0)\n"
            )
            binary.chmod(0o700)
            result = subprocess.run(
                [sys.executable, "-B", str(ROOT / "native-tests/desktop/run.py"),
                 "--binary", str(binary), "--data", str(root / "data")],
                env=dict(os.environ), text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30,
            )
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn("NetworkProcess exited abnormally", result.stdout)
            self.assertNotIn("PASS: missing document rejects", result.stdout)

    def test_abnormal_native_helper_refuses_before_readback(self):
        with tempfile.TemporaryDirectory(prefix="sumi-raw-helper-") as temporary:
            root = Path(temporary)
            attempts = root / "attempts.jsonl"
            binary = root / "fake-fixture"
            binary.write_text(
                "#!/usr/bin/env python3\nimport ctypes, json, os, sys, time\nfrom pathlib import Path\n"
                f"with Path({str(attempts)!r}).open('a') as log: log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
                "pid = os.fork()\nif pid == 0:\n"
                "    ctypes.CDLL(None).prctl(15, b'WebKitNetworkPr', 0, 0, 0)\n"
                "    time.sleep(0.3)\n    os._exit(7)\n"
                "time.sleep(0.1)\nprint('synthetic fixture parent succeeded', flush=True)\n"
            )
            binary.chmod(0o700)
            result = subprocess.run(
                [sys.executable, "-B", str(ROOT / "native-tests/desktop/run.py"),
                 "--binary", str(binary), "--data", str(root / "data")],
                env=dict(os.environ), text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30,
            )
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn("NetworkProcess exited abnormally", result.stdout)
            calls = [json.loads(line) for line in attempts.read_text().splitlines()]
            self.assertEqual(calls, [[]], "An abnormal native helper must refuse before readback")

    def test_an_old_or_partial_fixture_cannot_qualify_recovery(self):
        base = "PASS: stock error document committed and finished before native Close"
        for action, messages in ((None, []), ("--error-page-back", [base]), ("--error-page-retry", [base]), ("--cancelled-spa", [])):
            with self.subTest(action=action), tempfile.TemporaryDirectory(prefix="sumi-raw-protocol-") as temporary:
                root = Path(temporary)
                attempts = root / "attempts.jsonl"
                binary = root / "old-fixture"
                binary.write_text(
                    "#!/usr/bin/env python3\nimport json, sys\nfrom pathlib import Path\n"
                    f"with Path({str(attempts)!r}).open('a') as log: log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
                    + "\n".join(f"print({message!r})" for message in messages)
                    + "\nprint('PASS: new process restored native pushState route without a link click')\n"
                )
                binary.chmod(0o700)
                controls = (["--cancelled-navigation", "--cancelled-spa"] if action == "--cancelled-spa" else
                            ["--failed-navigation", "--error-page-close", *([action] if action else [])])
                result = subprocess.run(
                    [sys.executable, "-B", str(ROOT / "native-tests/desktop/run.py"),
                     "--binary", str(binary), "--data", str(root / "data"),
                     "--spa-resume", *controls],
                    env=dict(os.environ), text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30,
                )
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("Native writer did not report", result.stdout)
                calls = [json.loads(line) for line in attempts.read_text().splitlines()]
                self.assertEqual(len(calls), 1, "An unsupported writer must refuse before starting readback")
                self.assertNotIn("--readback", calls[0])


if __name__ == "__main__":
    unittest.main()
