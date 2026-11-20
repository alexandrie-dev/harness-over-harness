#!/usr/bin/env python3
"""Проверка перенесённого кода T3 (ui/src/t3) без сборки: у каждого импорта есть адресат, и список пакетов, которых нет в окне.

Запуск: python3 -I ui/scripts/t3-check.py. Печатает неразрешённые импорты и по пакетам — какие файлы T3 их используют.
"""
import json, os, re, sys
UI = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
T3DIR = os.path.join(UI, 'src', 't3')
aliases = json.load(open(os.path.join(T3DIR, 'aliases.json')))
manifest = json.load(open(os.path.join(T3DIR, 'manifest.json')))
pkg = json.load(open(os.path.join(UI, 'package.json')))
have = set(pkg['dependencies']) | set(pkg['devDependencies']) | {'react/jsx-runtime', 'react-dom/client'}
deferred = {f"{r}/{e['file']}" for r, es in manifest['files'].items() for e in es if isinstance(e, dict)}
imp = re.compile(r'''(?:import|export)\s(?:type\s)?[^;'"]*?from\s+['"]([^'"]+)['"]|import\(\s*['"]([^'"]+)['"]\s*\)|^import\s+['"]([^'"]+)['"]''', re.S | re.M)
def root_pkg(spec):
    parts = spec.split('/'); return '/'.join(parts[:2]) if spec.startswith('@') else parts[0]
def exists(base):
    return any(os.path.isfile(c) for c in (base, base + '.ts', base + '.tsx', base + '/index.ts', base + '/index.tsx'))
bad, missing = [], {}
for dirpath, _, names in os.walk(T3DIR):
    for name in names:
        if not name.endswith(('.ts', '.tsx')) or name.endswith('.d.ts'): continue
        path = os.path.join(dirpath, name); rel = os.path.relpath(path, T3DIR)
        for m in imp.finditer(open(path).read()):
            spec = m.group(1) or m.group(2) or m.group(3)
            if spec.startswith('.'):
                if not exists(os.path.normpath(os.path.join(dirpath, spec))): bad.append((rel, spec))
            elif spec.startswith('~/'):
                if not exists(os.path.join(T3DIR, 'web', spec[2:])): bad.append((rel, spec))
            elif spec in aliases: pass
            elif spec.startswith('@t3tools/'): bad.append((rel, spec))
            elif spec.startswith('node:') or spec in ('effect',) or spec.startswith('effect/'):
                (bad if spec.startswith('effect') else []).append((rel, spec))
            elif root_pkg(spec) in have or spec.startswith('@base-ui/react') : pass
            else: missing.setdefault(root_pkg(spec), set()).add((rel, rel in deferred))
for rel, spec in sorted(bad): print('НЕТ АДРЕСАТА', rel, spec)
print('--- пакеты, которых нет в package.json окна (кто импортирует; * — файл отложен):')
for p, files in sorted(missing.items()): print(p, '::', ', '.join(sorted(f + ('*' if d else '') for f, d in files)))
sys.exit(1 if bad else 0)
