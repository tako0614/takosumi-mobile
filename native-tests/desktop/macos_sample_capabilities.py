#!/usr/bin/env python3
"""Read installed sample usage/manual only; never pass a process to sample."""
import argparse
import gzip
import json
from pathlib import Path
import platform
import subprocess
import tempfile

LIMIT = 16384


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("This diagnostic reads the installed macOS tool")
    result = {"diagnosticOnly": True, "tool": "/usr/bin/sample", "processArgument": None,
              "macOS": platform.mac_ver()[0], "manual": None}
    # Missing process argument requests usage, not a sample. Keep the tool's
    # actual nonzero usage exit; it is not an App/native test result.
    with tempfile.TemporaryFile() as output:
        try:
            done = subprocess.run(["/usr/bin/sample"], stdin=subprocess.DEVNULL,
                                  stdout=output, stderr=subprocess.STDOUT, timeout=3, check=False)
            result["exit"] = done.returncode
        except (OSError, subprocess.TimeoutExpired) as error:
            result["errorType"] = type(error).__name__
        output.seek(0)
        usage = output.read(LIMIT + 1)
        result["usage"] = usage[:LIMIT].decode("utf-8", errors="replace")
        result["usageTruncated"] = len(usage) > LIMIT
    for path in (Path("/usr/share/man/man1/sample.1"), Path("/usr/share/man/man1/sample.1.gz")):
        if not path.is_file():
            continue
        try:
            with (gzip.open(path, "rb") if path.suffix == ".gz" else path.open("rb")) as source:
                data = source.read(LIMIT + 1)
            result["manual"] = {"path": str(path), "text": data[:LIMIT].decode("utf-8", errors="replace"),
                                "truncated": len(data) > LIMIT}
        except OSError as error:
            result["manualErrorType"] = type(error).__name__
        break
    with args.out.open("x") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    print("Recorded installed sample usage/manual; no process sampled, no shutdown verdict changed")


if __name__ == "__main__":
    main()
