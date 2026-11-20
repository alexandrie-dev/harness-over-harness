#!/usr/bin/env bash
# Offline by default. --refresh is the explicit npm-registry-only acquisition/audit step.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
# Extend the existing gate without changing ui/scripts/supply-check.py or its evidence.
exec /usr/bin/python3 -I - "$root" "$@" <<'PY'
import argparse
import copy
import datetime as dt
import importlib.util
import json
from pathlib import Path
import re
import sys
import types

sys.dont_write_bytecode = True
ROOT = Path(sys.argv[1])
spec = importlib.util.spec_from_file_location('supply_gate', ROOT / 'ui/scripts/supply-check.py')
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
parser = argparse.ArgumentParser(description='UI supply gate with a narrowly scoped CEO risk acceptance')
parser.add_argument('--config-only', action='store_true')
parser.add_argument('--refresh', action='store_true')
parser.add_argument('--root', type=Path, default=ROOT)
args = parser.parse_args(sys.argv[2:])
root = args.root.resolve()
GHSA = 'GHSA-fc5q-39xc-vjpf'
REASON = 'ReDoS на extglob-шаблонах; шаблоны только из репозитория; внешний модуль через окружение закрыт чистым окружением сборки'
original_audit = gate.audit_clean
require = gate.require
accepted = False
exception = None


def load_exception():
    path = root / 'ui/audit-exceptions.json'
    if not path.exists():
        return None
    record = gate.read_json(path)
    require(record.get('advisory') == GHSA and record.get('package') == 'picomatch' and
            record.get('version') == '4.0.7', 'exception: unsupported advisory/package/version')
    granted = record.get('granted', {})
    review = dt.date.fromisoformat(record['review_by'])
    require(dt.datetime.now(dt.timezone.utc).date() <= review, 'exception: expired; CEO review required')
    require(granted == {'date': '2026-11-18', 'by': 'CEO'} and
            review == dt.date(2026, 11, 18) + dt.timedelta(days=14), 'exception: invalid CEO decision/review period')
    require(record.get('reason') == REASON and record.get('build_only') is True and
            record.get('withdrawn_on') == '2026-11-18' and record.get('status') == 'Withdrawn' and
            record.get('url') == 'https://github.com/advisories/' + GHSA,
            'exception: incomplete risk acceptance')
    return record


def build_paths():
    """Prove ALL importer-to-picomatch paths are the audited build-only routes.

    Audit's dev=true alone is insufficient: it must agree with the lock graph.
    Peer-context snapshot keys are followed verbatim, including optional edges.
    """
    manifest = gate.read_json(root / 'ui/package.json')
    lock = gate.yaml.safe_load((root / 'ui/pnpm-lock.yaml').read_text())
    snapshots = lock['snapshots']
    found = set()
    allowed_roots = {'vite', '@vitejs/plugin-react', '@tailwindcss/vite'}
    allowed_tails = {('vite', 'picomatch'), ('vite', 'tinyglobby', 'picomatch'),
                     ('vite', 'tinyglobby', 'fdir', 'picomatch')}
    visits = 0

    def walk(name, version, names, active, group):
        nonlocal visits
        visits += 1
        require(visits <= 50000, 'exception: dependency graph too complex')
        # An npm: alias edge is stored as 'real-name@version(...)'; follow and name the REAL package.
        key = gate.dependency_key(name, version)
        require(key in snapshots, 'exception: missing snapshot ' + key)
        name = key.split('(')[0].rsplit('@', 1)[0]
        version = key[len(name) + 1:]
        if key in active:
            return
        route = (*names, name)
        if name == 'picomatch':
            tail = route if route[0] == 'vite' else route[1:]
            require(str(version) == '4.0.7' and group == 'devDependencies' and
                    route[0] in allowed_roots and tail in allowed_tails and
                    route[0] not in manifest.get('dependencies', {}),
                    'exception: picomatch is not exclusively in audited build paths')
            found.add('.>' + '>'.join(route))
            return
        # Source inspection/CEO acceptance covers these exact parent releases.
        if name in ('vite', 'tinyglobby'):
            expected = {'vite': '8.3.7', 'tinyglobby': '0.2.17'}[name]
            require(str(version).split('(')[0] == expected, 'exception: unreviewed build tool version')
        entry = snapshots[key]
        for child, reference in (entry.get('dependencies', {}) | entry.get('optionalDependencies', {})).items():
            walk(child, reference, route, active | {key}, group)

    for group, entries in lock['importers']['.'].items():
        for name, entry in entries.items():
            walk(name, entry['version'], (), set(), group)
    require(found, 'exception: no audited build path')
    return found


def audit_with_exception(report):
    global accepted
    if exception is None:
        return original_audit(report)
    require(isinstance(report, dict) and 'error' not in report, 'audit: unavailable or malformed report')
    advisories = report.get('advisories', {})
    require(isinstance(advisories, dict), 'audit: unsupported advisories format')
    matches = [item for item in advisories.values()
               if item.get('github_advisory_id') == GHSA and item.get('module_name') == 'picomatch']
    if not matches:
        original_audit(report)  # Other findings still fail; a clean report is never forged.
        require(not args.refresh, 'exception: advisory absent after refresh; delete ui/audit-exceptions.json')
        return
    require(len(matches) == len(advisories) == 1 and not report.get('vulnerabilities'),
            'audit: known vulnerability outside exception')
    advisory = matches[0]
    require(advisory.get('patched_versions') in (None, '', '<0.0.0'),
            'exception: fixed version available; remove exception and update dependency')
    require(advisory.get('vulnerable_versions') == '<=4.0.7', 'exception: changed affected range; review required')
    if args.refresh:
        metadata = gate.read_json(root / 'ui/.supply-cache/picomatch.json')
        # Full registry packument acquired by the existing refresh. For this exact
        # affected range, a published release with a higher numeric version is outside it.
        newer = [version for version in metadata['versions']
                 if re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?', version) and
                 tuple(map(int, re.match(r'(\d+)\.(\d+)\.(\d+)', version).groups())) > (4, 0, 7)]
        require(not newer, 'exception: fixed version available in registry; remove exception and update dependency')
    paths = []
    findings = advisory.get('findings')
    require(isinstance(findings, list) and findings, 'exception: missing findings')
    for finding in findings:
        require(finding.get('version') == '4.0.7', 'audit: known vulnerability outside exception version')
        require(finding.get('dev') is True and not finding.get('optional') and not finding.get('bundled'),
                'exception: runtime/optional/bundled finding')
        entries = finding.get('paths')
        require(isinstance(entries, list) and entries and all(isinstance(p, str) for p in entries),
                'exception: missing build paths')
        paths.extend(entries)
    require(set(paths) == build_paths(), 'exception: audit paths differ from lock build paths')
    severity = advisory.get('severity')
    counts = report.get('metadata', {}).get('vulnerabilities')
    expected = {level: int(level == severity) for level in ('info', 'low', 'moderate', 'high', 'critical')}
    require(severity in expected and isinstance(counts, dict) and counts == expected and
            all(type(value) is int for value in counts.values()), 'audit: unaccounted vulnerability counts')
    # Apply original fail-closed validation to the remainder; disk evidence stays intact.
    remainder = copy.deepcopy(report)
    remainder['advisories'] = {}
    remainder['metadata']['vulnerabilities'] = dict.fromkeys(expected, 0)
    original_audit(remainder)
    accepted = True


# pnpm audit legitimately returns 1 for an accepted finding. Normalize ONLY that
# exact command/status, and only AFTER the same validator accepts its full report.
original_run = gate.subprocess.run

def run_with_exception(command, *positional, **keywords):
    result = original_run(command, *positional, **keywords)
    if (exception is not None and args.refresh and
            command == ['pnpm', '-C', str(root / 'ui'), 'audit', '--json', '--no-offline', '--optional'] and
            result.returncode == 1):
        audit_with_exception(json.loads(result.stdout))
        result.returncode = 0
    return result


gate.audit_clean = audit_with_exception
gate.subprocess = types.SimpleNamespace(run=run_with_exception, check_output=gate.subprocess.check_output)
try:
    if not args.config_only:
        exception = load_exception()
    gate.check(args)
    if accepted:
        print('accepted: ' + GHSA + ' picomatch@4.0.7, build-only, CEO 2026-11-18; review by 2026-12-02')
except Exception as error:
    print('ui-supply-check: ' + str(error), file=sys.stderr)
    sys.exit(1)
PY
