#!/usr/bin/env python3
"""Positive and stale/foreign-generation controls; no CEF or network needed."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
from unittest.mock import patch
import browser_process as runner

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("owned_guard", REPO / "native-tests/browser/guard.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


def main():
    if not __debug__:
        raise RuntimeError("Control assertions must be enabled; optimized child behavior is tested separately")
    data = Path(tempfile.mkdtemp(prefix="sumi-snapshot-control-", dir="/root/dev/tmp"))
    (data / ".owned-browser-reference").write_text("generation controls; not a browser capability case")
    receipt = data / "guard.json"
    child = None
    proof = {"passed": False, "controls": []}
    scratch, outside = data / "scratch-root", data / "outside-root"
    scratch.mkdir()
    outside.mkdir()
    (scratch / "escape").symlink_to(outside, target_is_directory=True)
    (outside / ".owned-browser-reference").write_text("synthetic marker outside scratch")
    with patch.object(runner, "SCRATCH_ROOT", scratch):
        assert runner.new_case_path(scratch / "new-case") == scratch / "new-case"
        for candidate in [scratch / ".." / "outside-root" / "new-case", scratch / "escape" / "new-case"]:
            try:
                runner.new_case_path(candidate)
            except ValueError:
                pass
            else:
                raise AssertionError("Scratch path escape was accepted")
        try:
            runner.owned_case_path(scratch / "escape")
        except ValueError:
            pass
        else:
            raise AssertionError("Existing worker/snapshot case escaped scratch")
    assert not (outside / "new-case").exists()
    proof["controls"].append({"name": "canonical scratch paths reject traversal and parent symlinks", "passed": True})
    fake_repo = data / "source-scope"
    for name in (*runner.LINUX_SOURCES, "src/good.ts", "src/untracked.ts", ".env.private", "credential.pem", "operator-private.json"):
        file = fake_repo / name
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text("synthetic scope fixture, no credential\n")
    tracked = "src/good.ts\nsrc/operator-private.json\n"
    with patch.object(runner.subprocess, "check_output", return_value=tracked):
        selected = {str(file.relative_to(fake_repo)) for file in runner.source_files(fake_repo)}
    assert selected == set(runner.LINUX_SOURCES) | {"src/good.ts"}
    # Even a tracked source symlink must not capture bytes outside the repo.
    (fake_repo / "src/escape.ts").symlink_to(data / "outside-source")
    (data / "outside-source").write_text("synthetic private bytes\n")
    with patch.object(runner.subprocess, "check_output", return_value=tracked+"src/escape.ts\n"):
        try:
            runner.source_files(fake_repo)
        except ValueError:
            pass
        else:
            raise AssertionError("External source symlink was not rejected")
    proof["controls"].append({"name": "scoped source selection excludes unrelated files and external symlinks", "passed": True})
    optimized_check = """
import pathlib,sys
from unittest.mock import patch
sys.path.insert(0,sys.argv[1])
import browser_process as r
bad={'rootExit':0,'complete':True,'kernelNoChildren':True,'remaining':[],'forcedCleanup':1,'cleanupErrors':[],'error':None}
try:r.normal(bad)
except ValueError:pass
else:raise SystemExit('Optimized execution accepted forced cleanup')
with patch.object(r.subprocess,'check_output',return_value='src/escape.ts\\n'):
    try:r.source_files(pathlib.Path(sys.argv[2]))
    except ValueError:pass
    else:raise SystemExit('Optimized execution accepted external source symlink')
"""
    optimized = subprocess.run([sys.executable, "-O", "-B", "-c", optimized_check,
                                str(REPO / "native-tests/desktop"), str(fake_repo)], capture_output=True, text=True, timeout=2)
    assert optimized.returncode == 0, optimized.stderr
    proof["controls"].append({"name": "optimized Python preserves proof and source authority gates", "passed": True})
    cleanup_case = data / "cleanup-injection"
    cleanup_case.mkdir()
    class StuckApp:
        def poll(self): return None
        def terminate(self): pass
        def kill(self): pass
        def wait(self, timeout): raise subprocess.TimeoutExpired("owned App cleanup injection", timeout)
    with patch.object(runner, "enable_child_subreaper"), \
         patch.object(runner.subprocess, "Popen", return_value=StuckApp()), \
         patch.object(runner.subprocess, "run", side_effect=RuntimeError("original observer failure")):
        try:
            runner.worker(SimpleNamespace(data=cleanup_case, binary=Path("owned-test-App")))
        except subprocess.TimeoutExpired:
            pass
        else:
            raise AssertionError("Stuck cleanup did not fail")
    recorded = json.loads((cleanup_case / "app-execution.json").read_text())
    assert recorded["passed"] is False and recorded["error"] == "original observer failure" and recorded["cleanupError"]
    proof["controls"].append({"name": "uninterruptible App cleanup preserves original failure receipt", "passed": True})
    class StuckGuard(StuckApp):
        pid = os.getpid()  # Only an audit value; every process operation is mocked.
        def send_signal(self, _signal): pass
        def wait(self, timeout): raise subprocess.TimeoutExpired("owned guard cleanup injection", timeout)
    app_binary, browser_binary = fake_repo / "owned-app", fake_repo / "bin/owned-browser"
    browser_binary.parent.mkdir()
    for binary in (app_binary, browser_binary, browser_binary.parent / "libcef.so"):
        binary.write_text("synthetic executable identity, never run\n")
    outer_case = data / "outer-cleanup-injection"
    with patch.object(runner, "REPO", fake_repo), \
         patch.object(runner, "source_files", return_value=[fake_repo / "src/good.ts"]), \
         patch.object(runner.subprocess, "check_output", return_value="fixed-source-head\n"), \
         patch.object(runner.subprocess, "Popen", return_value=StuckGuard()), \
         patch.object(sys, "argv", ["browser_process.py", "--binary", str(app_binary),
                                   "--browser", str(browser_binary), "--data", str(outer_case)]):
        try:
            runner.main()
        except subprocess.TimeoutExpired:
            pass
        else:
            raise AssertionError("Stuck outer guard did not fail")
    recorded = json.loads((outer_case / "execution.json").read_text())
    assert recorded["passed"] is False and recorded["error"] and recorded["cleanupError"]
    assert recorded["artifactsBefore"] == recorded["artifactsAfter"]
    proof["controls"].append({"name": "uninterruptible guard cleanup preserves outer failure receipt", "passed": True})
    class BoundedSource:
        remaining = 3 * 1024 * 1024 + 17
        def open(self, mode):
            assert mode == "rb"
            return self
        def __enter__(self):
            return self
        def __exit__(self, *_args):
            pass
        def read(self, size):
            assert 0 < size <= 1024 * 1024, "Artifact hashing must never request the complete binary"
            count = min(self.remaining, size)
            self.remaining -= count
            return b"x" * count
    assert runner.digest(BoundedSource()) == hashlib.sha256(b"x" * (3 * 1024 * 1024 + 17)).hexdigest()
    proof["controls"].append({"name": "bounded artifact hashing", "passed": True})
    with (data / "guard.log").open("x") as output:
        child = subprocess.Popen(["python3", "-B", str(REPO / "native-tests/browser/guard.py"),
                                  str(receipt), sys.executable, "-B", "-c", "import time; time.sleep(3)"],
                                 start_new_session=True, stdout=output, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 1
            native = None
            while time.monotonic() < deadline:
                children = list(guard.descendants(child.pid))
                if children:
                    native = children[0]
                    break
                time.sleep(0.01)
            assert native, "Owned guard did not create its direct test child"
            profile = "11111111-1111-4111-8111-111111111111"
            binding = {"processes": [{"profile": profile, "pid": child.pid, "opened": {"nativePid": native}}]}
            file = data / "app-browser-process-open.json"
            file.write_text(json.dumps(binding))
            command = ["python3", "-B", str(REPO / "native-tests/desktop/browser_snapshot.py"),
                       "--data", str(data), "--profile", profile,
                       "--native-start", guard.ancestry_identity(native)[0],
                       "--guard-start", guard.ancestry_identity(child.pid)[0]]
            env = dict(os.environ, TAKOSUMI_BROWSER_PROCESS_PROGRAM=str(Path(sys.executable).resolve()))

            def run(name, args, expected):
                args = list(args)
                if not expected:
                    # A fresh destination makes disabled ownership checks go
                    # red; an existing receipt must not accidentally mask them.
                    denied_case = data / f"negative-{len(proof['controls'])}"
                    denied_case.mkdir()
                    (denied_case / ".owned-browser-reference").write_text("negative generation control")
                    (denied_case / "app-browser-process-open.json").write_text(file.read_text())
                    args[args.index("--data") + 1] = str(denied_case)
                result = subprocess.run(args, env=env, capture_output=True, text=True, timeout=2)
                assert (result.returncode == 0) == expected, result.stderr
                if not expected:
                    assert not list(denied_case.glob("failure-process-*.json")), "Denied ownership must not write a snapshot"
                proof["controls"].append({"name": name, "exit": result.returncode, "expectedSuccess": expected})

            run("exact live generation", command, True)
            snapshot = data / f"failure-process-{profile}.json"
            previous = snapshot.read_bytes()
            stale_guard = list(command)
            stale_guard[-1] = "0"
            run("stale guard generation", stale_guard, False)
            run("optimized stale guard generation", [stale_guard[0], "-O", *stale_guard[1:]], False)
            stale_native = list(command)
            stale_native[-3] = "0"
            run("stale native generation", stale_native, False)
            run("optimized stale native generation", [stale_native[0], "-O", *stale_native[1:]], False)
            # This test's own process is deliberately outside the guard tree.
            binding["processes"][0]["opened"]["nativePid"] = os.getpid()
            file.write_text(json.dumps(binding))
            foreign = list(command)
            foreign[-3] = guard.ancestry_identity(os.getpid())[0]
            run("foreign live process", foreign, False)
            run("optimized foreign live process", [foreign[0], "-O", *foreign[1:]], False)
            assert snapshot.read_bytes() == previous, "Denied read must not replace the valid snapshot"
            assert child.wait(timeout=5) == 0
            proof["guard"] = json.loads(receipt.read_text())
            assert proof["guard"]["forcedCleanup"] == 0 and proof["guard"]["kernelNoChildren"]
            proof["passed"] = True
        finally:
            if child.poll() is None:
                # Ask only this exact owned guard to drain its children.
                child.send_signal(signal.SIGTERM)
                child.wait(timeout=5)
            (data / "controls.json").write_text(json.dumps(proof, indent=2)+"\n")
    print(json.dumps({"data": str(data), **proof}))


if __name__ == "__main__":
    main()
