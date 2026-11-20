#!/usr/bin/env python3
"""Real gate on planted lock/audit/registry evidence; fake pnpm only emits JSON."""
import copy
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import yaml

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent / 'exceptions'
BASE = json.loads((HERE / 'base.json').read_text())
now = dt.datetime(2026, 11, 18, 18, 0, tzinfo=dt.timezone.utc)
for case_file in sorted(HERE.glob('case-*.json')):
    case = json.loads(case_file.read_text())
    now = dt.datetime.fromisoformat(case.get('testDate', '2026-11-18') + 'T18:00:00+00:00')
    data = copy.deepcopy(BASE)
    for pointer, value in case.get('replace', {}).items():
        parts = pointer.split('/')
        target = data
        for part in parts[:-1]:
            target = target[int(part)] if isinstance(target, list) else target[part]
        if isinstance(target, list):
            target[int(parts[-1])] = value
        else:
            target[parts[-1]] = value
    if case.get('cleanAudit'):
        data['audit']['advisories'] = {}
        data['audit']['metadata']['vulnerabilities'] = dict.fromkeys(data['audit']['metadata']['vulnerabilities'], 0)
    if case.get('extraAdvisory'):
        other = copy.deepcopy(data['audit']['advisories']['1247710'])
        other['github_advisory_id'] = 'GHSA-xxxx-yyyy-zzzz'
        data['audit']['advisories']['999999'] = other
        data['audit']['metadata']['vulnerabilities']['high'] += 1
    if case.get('runtimeLock'):
        dev = data['lock']['importers']['.'].pop('devDependencies')
        data['lock']['importers']['.']['dependencies'] = dev
        data['manifest']['dependencies'] = data['manifest'].pop('devDependencies')
    if case.get('bypassVite'):
        data['manifest'].setdefault('dependencies', {})['picomatch'] = '4.0.7'
        data['lock']['importers']['.']['dependencies'] = {'picomatch': {'specifier': '4.0.7', 'version': '4.0.7'}}
    if case.get('newRegistryRelease'):
        data['metadata']['picomatch']['versions']['4.0.8'] = {}
        data['metadata']['picomatch']['time']['4.0.8'] = '2026-11-18T00:00:00Z'
    with tempfile.TemporaryDirectory(prefix='hoh-exception-') as directory:
        root = Path(directory)
        ui = root / 'ui'
        cache = ui / '.supply-cache'
        cache.mkdir(parents=True)
        (root / 'docs/ts').mkdir(parents=True)
        for filename in ('.npmrc', 'pnpm-workspace.yaml'):
            shutil.copyfile(ROOT / 'ui' / filename, ui / filename)
        # Fixture locks carry no patches; the real workspace's patchedDependencies would need patch files.
        cfg = yaml.safe_load((ui / 'pnpm-workspace.yaml').read_text())
        cfg.pop('patchedDependencies', None)
        (ui / 'pnpm-workspace.yaml').write_text(yaml.safe_dump(cfg))
        (ui / 'package.json').write_text(json.dumps(data['manifest']))
        (ui / 'pnpm-lock.yaml').write_text(yaml.safe_dump(data['lock']))
        direct = data['manifest'].get('dependencies', {}) | data['manifest'].get('devDependencies', {})
        policy = {'direct': direct, 'licenses': ['MIT']}
        rows = ''.join(f'| `{name}` | {version} | 2020-01-01 | MIT | Planted build fixture |\n' for name, version in direct.items())
        (root / 'docs/ts/DEPS.md').write_text('```json\n' + json.dumps(policy) + '\n```\n' + rows)
        for name, metadata in data['metadata'].items():
            (cache / (name + '.json')).write_text(json.dumps(metadata))
        checked = now - dt.timedelta(days=2) if case.get('staleAudit') else now
        audit = {'lockSha256': hashlib.sha256((ui / 'pnpm-lock.yaml').read_bytes()).hexdigest(),
                 'checkedAt': checked.isoformat(), 'report': data['audit']}
        if case.get('wrongLockAudit'):
            audit['lockSha256'] = '0' * 64
        (cache / 'audit.json').write_text(json.dumps(audit))
        if not case.get('noException'):
            (ui / 'audit-exceptions.json').write_text(json.dumps(data['exception'], ensure_ascii=False))
        arguments = ['/usr/bin/python3', '-I', str(HERE.parent / 'gate-at-ceo-date.py'),
                     '--test-date=' + case.get('testDate', '2026-11-18'), '--root', directory]
        env = os.environ.copy()
        if case.get('refresh'):
            # Cache was just written: original refresh reads it without HTTP.
            # The only executable named pnpm here is this non-networking JSON printer.
            tools = root / 'bin'
            tools.mkdir()
            stub = tools / 'pnpm'
            report = json.dumps(data['audit'])
            status = case.get('auditExit', 1 if data['audit']['advisories'] else 0)
            stub.write_text('#!/usr/bin/python3 -I\nimport sys\nassert sys.argv[1:] == ' + repr(['-C', str(ui), 'audit', '--json', '--no-offline', '--optional']) + '\nprint(' + repr(report) + ')\nsys.exit(' + str(status) + ')\n')
            stub.chmod(0o755)
            env['PATH'] = str(tools) + ':/usr/bin:/bin'
            arguments.append('--refresh')
        # seccomp denies real network even if the refresh implementation regresses.
        # Preserve the fixture PATH via env(1) arguments after offline.py's clean boundary.
        command = ['/usr/bin/python3', '-I', str(ROOT / 'ui/scripts/offline.py'),
                   '/usr/bin/env', 'PATH=' + env['PATH'], *arguments]
        result = subprocess.run(command, capture_output=True, text=True, env=env)
        expected = case.get('error')
        if expected:
            assert result.returncode != 0 and expected in result.stderr, (case_file.name, result.stdout, result.stderr)
        else:
            assert result.returncode == 0, (case_file.name, result.stdout, result.stderr)
            if not case.get('noException'):
                assert 'accepted: GHSA-fc5q-39xc-vjpf picomatch@4.0.7' in result.stdout
        # Raw vulnerable evidence must not be rewritten to look clean by acceptance.
        saved = json.loads((cache / 'audit.json').read_text())
        assert saved['report'] == data['audit'], case_file.name
        print('PASS exception ' + case_file.stem + ': ' + (expected or 'accepted control'))

# Finally prove the real saved lock (peer contexts and all nine audit paths) works
# through an alternate input tree. No pnpm config query or installed code runs.
with tempfile.TemporaryDirectory(prefix='hoh-real-evidence-') as directory:
    root = Path(directory)
    (root / 'ui').mkdir()
    (root / 'docs/ts').mkdir(parents=True)
    for relative in ('ui/package.json', 'ui/pnpm-lock.yaml', 'ui/.npmrc', 'ui/pnpm-workspace.yaml',
                     'docs/ts/DEPS.md', 'docs/ts/npm-lock-metadata.json',
                     'docs/ts/pnpm-audit.json'):
        shutil.copyfile(ROOT / relative, root / relative)
    shutil.copytree(ROOT / 'ui/patches', root / 'ui/patches')
    active_exception = ROOT / 'ui/audit-exceptions.json'
    if active_exception.exists():
        shutil.copyfile(active_exception, root / 'ui/audit-exceptions.json')
    # This control tests the real dependency graph, not the age of a historical
    # live response: only the temporary fixture's timestamp uses the test clock.
    audit_path = root / 'docs/ts/pnpm-audit.json'
    audit = json.loads(audit_path.read_text())
    audit['checkedAt'] = '2026-11-18T18:00:00+00:00'
    audit_path.write_text(json.dumps(audit))
    result = subprocess.run(['/usr/bin/python3', '-I', str(ROOT / 'ui/scripts/offline.py'),
                             '/usr/bin/python3', '-I', str(HERE.parent / 'gate-at-ceo-date.py'), '--root', directory],
                            capture_output=True, text=True)
    assert result.returncode == 0, (result.stdout, result.stderr)
    assert ('accepted: GHSA-fc5q-39xc-vjpf' in result.stdout) == active_exception.exists()
print('PASS exception real saved evidence: actual lock and saved audit (active exception only when recorded); no package-manager invocation')
