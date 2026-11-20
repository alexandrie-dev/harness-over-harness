#!/usr/bin/env python3
"""Prove pnpm 12's native age gate using already cached React metadata, no network.
Run after the first lock resolve. No archives, scripts or package execution.
"""
import json
from pathlib import Path
import re
import subprocess
import tempfile
import yaml

root = Path(__file__).resolve().parents[3]
with tempfile.TemporaryDirectory(prefix='hoh-native-age-') as tmp:
    fixture = Path(tmp)
    cfg = yaml.safe_load((root/'ui/pnpm-workspace.yaml').read_text())
    cfg.update(yaml.safe_load((Path(__file__).parent/'native-age.yaml').read_text()))
    (fixture/'pnpm-workspace.yaml').write_text(yaml.safe_dump(cfg))
    (fixture/'package.json').write_text(json.dumps({'private': True, 'dependencies': {'react': '19.3.1'}}))
    runtime = fixture/'runtime'
    runtime.mkdir(mode=0o700)
    import os
    result = subprocess.run(['pnpm', '-C', tmp, 'install', '--lockfile-only', '--no-frozen-lockfile',
                             '--offline', '--ignore-scripts', '--no-optional'], capture_output=True, text=True,
                            env=os.environ | {'XDG_RUNTIME_DIR': str(runtime)})
    output = result.stdout + result.stderr
    assert result.returncode != 0 and re.search(r'minimum.?release.?age|release.age', output, re.I), output
    print(output.strip())
    print('PASS: native pnpm release-age gate refused a planted age violation, offline')
