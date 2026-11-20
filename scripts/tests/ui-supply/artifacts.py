#!/usr/bin/env python3
"""Exercise only the read-only verify.sh window mode on planted bytes."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
with tempfile.TemporaryDirectory(prefix='hoh-artifacts-') as directory:
    root = Path(directory)
    dist = root / 'dist'
    dist.mkdir()
    js = dist / 'index.js'
    source_map = dist / 'index.js.map'
    binary = root / 'hoh-desktop'

    def check(name, content, expected):
        js.write_bytes(b'console.log("shell");')
        source_map.write_bytes(b'{"sources":["shell.tsx"]}')
        binary.write_bytes(b'\x7fELF\0shell\xff')
        for path, data in content.items():
            path.write_bytes(data)
        result = subprocess.run([str(ROOT / 'scripts/verify.sh'), '--window-artifacts-only', str(dist), str(binary)],
                                capture_output=True, text=True)
        assert (result.returncode == 0) == expected, (name, result.stdout, result.stderr)
        if not expected:
            assert 'picomatch found' in result.stderr, (name, result.stderr)
        print('PASS window artifacts: ' + name)

    check('clean dist + binary', {}, True)
    check('JavaScript', {js: b'/* picomatch */'}, False)
    check('source map', {source_map: b'{"sources":["node_modules/picomatch/index.js"]}'}, False)
    check('binary string', {binary: b'\x7fELF\0picomatch\0'}, False)
    check('binary chunk boundary', {binary: b'x' * 65532 + b'picomatch\0'}, False)
    check('binary UTF-16 string', {binary: 'PICOMATCH'.encode('utf-16le')}, False)
    check('unrelated package', {js: b'console.log("picocolors")'}, True)
    # Missing output must not be reported as a passed scan.
    binary.unlink()
    result = subprocess.run([str(ROOT / 'scripts/verify.sh'), '--window-artifacts-only', str(dist), str(binary)],
                            capture_output=True, text=True)
    assert result.returncode != 0 and 'binary missing' in result.stderr
    print('PASS window artifacts: missing binary rejected')

    js.write_bytes(b'console.log("shell")')
    source_map.write_bytes(b'{"sources":["shell.tsx"]}')
    result = subprocess.run([str(ROOT / 'scripts/verify.sh'), '--window-artifacts-only', str(dist)], capture_output=True, text=True)
    assert result.returncode == 0 and 'skipped: Tauri binary not supplied' in result.stdout
    print('PASS window artifacts: dist-only explicitly reports native check skipped')
    source_map.write_bytes(b'picomatch')
    result = subprocess.run([str(ROOT / 'scripts/verify.sh'), '--window-artifacts-only', str(dist)], capture_output=True, text=True)
    assert result.returncode != 0 and 'picomatch found' in result.stderr
    print('PASS window artifacts: dist-only still rejects poisoned maps')
