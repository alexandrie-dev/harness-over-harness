#!/usr/bin/env python3
"""Build both HoH executables against one already-built ui/dist."""

import argparse
import json
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


def stage_linux(output, desktop, engine, release_version=None):
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        staged = Path(temporary) / output.name
        staged.mkdir()
        bundle = staged
        if release_version is not None:
            bundle = staged / "app"
            bundle.mkdir()
            (bundle / ".hoh-release").write_text(release_version + "\n", encoding="utf-8")
            for program in ("hoh-desktop", "hoh"):
                (staged / program).symlink_to(Path("app") / program)
        shutil.copy2(desktop, bundle / "hoh-desktop")
        shutil.copy2(engine, bundle / "hoh")
        for name in ("NOTICE", "LICENSE"):
            source = ROOT / name
            if source.is_file():
                shutil.copy2(source, bundle / name)
        staged.rename(output)


def package_linux(output, bundle_name, desktop, engine):
    config = json.loads((ROOT / "apps/hoh-desktop/tauri.conf.json").read_text(encoding="utf-8"))
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        staging = Path(temporary)
        stage_linux(staging / bundle_name, desktop, engine, config["version"])
        archive = staging / output.name
        subprocess.run(
            ["tar", "--zstd", "-cf", str(archive), "-C", temporary, bundle_name], check=True,
        )
        output.hardlink_to(archive)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, required=True)
    parser.add_argument("--target", help="Rust target triple, defaulting to the host")
    parser.add_argument("--rcodesign", help="rcodesign binary for optional cross-packaging")
    parser.add_argument("--release", action="store_true", help="produce a GitHub Release archive")
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
    bundle_name = f"HoH-linux-{target.split('-', 1)[0]}"
    if macos:
        artifact = root / "HoH-macos-arm64.zip"
    else:
        artifact = root / (f"{bundle_name}.tar.zst" if args.release else f"HoH-{target}")
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
    elif args.release:
        package_linux(artifact, bundle_name, desktop, engine)
    else:
        stage_linux(artifact, desktop, engine)
    print(artifact)


if __name__ == "__main__":
    main()
