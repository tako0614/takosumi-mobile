#!/usr/bin/env python3
"""Read only the exact live browser guard generation in an owned App case."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import time
import uuid

from web_home import descendants
from browser_process import owned_case_path, require

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("owned_browser_guard", REPO / "native-tests/browser/guard.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


def describe(pid):
    root = Path("/proc") / str(pid)
    fields = (root / "stat").read_text().rsplit(") ", 1)[1].split()
    row = {"pid": pid, "start": fields[19], "state": fields[0], "parent": int(fields[1]),
           "cpuTicks": [int(fields[11]), int(fields[12])], "rssPages": int(fields[21]),
           "name": (root / "comm").read_text().strip(), "wait": (root / "wchan").read_text().strip()}
    row["threads"] = []
    for task in sorted((root / "task").iterdir())[:16]:
        try:
            row["threads"].append({"tid": int(task.name), "name": (task / "comm").read_text().strip(),
                                   "wait": (task / "wchan").read_text().strip()})
        except (FileNotFoundError, ProcessLookupError):
            pass
    return row


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--native-start", required=True)
    parser.add_argument("--guard-start", required=True)
    args = parser.parse_args()
    require(str(uuid.UUID(args.profile)) == args.profile, "Canonical profile UUID required")
    data = owned_case_path(args.data)
    opened = json.loads(data.joinpath("app-browser-process-open.json").read_text())
    matches = [p for p in opened["processes"] if p["profile"] == args.profile]
    require(len(matches) == 1, "One exact profile binding is required")
    process = matches[0]
    native = process["opened"]["nativePid"]
    parent = process["pid"]
    # The runner names the executable separately for --packaged runs, where
    # the App resolves its bundled browser and no override is set.
    expected = Path(os.environ.get("TAKOSUMI_BROWSER_PROCESS_SNAPSHOT_PROGRAM")
                    or os.environ["TAKOSUMI_BROWSER_PROCESS_PROGRAM"]).resolve(strict=True)

    def generation_live():
        return (guard.ancestry_identity(parent) or (None,))[0] == args.guard_start

    require(generation_live(), "The owned guard generation must still exist")
    actual = guard.ancestry_identity(native)
    require(actual == (args.native_start, parent), f"Native generation changed: expected {(args.native_start, parent)!r}, observed {actual!r}")
    require(Path(os.readlink(f"/proc/{native}/exe")) == expected, "The native executable changed")
    require(guard.is_owned(native, parent, args.guard_start), "The native process is not owned by this guard")
    result = {"profile": args.profile, "nativePid": native, "guardPid": parent,
              "method": "exact live guard generation and stable ancestry; no signals", "samples": []}
    # Two bounded kernel samples distinguish CPU progress from a stable wait.
    # No renderer evaluation, storage values, argv, environment or global scan.
    for _ in range(2):
        require(generation_live(), "The guard generation ended during sampling")
        sample = {"monotonic": time.monotonic(), "processes": []}
        for pid in sorted({parent} | descendants(parent))[:64]:
            if not guard.is_owned(pid, parent, args.guard_start):
                continue
            try:
                row = describe(pid)
                identity = guard.ancestry_identity(pid)
                if identity and identity[0] == row["start"] and guard.is_owned(pid, parent, args.guard_start):
                    sample["processes"].append(row)
            except (FileNotFoundError, ProcessLookupError):
                pass
        result["samples"].append(sample)
        time.sleep(0.1)
    require(generation_live(), "The guard generation ended before receipt")
    file = data / f"failure-process-{args.profile}.json"
    with file.open("x") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    print(json.dumps({"snapshot": str(file), "processCount": len(result["samples"][-1]["processes"])}))


if __name__ == "__main__":
    main()
