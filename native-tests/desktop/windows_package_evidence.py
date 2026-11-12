"""Copy only known, bounded text diagnostics from standalone W6 browser cases."""
import argparse
import json
import os
from pathlib import Path
import re
import stat

DIAGNOSTICS = ("boot.json", "browser.log", "cef-native.log", "summary.json")
CASE = re.compile(r"sumi-cef-smoke-[a-z0-9_-]{1,64}")
LIMIT = 8 * 1024 * 1024


def plain(info, directory=False):
    return not (getattr(info, "st_file_attributes", 0) & 0x400) and (
        stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode))


def identity(info):
    return info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns


def text_bytes(path):
    """Validate the opened file and keep exactly those inspected bytes."""
    try:
        before = path.lstat()
        if not plain(before) or before.st_nlink != 1 or before.st_size > LIMIT:
            return None
        with path.open("rb") as source:
            opened = os.fstat(source.fileno())
            if identity(opened) != identity(before):
                return None
            data = source.read(LIMIT + 1)
            after = os.fstat(source.fileno())
        if (len(data) > LIMIT or identity(after) != identity(before)
                or identity(path.lstat()) != identity(before)):
            return None
        if b"\0" in data or data.startswith((b"MZ", b"\x7fELF", b"PK\x03\x04")):
            return None
        data.decode("utf-8", errors="strict")
        return data
    except (OSError, UnicodeDecodeError):
        return None


def collect(source, destination):
    # Never descend into a browser profile, even when a DB file has a text
    # suffix/name. Cases and diagnostic leaves must themselves be plain.
    if not plain(source.lstat(), directory=True):
        raise ValueError("Plain browser evidence root required")
    destination.mkdir(parents=True, mode=0o700, exist_ok=False)
    kept = []
    for case in sorted(source.iterdir()):
        if not CASE.fullmatch(case.name) or not plain(case.lstat(), directory=True):
            continue
        for name in DIAGNOSTICS:
            data = text_bytes(case / name)
            if data is None:
                continue
            target = destination / case.name / name
            target.parent.mkdir(mode=0o700, exist_ok=True)
            with target.open("xb") as output:
                output.write(data)
            target.chmod(0o600)
            kept.append(str(target.relative_to(destination)))
    return kept


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps({"kept": collect(args.source, args.destination)}))
