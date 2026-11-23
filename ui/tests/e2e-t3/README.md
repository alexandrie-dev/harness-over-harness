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
