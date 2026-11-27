# Live E2E for the T3 Tauri window

This suite drives explicitly selected current-head HoH binaries with a private Xvfb display and
AT-SPI bus. It launches through the same `scripts/demo-window.sh --claude` path
used for a human demo, preserves a semantic tree dump for every checkpoint, and
writes screenshots plus `RESULTS.json` under the supplied QA directory.

```sh
python3 ui/tests/e2e-t3/run.py \
  --qa-dir /mnt/toshiba/alexandrie/hoh/qa/cx-qe-YYYYMMDD-HHMMSS \
  --display :100 \
  --commit "$(git rev-parse --short HEAD)" \
  --desktop /home/save/.cache/hoh-build/line-claude/release/hoh-desktop \
  --engine /home/save/.cache/hoh-build/line-claude/release/hoh \
  --mock /home/save/.cache/hoh-build/line-claude/release/hoh-llm-openai-mock
```

The three paths are required, resolved to existing executable files before Xvfb
starts, and recorded in `RESULTS.json`. The runner creates private symlinks under
the fresh QA directory so `demo-window.sh` sets `HOH_ENGINE_PATH` to the supplied
engine without reading or writing the shared release directory. The display is
also required and must be one of this line's assigned `:100` through `:109`.

Requirements are system tools already used by this repository: Python GI with
AT-SPI, `dbus-run-session`, `Xvfb`, `xdotool`, and ImageMagick `import`. The run
must start with a new QA directory; it refuses to reuse an existing `data/`.
It sends five short live-Claude prompts, below the assignment limit of ten.
Screenshots come from the Xvfb framebuffer cropped to the semantic `HoH` frame,
which avoids blank WebKitGTK backing-surface captures.
The window starts at 1440×900; scenario 8 resizes the same live window to
800×550 before its compact-layout screenshots.

The command exits nonzero if any product scenario fails, but it continues after
independent scenario failures where the remaining UI is still reachable. A
human must inspect every generated PNG and write the final QA report; the runner
does not infer visual overlap from pixels.

## Native clipboard regression

Run this test with the Linux desktop and engine binaries under test. It puts
`text/html` and `image/png` on the same GTK clipboard, sends Ctrl+V to a fresh
chat and asserts that the composer shows one PNG attachment. It uses an isolated
Xvfb session and a local Codex stub. No prompt is sent.

In addition to the tools above, install a C compiler, `pkg-config` and GTK 3
development headers. Build the stub with
`cargo build --locked --offline -p hoh-harness-codex --features stub --bin codex-stub`.

```sh
/usr/bin/python3 -B ui/tests/e2e-t3/clipboard.py \
  --desktop /path/to/hoh-desktop \
  --engine /path/to/hoh \
  --codex-stub /path/to/codex-stub
```

Pass `--qa-dir` with a new directory to keep the window log and accessibility
tree at a specific path. The test prints the path it used.
