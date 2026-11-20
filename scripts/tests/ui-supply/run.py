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
        # The real workspace carries T3's patches; fixtures declare their own (case 'patched') or none.
        cfg = yaml.safe_load((ui/'pnpm-workspace.yaml').read_text())
        cfg.pop('patchedDependencies', None)
        cfg.update(case.get('configChanges', {}))
        patched = case.get('patched', {})
        data = copy.deepcopy(base)
        for relative, content in patched.get('files', {}).items():
            (ui / relative).parent.mkdir(parents=True, exist_ok=True)
            (ui / relative).write_text(content)
        if patched.get('workspace'):
            cfg['patchedDependencies'] = patched['workspace']
        if patched.get('lock'):
            data['lock']['patchedDependencies'] = {
                key: hashlib.sha256((ui / relative).read_bytes()).hexdigest() for key, relative in patched['lock'].items()}
        for relative, content in patched.get('outside', {}).items():
            # The patch lives beside ui/, reached through a symlink inside it.
            (root / 'outside.patch').write_text(content)
            (ui / relative).unlink()
            (ui / relative).symlink_to(root / 'outside.patch')
        for relative, content in patched.get('mutate', {}).items():
            (ui / relative).write_text(content)
        (ui/'pnpm-workspace.yaml').write_text(yaml.safe_dump(cfg))
        # Extra lock packages: 'name@version' -> {license, snapshotSuffix}; metadata is planted beside the lock.
        extra_metadata = {}
        for key, spec in case.get('extraPackages', {}).items():
            name, version = key.rsplit('@', 1)
            data['lock']['packages'][key] = copy.deepcopy(data['lock']['packages']['fixture@1.0.0'])
            data['lock']['snapshots'][key + spec.get('snapshotSuffix', '')] = {}
            release = copy.deepcopy(data['metadata']['versions']['1.0.0'])
            release['license'] = spec.get('license', 'MIT')
            release['dist']['tarball'] = f'https://registry.npmjs.org/{name}/-/x-{version}.tgz'
            extra_metadata[name] = {'versions': {version: release}, 'time': {version: '2020-01-01T00:00:00Z'}}
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
        for name, metadata in extra_metadata.items():
            (cache / (name.replace('/', '__') + '.json')).write_text(json.dumps(metadata))
        audit = {'lockSha256': hashlib.sha256((ui/'pnpm-lock.yaml').read_bytes()).hexdigest(),
                 'checkedAt': now.isoformat(), 'report': data['audit']}
        (cache / 'audit.json').write_text(json.dumps(audit))
        if case.get('installed'):
            installed = ui/'node_modules/.pnpm'/case.get('installedDir', 'fixture@1.0.0')/'node_modules'/case.get('installedName', 'fixture')
            installed.mkdir(parents=True)
            (installed/'package.json').write_text(json.dumps(case['installed']))
            (root / 'outside').mkdir()
            for relative, manifest in case.get('installedFiles', {}).items():
                extra = ui/'node_modules/.pnpm'/case.get('installedDir', 'fixture@1.0.0')/relative
                extra.parent.mkdir(parents=True, exist_ok=True)
                # A string is raw file text (e.g. a pnpm shim); anything else is a JSON manifest.
                extra.write_text(manifest if isinstance(manifest, str) else json.dumps(manifest))
            for relative, target in case.get('installedLinks', {}).items():
                link = ui/'node_modules/.pnpm'/case.get('installedDir', 'fixture@1.0.0')/relative
                link.parent.mkdir(parents=True, exist_ok=True)
                link.symlink_to(target.replace('@OUTSIDE@', str(root / 'outside')))
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
