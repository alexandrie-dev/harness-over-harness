#!/usr/bin/env python3
"""Check Ctrl+V when WebKitGTK receives HTML alongside a system clipboard PNG."""

import argparse
import base64
import os
from pathlib import Path
import select
import shlex
import shutil
import subprocess
import sys
import tempfile

from run import Harness, terminate_owned, wait_until


PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg=="
)


def run_window(args):
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    qa = args.qa_dir
    args.display = os.environ["DISPLAY"]
    harness = Harness(args, Atspi)
    image = qa / "sample.png"
    image.write_bytes(PNG)

    clipboard = None
    with (qa / "logs/window.log").open("w", encoding="utf-8") as log:
        app = subprocess.Popen([str(args.desktop)], stdout=log, stderr=log)
        try:
            wait_until(harness._discover_app, 30, "isolated HoH window")
            for label in ["Start", "Start without a project"]:
                wait_until(lambda: harness.find(role="push button", name=label), 20, label)
                harness.press(label, role="push button", name=label)
            editor = wait_until(
                lambda: harness.find(role="entry", pattern=r"^Message"), 20, "chat composer"
            )
            x, y, width, height = harness.extent(editor)
            subprocess.run(
                ["xdotool", "windowfocus", "--sync", harness.window_id(), "mousemove",
                 str(x + min(width // 2, 20)), str(y + min(height // 2, 15)), "click", "1"],
                check=True,
            )

            clipboard = subprocess.Popen(
                [str(qa / "clipboard-owner"), str(image)], stdout=subprocess.PIPE, text=True
            )
            assert clipboard.stdout is not None
            readable, _, _ = select.select([clipboard.stdout], [], [], 5)
            if not readable or clipboard.stdout.readline().strip() != "clipboard-ready":
                raise RuntimeError("GTK did not take ownership of the mixed clipboard")
            attachment = {"role": "push button", "name": "Remove clipboard.png"}
            assert not harness.find_all(**attachment), "the new draft already has a clipboard image"
            harness.xkey("ctrl+v")
            wait_until(
                lambda: len(harness.find_all(**attachment)) == 1,
                10,
                "one PNG attachment from the mixed HTML and PNG clipboard",
            )
            print("PASS: native Ctrl+V attached the PNG from the mixed clipboard")
        finally:
            try:
                if harness.app is not None:
                    harness.dump("clipboard")
            finally:
                terminate_owned(clipboard)
                terminate_owned(app)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["desktop", "engine", "codex-stub"]:
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--qa-dir", type=Path)
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.inside:
        run_window(args)
        return 0

    for name in ["desktop", "engine", "codex_stub"]:
        path = getattr(args, name).resolve()
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{path} must be an executable file")
        setattr(args, name, path)
    if args.qa_dir is None:
        qa = Path(tempfile.mkdtemp(prefix="hoh-clipboard-"))
    else:
        qa = args.qa_dir.resolve()
        qa.mkdir(mode=0o700)
    print(f"QA logs: {qa}", flush=True)
    for name in ["bin", "home/Documents", "logs", "runtime", "services", "shots"]:
        (qa / name).mkdir(mode=0o700, parents=True)
    (qa / "bin/codex").symlink_to(args.codex_stub)
    shutil.copy2("/usr/share/dbus-1/services/org.a11y.Bus.service", qa / "services")
    bus = qa / "bus.conf"
    bus.write_text(
        '<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" '
        '"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">\n'
        '<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>'
        '<auth>EXTERNAL</auth><servicedir>services</servicedir>'
        '<policy context="default"><allow send_destination="*" eavesdrop="true"/>'
        '<allow eavesdrop="true"/><allow own="*"/></policy></busconfig>\n',
        encoding="utf-8",
    )
    gtk = subprocess.check_output(["pkg-config", "--cflags", "--libs", "gtk+-3.0"], text=True)
    source = Path(__file__).resolve()
    subprocess.run(
        ["cc", str(source.with_name("clipboard-owner.c")), "-o", str(qa / "clipboard-owner"),
         *shlex.split(gtk)],
        check=True,
    )
    lock = Path.home() / ".cache/hoh-build/xvfb.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    env = {
        "HOME": str(qa / "home"),
        "PATH": f"{qa / 'bin'}:{os.defpath}",
        "HOH_ENGINE_PATH": str(args.engine),
        **{f"XDG_{key.upper()}_HOME": str(qa / key)
           for key in ["cache", "config", "data", "state"]},
        "XDG_RUNTIME_DIR": str(qa / "runtime"),
        "LANG": "C.UTF-8",
        "GDK_BACKEND": "x11",
        "GTK_MODULES": "gail:atk-bridge",
        "GTK_USE_PORTAL": "0",
        "GIO_USE_VFS": "local",
        "NO_AT_BRIDGE": "0",
        "WEBKIT_DISABLE_COMPOSITING_MODE": "1",
        "LIBGL_ALWAYS_SOFTWARE": "1",
        "PULSE_SERVER": "unix:/nonexistent",
    }
    return subprocess.run(
        ["flock", "-w", "20", str(lock), "timeout", "-k", "5", "85",
         "dbus-run-session", f"--config-file={bus}", "--", "xvfb-run", "-a", "-s",
         "-screen 0 1280x860x24 -nolisten tcp", sys.executable, "-B", str(source), "--inside",
         "--qa-dir", str(qa), "--desktop", str(args.desktop), "--engine", str(args.engine),
         "--codex-stub", str(args.codex_stub)],
        cwd=qa, env=env,
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
