# Live E2E for the T3 Tauri window

This suite drives an immutable HoH release with a private Xvfb display and
AT-SPI bus. It launches through the same `scripts/demo-window.sh --claude` path
used for a human demo, preserves a semantic tree dump for every checkpoint, and
writes screenshots plus `RESULTS.json` under the supplied QA directory.

```sh
python3 ui/tests/e2e-t3/run.py \
  --qa-dir /mnt/toshiba/alexandrie/hoh/qa/cx-qe-YYYYMMDD-HHMMSS \
  --display :84 \
  --commit b0f350af
```

Requirements are system tools already used by this repository: Python GI with
AT-SPI, `dbus-run-session`, `Xvfb`, `xdotool`, and ImageMagick `import`. The run
must start with a new QA directory; it refuses to reuse an existing `data/`.
It sends five short live-Claude prompts, below the assignment limit of ten.
Screenshots come from the Xvfb framebuffer cropped to the semantic `HoH` frame,
which avoids blank WebKitGTK backing-surface captures.

The command exits nonzero if any product scenario fails, but it continues after
independent scenario failures where the remaining UI is still reachable. A
human must inspect every generated PNG and write the final QA report; the runner
does not infer visual overlap from pixels.
