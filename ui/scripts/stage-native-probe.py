#!/usr/bin/env python3
"""Independent native CSS probe; copies authored assets only. No npm execution.
This is NOT a substitute for the gated React/Vite production build.
"""
from pathlib import Path
import shutil
root = Path(__file__).resolve().parents[2]
out = root / 'apps/hoh-desktop/probe-dist'
out.mkdir(exist_ok=True)
html = (root / 'ui/probe.html').read_text().replace('/src/probe.css', './probe.css').replace('/src/probe.js', './probe.js')
(out/'probe.html').write_text(html)
(out/'index.html').write_text('<!doctype html><html lang="ru"><meta charset="UTF-8"><title>HoH CSS probe</title><p>Проверка начинается…</p></html>')
for name in ['probe.css', 'probe.js']:
    shutil.copyfile(root / 'ui/src' / name, out/name)
print(out)
