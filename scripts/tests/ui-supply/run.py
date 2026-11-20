#!/usr/bin/env python3
"""Run the real gate against isolated, deliberately poisoned input trees. No network."""
import copy
import datetime as dt
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import yaml

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
now = dt.datetime.now(dt.timezone.utc)
base = json.loads((HERE / 'base.json').read_text())
for case_file in sorted(HERE.glob('case-*.json')):
    case = json.loads(case_file.read_text())
    with tempfile.TemporaryDirectory(prefix='hoh-supply-') as tmp:
        root = Path(tmp)
        ui = root / 'ui'
        cache = ui / '.supply-cache'
        cache.mkdir(parents=True)
        (root / 'docs/ts').mkdir(parents=True)
        for file in ['.npmrc', 'pnpm-workspace.yaml']:
            shutil.copyfile(ROOT / 'ui' / file, ui / file)
        if case.get('configChanges'):
            cfg = yaml.safe_load((ui/'pnpm-workspace.yaml').read_text())
            cfg.update(case['configChanges'])
            (ui/'pnpm-workspace.yaml').write_text(yaml.safe_dump(cfg))
        data = copy.deepcopy(base)
        for key, value in case.get('replace', {}).items():
            target = data
            parts = key.split('/')
            for part in parts[:-1]:
                target = target[part]
            target[parts[-1]] = value
        if case.get('young'):
            data['metadata']['time']['1.0.0'] = now.isoformat()
        (ui / 'package.json').write_text(json.dumps(data['manifest']))
        (ui / 'pnpm-lock.yaml').write_text(yaml.safe_dump(data['lock']))
        reason = '' if case.get('missingWhy') else 'Planted fixture for supply gate checks'
        (root / 'docs/ts/DEPS.md').write_text('```json\n'+json.dumps(data['policy'])+'\n```\n' + f'| `fixture` | 1.0.0 | 2020-01-01 | MIT | {reason} |\n')
        (cache / 'fixture.json').write_text(json.dumps(data['metadata']))
        audit = {'lockSha256': hashlib.sha256((ui/'pnpm-lock.yaml').read_bytes()).hexdigest(),
                 'checkedAt': now.isoformat(), 'report': data['audit']}
        (cache / 'audit.json').write_text(json.dumps(audit))
        if case.get('installed'):
            installed = ui/'node_modules/.pnpm/fixture@1.0.0/node_modules/fixture'
            installed.mkdir(parents=True)
            (installed/'package.json').write_text(json.dumps(case['installed']))
        result = subprocess.run([str(ROOT/'scripts/ui-supply-check.sh'), '--root', tmp], capture_output=True, text=True)
        expected = case.get('error')
        if expected:
            assert result.returncode != 0 and expected in result.stderr, (case_file, result.stdout, result.stderr)
        else:
            assert result.returncode == 0, (case_file, result.stdout, result.stderr)
            assert 'skipped: cargo-deny not installed' in result.stdout
        print(f'PASS {case_file.stem}: {expected or "clean control + cargo-deny skip"}')

# All extensions run on planted input or read-only copies, with no real build.
import runpy
for extension in ('environment.py', 'exceptions.py', 'artifacts.py'):
    runpy.run_path(str(HERE / extension), run_name='__main__')
