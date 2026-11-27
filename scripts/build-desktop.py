#!/usr/bin/env python3
"""Build both HoH executables against one already-built ui/dist."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parent.parent


def build(target, output, args, env):
    subprocess.run(
        [
            "cargo", "build", "--locked", "--offline", "--release",
            "--target", target, "--target-dir", str(output), *args,
        ],
        cwd=ROOT,
        env=env,
        check=True,
    )
    return output / target / "release"


def stage_linux(output, desktop, engine):
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        staged = Path(temporary) / output.name
        staged.mkdir()
        shutil.copy2(desktop, staged / "hoh-desktop")
        shutil.copy2(engine, staged / "hoh")
        for name in ("NOTICE", "LICENSE"):
            source = ROOT / name
            if source.is_file():
                shutil.copy2(source, staged / name)
        staged.rename(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, required=True)
    parser.add_argument("--target", help="Rust target triple, defaulting to the host")
    parser.add_argument("--rcodesign", help="rcodesign binary for optional cross-packaging")
    args = parser.parse_args()
    if not (ROOT / "ui/dist/index.html").is_file():
        parser.error("build ui/dist first or copy it from the same source revision")
    target = args.target or subprocess.check_output(
        ["rustc", "--print", "host-tuple"], cwd=ROOT, text=True
    ).strip()
    if target not in (
        "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin",
    ):
        parser.error("supported targets are Linux x64/arm64 and macOS arm64")
    macos = target == "aarch64-apple-darwin"
    root = args.build_dir.resolve()
    root.mkdir(parents=True, exist_ok=True)
    artifact = root / ("HoH-macos-arm64.zip" if macos else f"HoH-{target}")
    if artifact.exists():
        parser.error(f"output already exists: {artifact}")

    env = dict(os.environ)
    env.pop("HOH_ENGINE_PATH", None)
    env.setdefault("CARGO_BUILD_JOBS", "2")
    engine = build(target, root / "engine", ["-p", "hoh"], env) / "hoh"
    desktop = build(
        target, root / "desktop", ["--manifest-path", "apps/hoh-desktop/Cargo.toml"], env
    ) / "hoh-desktop"
    if macos:
        signer = ["--rcodesign", args.rcodesign] if args.rcodesign else []
        subprocess.run(
            [sys.executable, str(ROOT / "scripts/package-macos.py"),
             str(desktop), str(engine), str(artifact), *signer],
            check=True,
        )
    else:
        stage_linux(artifact, desktop, engine)
    print(artifact)


if __name__ == "__main__":
    main()
