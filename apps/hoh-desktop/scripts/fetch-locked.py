#!/usr/bin/env python3
"""Recover crates.io downloads from DNS failures, preserving TLS + lock SHA256.
Only populates the isolated archive cache, never installs or builds anything.
--cdn-address must be an address obtained for static.crates.io from system DNS.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import ipaddress
import json
import os
import re
from pathlib import Path
import subprocess
import tomllib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--cdn-address', action='append', default=[])
args = parser.parse_args()
for address in args.cdn_address:
    ipaddress.ip_address(address)
root = Path(__file__).resolve().parents[3]
lock = tomllib.loads((root/'apps/hoh-desktop/Cargo.lock').read_text())
cache = Path('/mnt/toshiba/alexandrie/hoh/cargo-home-ui2/registry/cache/index.crates.io-1949cf8c6b5b557f')
cache.mkdir(parents=True, exist_ok=True)
packages = [p for p in lock['package'] if p.get('source')]
assert all(re.fullmatch(r'[A-Za-z0-9_-]+', p['name']) and
           re.fullmatch(r'[0-9A-Za-z.+-]+', p['version']) and
           re.fullmatch(r'[0-9a-f]{64}', p['checksum']) for p in packages)
assert all(p['source'] == 'registry+https://github.com/rust-lang/crates.io-index' and p.get('checksum') for p in packages)
missing = [p for p in packages if not (cache/(p['name']+'-'+p['version']+'.crate')).exists()]
missing.sort(key=lambda p: (not p['name'].startswith(('tauri', 'wry', 'tao', 'muda')), p['name']))
print(f'Remaining locked archives: {len(missing)}; two workers', flush=True)


def get(p):
    name = p['name']+'-'+p['version']+'.crate'
    target = cache/name
    tmp = cache/(name+'.hoh-download')
    for address in args.cdn_address or [None]:
        resolve = ['--resolve', 'static.crates.io:443:'+address] if address else []
        result = subprocess.run(['curl', '--fail', '--silent', '--show-error', '--connect-timeout', '8',
                                 '--max-time', '30', *resolve, '--output', str(tmp),
                                 'https://static.crates.io/crates/'+p['name']+'/'+name], capture_output=True, text=True)
        if result.returncode:
            continue
        if hashlib.sha256(tmp.read_bytes()).hexdigest() != p['checksum']:
            raise ValueError('SHA256 mismatch: '+name)
        os.replace(tmp, target)
        return name
    raise RuntimeError(name+': '+result.stderr.strip())


success = []
error = None
pool = ThreadPoolExecutor(max_workers=2)
futures = [pool.submit(get, p) for p in missing]
try:
    for future in futures:
        name = future.result()
        success.append(name)
        if len(success) % 10 == 0:
            print(f'Checksum-verified: {len(success)}/{len(missing)}', flush=True)
except Exception as failure:
    error = str(failure)
    for future in futures:
        future.cancel()
finally:
    pool.shutdown(wait=True, cancel_futures=True)
receipt = {'lockPackages': len(packages), 'archivesPresent': len(list(cache.glob('*.crate'))),
           'downloadedThisStep': len(success), 'error': error, 'registry': 'crates.io',
           'tlsCertificateValidation': True, 'downloadedChecksumsVerified': True}
(root/'docs/ts/crate-acquisition.json').write_text(json.dumps(receipt, indent=2)+'\n')
print(json.dumps(receipt), flush=True)
if error:
    raise SystemExit(1)
