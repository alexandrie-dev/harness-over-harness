#!/usr/bin/env python3
"""Package the built arm64 desktop and its companion engine into HoH.app."""

import argparse
import json
import plistlib
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path


def deployment_target(binary):
    with binary.open("rb") as stream:
        magic, cpu, _, kind, commands, *_ = struct.unpack("<8I", stream.read(32))
        if (magic, cpu, kind) != (0xFEEDFACF, 0x0100000C, 2):
            raise ValueError(f"{binary} is not a macOS arm64 executable")
        for _ in range(commands):
            command, length = struct.unpack("<2I", stream.read(8))
            data = stream.read(length - 8)
            if command == 0x32:  # LC_BUILD_VERSION
                platform, version = struct.unpack_from("<2I", data)
                if platform != 1:
                    raise ValueError(f"{binary} is not built for macOS")
                return version >> 16, (version >> 8) & 255, version & 255
    raise ValueError(f"No macOS deployment target recorded in {binary}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("desktop", type=Path)
    parser.add_argument("engine", type=Path)
    parser.add_argument("archive", type=Path, help="output .zip")
    parser.add_argument("--rcodesign", help="path to rcodesign for signing on Linux")
    args = parser.parse_args()
    if args.archive.suffix != ".zip" or args.archive.exists():
        parser.error("archive must be a new .zip file")
    signer = args.rcodesign or shutil.which("rcodesign")
    if sys.platform != "darwin" and not signer:
        parser.error("install rcodesign or pass --rcodesign to sign the app bundle")
    deployment = max(deployment_target(args.desktop), deployment_target(args.engine))
    repo = Path(__file__).resolve().parent.parent
    desktop = repo / "apps/hoh-desktop"
    config = json.loads((desktop / "tauri.conf.json").read_text())
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        contents = root / "HoH.app/Contents"
        (contents / "MacOS").mkdir(parents=True)
        for input_file, name in [(args.desktop, "hoh-desktop"), (args.engine, "hoh")]:
            installed = contents / "MacOS" / name
            shutil.copyfile(input_file, installed)
            installed.chmod(0o755)
        resources = contents / "Resources"
        resources.mkdir()
        icons = []
        for code, dimension in [(b"ic08", 256), (b"ic09", 512)]:
            png = (desktop / f"icons/{dimension}x{dimension}.png").read_bytes()
            icons.append(struct.pack(">4sI", code, len(png) + 8) + png)
        icon = b"".join(icons)
        (resources / "HoH.icns").write_bytes(struct.pack(">4sI", b"icns", len(icon) + 8) + icon)
        tracked = subprocess.check_output(
            ["git", "ls-files", "-z", "--", "NOTICE", "LICENSE", "ui", "crates", "plugins"],
            cwd=repo,
        )
        for entry in tracked.decode().split("\0"):
            source = Path(entry)
            if not source.name.startswith(("LICENSE", "NOTICE", "COPYING")):
                continue
            notice = resources / "Licenses" / source
            notice.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(repo / source, notice)
        with (contents / "Info.plist").open("wb") as plist:
            plistlib.dump({
                "CFBundleExecutable": "hoh-desktop",
                "CFBundleIconFile": "HoH.icns",
                "CFBundleIdentifier": config["identifier"],
                "CFBundleName": config["productName"],
                "CFBundlePackageType": "APPL",
                "CFBundleShortVersionString": config["version"],
                "CFBundleVersion": config["version"],
                "LSMinimumSystemVersion": ".".join(map(str, deployment)),
                "NSHighResolutionCapable": True,
                "NSMicrophoneUsageDescription": "HoH uses the microphone when you start dictation.",
            }, plist)
        app = contents.parent
        if signer:
            subprocess.run([signer, "sign", "--config-file", "/dev/null", str(app)], check=True)
        else:
            for target in [contents / "MacOS" / "hoh", app]:
                subprocess.run(["codesign", "--force", "--sign", "-", str(target)], check=True)
            subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        args.archive.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["zip", "-q", "-r", str(args.archive.resolve()), "HoH.app"], cwd=root, check=True)


if __name__ == "__main__":
    main()
