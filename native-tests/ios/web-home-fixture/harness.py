"""Reuse the established Simulator/CLI custody, without importing this runner as `run`."""
import importlib.util
from pathlib import Path
import sys

DOWNLOAD = Path(__file__).resolve().parent.parent / "download-fixture"
site_spec = importlib.util.spec_from_file_location("sumi_ios_download_site", DOWNLOAD / "server.py")
site = importlib.util.module_from_spec(site_spec)
site_spec.loader.exec_module(site)
spec = importlib.util.spec_from_file_location("sumi_ios_download_runtime", DOWNLOAD / "run.py")
runtime = importlib.util.module_from_spec(spec)
# Its historical absolute import must bind its own server even if another
# `server` was imported by a caller. Keep that binding private to module load.
missing = object()
previous = sys.modules.get("server", missing)
sys.modules["server"] = site
try:
    spec.loader.exec_module(runtime)
finally:
    if previous is missing:
        del sys.modules["server"]
    else:
        sys.modules["server"] = previous
