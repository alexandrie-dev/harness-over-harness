#!/usr/bin/env python3
"""Fail closed on unknown metadata, lock formats and unavailable audit data.
Requires the host's Python 3 + PyYAML; installs nothing. Network only with --refresh.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import tomllib
import urllib.request
import urllib.parse
import yaml

ROOT = Path(__file__).resolve().parents[2]


def require(ok, message):
    if not ok:
        raise ValueError(message)


def read_json(path):
    return json.loads(path.read_text())


def dependency_key(name, reference):
    """Lock snapshot key of a dependency edge; an npm: alias is stored as 'real-name@version(...)'."""
    reference = str(reference)
    return reference if '@' in reference.split('(')[0].lstrip('@') else name + '@' + reference


def license_allowed(license, key, policy):
    """Listed license, or the exact name@version exception: exact license string + non-empty written reason."""
    if isinstance(license, str) and license in policy['licenses']:
        return True
    exception = policy.get('licenseExceptions', {}).get(key)
    if not isinstance(exception, dict):
        return False
    reason = exception.get('reason')
    return (isinstance(license, str) and bool(license) and exception.get('license') == license and
            isinstance(reason, str) and bool(reason.strip()))


def download(name, cache):
    path = cache / (name.replace('/', '__') + '.json')
    if path.exists() and time.time() - path.stat().st_mtime < 3600:
        return read_json(path)
    for attempt in range(5):
        try:
            request = urllib.request.Request('https://registry.npmjs.org/' + name.replace('/', '%2f'),
                                             headers={'User-Agent': 'hoh-ui-supply-check'})
            # Redirects are forbidden: this task only authorizes the npm registry.
            class NoRedirect(urllib.request.HTTPRedirectHandler):
                def redirect_request(self, *args, **kwargs):
                    raise ValueError('registry redirect forbidden')
            with urllib.request.build_opener(NoRedirect).open(request, timeout=45) as response:
                data = json.load(response)
            path.write_text(json.dumps(data))
            return data
        except Exception:
            if attempt == 4:
                raise ValueError(f'metadata: failed to fetch {name}')
            time.sleep(1)


def audit_clean(report):
    require(isinstance(report, dict) and 'error' not in report, 'audit: unavailable or malformed report')
    counts = report.get('metadata', {}).get('vulnerabilities')
    require(isinstance(counts, dict) and counts, 'audit: missing vulnerability counts')
    require(all(isinstance(v, int) and v == 0 for v in counts.values()) and
            not report.get('advisories') and not report.get('vulnerabilities'),
            'audit: known vulnerability')


def check(args):
    if not shutil.which('cargo-deny'):
        print('skipped: cargo-deny not installed', flush=True)
    root = args.root.resolve()
    ui = root / 'ui'
    policy_text = (root / 'docs/ts/DEPS.md').read_text()
    match = re.search(r'```json\n(.*?)\n```', policy_text, re.S)
    require(match, 'policy: missing machine-readable policy')
    policy = json.loads(match[1])
    cfg = yaml.safe_load((ui / 'pnpm-workspace.yaml').read_text())
    expected = {'minimumReleaseAge': 10080, 'minimumReleaseAgeStrict': True,
                'minimumReleaseAgeExclude': [], 'allowBuilds': {}, 'ignoreScripts': True,
                'optional': False, 'ignoredOptionalDependencies': ['*'], 'autoInstallPeers': False, 'blockExoticSubdeps': True, 'trustLockfile': False,
                'frozenLockfile': True, 'offline': True, 'nodeLinker': 'isolated', 'verifyStoreIntegrity': True, 'registry': 'https://registry.npmjs.org/'}
    for key, value in expected.items():
        require(cfg.get(key) == value, f'config: {key} must be {value!r}')
    require('store-dir=/mnt/toshiba/alexandrie/hoh/pnpm-store' in (ui / '.npmrc').read_text(),
            'config: missing dedicated store')
    manifest = read_json(ui / 'package.json')
    require(not ({'packageManager', 'devEngines', 'pnpm', 'configDependencies'} & set(manifest)),
            'direct: automatic package manager/runtime downloads forbidden')
    require(manifest.get('engines', {}).get('pnpm') == '12.21.0', 'direct: pnpm engine must be 12.21.0')
    if root == ROOT:
        rust_allow = tomllib.loads((root / 'deny.toml').read_text())['licenses']['allow']
        require(set(rust_allow) <= set(policy['licenses']), 'license: deny.toml exceeds DEPS.md allow-list')
        require(subprocess.check_output(['pnpm', '--version'], text=True).strip() == '12.21.0', 'config: pnpm 12.21.0 required')
        effective = json.loads(subprocess.check_output(['pnpm', '-C', str(ui), 'config', 'list'], text=True))
        for key, value in expected.items():
            require(effective.get(key) == value, f'config: effective {key} differs')
        require(effective.get('storeDir') == '/mnt/toshiba/alexandrie/hoh/pnpm-store', 'config: effective store differs')
    direct = manifest.get('dependencies', {}) | manifest.get('devDependencies', {})
    require(direct == policy['direct'], 'direct: package.json differs from DEPS.md pins')
    require(not manifest.get('optionalDependencies'), 'direct: optional dependencies forbidden')
    for name, version in direct.items():
        require(re.fullmatch(r'\d+\.\d+\.\d+(?:-[\w.-]+)?', version), f'direct: non-exact version {name}')
        reason = re.search(r'^\| `' + re.escape(name) + r'` \| ' + re.escape(version) + r' \| [^|]+ \| [^|]+ \| ([^|]*) \|$', policy_text, re.M)
        require(reason and bool(reason[1].strip()), f'direct: missing documented reason for {name}')
    if args.config_only:
        print('passed: pnpm configuration and documented exact direct pins')
        return
    lock_path = ui / 'pnpm-lock.yaml'
    lock_bytes = lock_path.read_bytes()
    lock_hash = hashlib.sha256(lock_bytes).hexdigest()
    lock = yaml.safe_load(lock_bytes)
    require(str(lock.get('lockfileVersion')) == '9.0', 'lock: unsupported lock format')
    packages = lock.get('packages')
    require(isinstance(packages, dict) and packages, 'lock: empty packages')
    importers = lock.get('importers', {})
    require(set(importers) == {'.'}, 'lock: unexpected workspace importer')
    require(set(importers['.']) <= {'dependencies', 'devDependencies'}, 'lock: unsupported automatic dependencies')
    imported = importers['.'].get('dependencies', {}) | importers['.'].get('devDependencies', {})
    require(set(imported) == set(direct), 'lock: importer differs from direct dependencies')
    for name, entry in imported.items():
        require(entry['specifier'] == direct[name] and entry['version'].split('(')[0] == direct[name],
                f'lock: importer pin differs: {name}')
    snapshots = lock.get('snapshots', {})
    require(isinstance(snapshots, dict) and snapshots, 'lock: missing dependency snapshots')
    bases = {key.split('(')[0] for key in snapshots}
    require(bases == set(packages), 'lock: packages and snapshots differ')
    for key, snapshot in snapshots.items():
        for name, version in (snapshot.get('dependencies', {}) | snapshot.get('optionalDependencies', {})).items():
            # An npm: alias points at 'real-name@version'; that target key must exist and is checked below.
            require(dependency_key(name, version).split('(')[0] in packages, f'lock: missing dependency {name}@{version}')
    for name, entry in imported.items():
        require(name + '@' + entry['version'].split('(')[0] in packages, f'lock: missing direct package {name}')
    # Validate structure before the network phase; malformed locks never trigger requests.
    for key, entry in packages.items():
        require(re.fullmatch(r'(?:@[^/@]+/)?[^/@]+@\d+\.\d+\.\d+(?:-[\w.-]+)?', key), f'lock: exotic source {key}')
        resolution = entry.get('resolution', {})
        require(re.fullmatch(r'sha512-[A-Za-z0-9+/]{86}==', resolution.get('integrity', '')), f'integrity: missing/invalid SHA512 for {key}')
        require(set(resolution) == {'integrity'}, f'lock: non-registry resolution {key}')
    check_patches(ui, cfg, lock, packages)
    cache = ui / '.supply-cache'
    cache.mkdir(exist_ok=True)
    now = dt.datetime.now(dt.timezone.utc)
    evidence_path = root / 'docs/ts/npm-lock-metadata.json'
    evidence = read_json(evidence_path) if evidence_path.exists() else {}
    if evidence and not args.refresh:
        require(evidence['lockSha256'] == lock_hash, 'metadata: evidence bound to a different lock')
    seen = {}
    if args.refresh:
        names = sorted({key.rsplit('@', 1)[0] for key in packages})
        with ThreadPoolExecutor(max_workers=4) as pool:
            for name, metadata in zip(names, pool.map(lambda name: download(name, cache), names)):
                seen[name] = metadata
        print(f'metadata ready: {len(names)} package names', flush=True)
    for key, entry in packages.items():
        name, version = key.rsplit('@', 1)
        integrity = entry['resolution']['integrity']
        if name not in seen:
            path = cache / (name.replace('/', '__') + '.json')
            seen[name] = download(name, cache) if args.refresh else (read_json(path) if path.exists() else evidence['packages'][name])
        metadata = seen[name]
        release = metadata.get('versions', {}).get(version)
        require(release, f'metadata: unknown {key}')
        published = metadata.get('time', {}).get(version)
        require(published, f'age: missing publication time for {key}')
        age = now - dt.datetime.fromisoformat(published.replace('Z', '+00:00'))
        require(age >= dt.timedelta(days=7), f'age: {key} younger than 7 days')
        require(release.get('dist', {}).get('integrity') == integrity, f'integrity: registry mismatch for {key}')
        tarball = urllib.parse.urlparse(release.get('dist', {}).get('tarball', ''))
        require(tarball.scheme == 'https' and tarball.netloc == 'registry.npmjs.org', f'source: foreign tarball for {key}')
        # Exact name@version + exact license string, with a written reason (CEO 20.11.2026); never a pattern.
        require(license_allowed(release.get('license'), key, policy), f'license: forbidden {key}: {release.get("license")}')
        scripts = release.get('scripts', {})
        hooks = set(scripts) & {'preinstall', 'install', 'postinstall', 'prepare'}
        allowed = set(policy.get('scriptExceptions', {}).get(key, []))
        require(not (hooks - allowed) and not release.get('gypfile'), f'script: install hook/binding.gyp in {key}: {sorted(hooks)}')
    if args.refresh:
        compact = {}
        for key in packages:
            name, version = key.rsplit('@', 1)
            release = seen[name]['versions'][version]
            compact.setdefault(name, {'versions': {}, 'time': {}})
            compact[name]['time'][version] = seen[name]['time'][version]
            compact[name]['versions'][version] = {key: release[key] for key in ['license', 'scripts', 'gypfile', 'dist'] if key in release}
        evidence_path.write_text(json.dumps({'lockSha256': lock_hash, 'registry': expected['registry'], 'checkedAt': now.isoformat(), 'packages': compact}, indent=2) + '\n')
    print(f'passed: integrity, exact pins, 7-day age, scripts, licenses ({len(packages)} lock packages)')
    # The manifest inside every materialized tarball is checked as well as registry metadata.
    store = Path(os.path.realpath(ui / 'node_modules/.pnpm'))
    for directory in sorted(store.iterdir()) if store.is_dir() else []:
        if directory.name == 'lock.yaml' and directory.is_file():
            continue
        require(not directory.is_symlink() and directory.is_dir(), f'installed: unexpected store entry {directory.name}')
        if directory.name == 'node_modules':  # pnpm's hoisted symlinks: same rules as any node_modules
            inspect_modules(directory, policy, packages, store)
            continue
        found = STORE_ENTRY.fullmatch(directory.name)
        require(found, f'installed: unexpected store entry {directory.name}')
        name, version = found[1].replace('+', '/', 1), found[2]
        require(name + '@' + version in packages, f'installed: not in lock {name}@{version}')
        modules = directory / 'node_modules'
        require(not modules.is_symlink() and not (modules / name).is_symlink(), f'installed: node_modules is a symlink in {directory.name}')
        manifest = modules / name / 'package.json'
        require(manifest.is_file(), f'installed: missing manifest for {name}@{version}')
        inspect_installed(manifest, policy, name, version)
        # Everything else physically present (real neighbours, nested node_modules) is inspected too.
        inspect_modules(modules, policy, packages, store, modules / name)
    audit_path = cache / 'audit.json'
    if not audit_path.exists() and (root / 'docs/ts/pnpm-audit.json').exists():
        audit_path = root / 'docs/ts/pnpm-audit.json'
    if args.refresh:
        result = subprocess.run(['pnpm', '-C', str(ui), 'audit', '--json', '--no-offline', '--optional'],
                                text=True, capture_output=True)
        try:
            report = json.loads(result.stdout)
        except json.JSONDecodeError:
            raise ValueError('audit: pnpm audit failed: ' + result.stderr[:500])
        (cache / 'audit.raw.json').write_text(json.dumps(report, indent=2))
        audit_path = cache / 'audit.json'
        audit_path.write_text(json.dumps({'lockSha256': lock_hash, 'checkedAt': now.isoformat(), 'report': report}, indent=2))
        (root / 'docs/ts/pnpm-audit.json').write_text(audit_path.read_text() + '\n')
        audit_clean(report)
        require(result.returncode == 0, 'audit: pnpm audit returned failure')
    record = read_json(audit_path)
    require(record['lockSha256'] == lock_hash, 'audit: lock changed since audit')
    checked = dt.datetime.fromisoformat(record['checkedAt'])
    require(dt.timedelta() <= now - checked <= dt.timedelta(hours=24), 'audit: stale/future report; run --refresh')
    audit_clean(record['report'])
    print('passed: pnpm audit (whole lock, including optional entries; lock-bound report <24h)')
    if shutil.which('cargo-deny'):
        # --disable-fetch is deliberate: this task does not authorize GitHub/RustSec network access.
        env = os.environ | {'CARGO_BUILD_JOBS': '2', 'CARGO_TARGET_DIR': '/mnt/toshiba/alexandrie/hoh/target-ui2',
                            'CARGO_HOME': '/mnt/toshiba/alexandrie/hoh/cargo-home-ui2', 'CARGO_NET_OFFLINE': 'true'}
        subprocess.run(['cargo', 'deny', '--manifest-path', str(root / 'apps/hoh-desktop/Cargo.toml'),
                        '--locked', '--offline', 'check', '--disable-fetch'], env=env, check=True, cwd=root)


def check_patches(ui, cfg, lock, packages):
    """patchedDependencies: workspace set == lock set; each patch file hashes to the lock value (pnpm: sha256, LF-normalized)."""
    workspace = cfg.get('patchedDependencies') or {}
    locked = lock.get('patchedDependencies') or {}
    require(isinstance(workspace, dict) and isinstance(locked, dict), 'patch: malformed patchedDependencies')
    require(set(workspace) == set(locked), 'patch: workspace and lock patchedDependencies differ')
    for key, relative in workspace.items():
        require(key in packages, f'patch: {key} is not an exact name@version in the lock')
        path = (ui / str(relative)).resolve()
        require(path.is_file(), f'patch: missing file for {key}')
        require(path.is_relative_to(ui.resolve()), f'patch: file outside ui/ for {key}')
        digest = hashlib.sha256(path.read_bytes().replace(b'\r\n', b'\n')).hexdigest()
        require(locked[key] == digest, f'patch: hash differs for {key}')


STORE_ENTRY = re.compile(r'((?:@[^+@]+\+)?[^+@]+)@(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(?:_.*)?')


def inspect_modules(modules, policy, packages, store, own=None):
    """Inspect everything physically under a node_modules directory, recursively.

    Real packages must be lock keys and pass the manifest checks. A symlink must lead to
    <store>/<lock key directory>/node_modules/<name>. Only .bin (shims inside the store) is
    special; every other entry, dotted or not, follows the same rule.
    """
    if modules.is_symlink():
        raise ValueError(f'installed: node_modules is a symlink: {modules}')
    if not modules.is_dir():
        return
    for entry in sorted(modules.iterdir()):
        if entry.name == '.bin' and not entry.is_symlink():
            inspect_bin(entry, store, packages)
            continue
        candidates = [entry]
        if entry.name.startswith('@') and entry.is_dir() and not entry.is_symlink():
            candidates = sorted(entry.iterdir())
        for candidate in candidates:
            relative = candidate.relative_to(modules).as_posix()
            if candidate.is_symlink():
                target = Path(os.path.realpath(candidate))
                parts = target.relative_to(store).parts if target.is_relative_to(store) else ()
                found = len(parts) >= 3 and parts[1] == 'node_modules' and STORE_ENTRY.fullmatch(parts[0])
                require(found, f'installed: symlink leaves the store: {relative}')
                name, version = found[1].replace('+', '/', 1), found[2]
                require(name + '@' + version in packages and '/'.join(parts[2:]) == name,
                        f'installed: symlink to unknown package: {relative}')
                continue
            require(candidate.is_dir(), f'installed: unexpected file {relative}')
            if candidate != own:
                manifest = candidate / 'package.json'
                require(manifest.is_file(), f'installed: unexpected entry {relative}')
                pkg = read_json(manifest)
                require(pkg.get('name') == relative and isinstance(pkg.get('version'), str) and
                        relative + '@' + pkg['version'] in packages, f'installed: not in lock {relative}')
                inspect_installed(manifest, policy, relative, pkg['version'])
            inspect_modules(candidate / 'node_modules', policy, packages, store)


def inspect_bin(directory, store, packages):
    """.bin holds only symlinks or pnpm shims whose target, after realpath, is inside a lock-key store directory."""
    for entry in sorted(directory.iterdir()):
        if entry.is_symlink():
            target = Path(os.path.realpath(entry))
        else:
            require(entry.is_file(), f'installed: unexpected entry in .bin: {entry.name}')
            targets = re.findall(r'^# cmd-shim-target=(\S+)$', entry.read_text(errors='replace'), re.M)
            require(len(targets) == 1, f'installed: unrecognised shim .bin/{entry.name}')
            # realpath: the tree may be reached through a link (e.g. a moved worktree); the real file must still be in the store.
            target = Path(os.path.realpath(directory / targets[0]))
        parts = target.relative_to(store).parts if target.is_relative_to(store) else ()
        found = parts and STORE_ENTRY.fullmatch(parts[0])
        require(found and found[1].replace('+', '/', 1) + '@' + found[2] in packages, f'installed: .bin/{entry.name} leaves the store')


def inspect_installed(path, policy, name, version):
    pkg = read_json(path)
    # The store directory name decides the identity; a manifest must not claim another package's exceptions.
    require(pkg.get('name') == name and pkg.get('version') == version, f'installed: manifest differs from {name}@{version}')
    key = name + '@' + version
    require(name not in ('fsevents', '@napi-rs/canvas'), f'installed: forbidden {name}')
    # Foreign binary packages are not allowed even if someone overrides optional=false.
    if pkg.get('os') or pkg.get('cpu'):
        require(name in policy['direct'], f'installed: optional platform package {name}')
        require(pkg.get('os', ['linux']) == ['linux'] and pkg.get('cpu', ['x64']) == ['x64'],
                f'installed: foreign platform {name}')
    hooks = set(pkg.get('scripts', {})) & {'preinstall', 'install', 'postinstall', 'prepare'}
    allowed = set(policy.get('scriptExceptions', {}).get(key, []))
    require(not (hooks - allowed) and not (path.parent / 'binding.gyp').exists(), f'script: installed hook/binding.gyp in {name}')
    require(license_allowed(pkg.get('license'), key, policy), f'license: installed {name}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config-only', action='store_true', help='preflight before the first resolve, without network')
    parser.add_argument('--refresh', action='store_true', help='npm registry metadata + pnpm audit; no other network')
    parser.add_argument('--root', type=Path, default=ROOT, help='alternate complete input tree for planted tests')
    try:
        check(parser.parse_args())
    except Exception as error:
        print(f'ui-supply-check: {error}', file=sys.stderr)
        sys.exit(1)
