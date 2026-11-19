#!/usr/bin/env python3
"""Stage the production approval page beside a tiny native-probe main page."""
from pathlib import Path
import shutil

desktop = Path(__file__).resolve().parents[1]
root = desktop.parents[1]
target = desktop / "probe-dist"
shutil.copytree(root / "ui" / "dist", target, dirs_exist_ok=True)
for name in ("index.html", "approval-probe.js"):
    shutil.copy2(desktop / "probe-approval" / name, target / name)
