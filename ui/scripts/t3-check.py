#!/usr/bin/env python3
"""Проверка перенесённого кода T3 (ui/src/t3) без сборки: у каждого импорта есть адресат, и список пакетов, которых нет в окне.

Запуск: python3 -I ui/scripts/t3-check.py. Печатает неразрешённые импорты и по пакетам — какие файлы T3 их используют.
Два набора со своими псевдонимами: прежний перенос (src/t3, псевдонимы src/t3/aliases.json) и основа HOH-40
(src/t3/base, псевдонимы src/t3/base/aliases.json; * — файл отложен, его пакеты ещё не установлены).
"""
import json, os, re, sys
UI = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
T3DIR = os.path.join(UI, 'src', 't3')
pkg = json.load(open(os.path.join(UI, 'package.json')))
have = set(pkg['dependencies']) | set(pkg['devDependencies']) | {'react/jsx-runtime', 'react-dom/client'}
imp = re.compile(r'''(?:import|export)\s(?:type\s)?[^;'"]*?from\s+['"]([^'"]+)['"]|import\(\s*['"]([^'"]+)['"]\s*\)|^import\s+['"]([^'"]+)['"]''', re.S | re.M)
def root_pkg(spec):
    parts = spec.split('/'); return '/'.join(parts[:2]) if spec.startswith('@') else parts[0]
def exists(base):
    js = [re.sub(r'\.m?js$', e, base) for e in ('.ts', '.tsx')] if re.search(r'\.m?js$', base) else []  # TS: "./x.js" — это x.ts
    return any(os.path.isfile(c) for c in (base, *js, base + '.ts', base + '.tsx', base + '/index.ts', base + '/index.tsx'))

def check(name, top, aliases, tilde, roots, deferred):
    """top — папка набора; aliases — {спецификатор: путь от ui/}; tilde — папка для ~/; roots — обходимые подпапки top."""
    bad, missing, files = [], {}, 0
    for root in roots:
        for dirpath, _, names in os.walk(os.path.join(top, root)):
            for n in names:
                if not n.endswith(('.ts', '.tsx')) or n.endswith('.d.ts'): continue
                path = os.path.join(dirpath, n); rel = os.path.relpath(path, top); files += 1
                for m in imp.finditer(open(path, encoding='utf-8').read()):
                    spec = (m.group(1) or m.group(2) or m.group(3)).split('?')[0]
                    if spec.startswith('.'):
                        if not exists(os.path.normpath(os.path.join(dirpath, spec))): bad.append((rel, spec))
                    elif spec.startswith('~/'):
                        if not exists(os.path.join(tilde, spec[2:])): bad.append((rel, spec))
                    elif spec in aliases:
                        if not exists(os.path.join(UI, aliases[spec])): bad.append((rel, spec))
                    elif spec.startswith('@t3tools/'): bad.append((rel, spec))
                    elif spec.startswith('node:'): pass
                    elif spec == 'effect' or spec.startswith('effect/'):
                        if 'effect' not in have and name == 'прежний перенос': bad.append((rel, spec))
                        elif 'effect' not in have: missing.setdefault('effect', set()).add((rel, rel in deferred))
                    elif root_pkg(spec) in have or spec.startswith('@base-ui/react'): pass
                    else: missing.setdefault(root_pkg(spec), set()).add((rel, rel in deferred))
    print(f'=== {name}: {files} файлов')
    for rel, spec in sorted(bad): print('НЕТ АДРЕСАТА', rel, spec)
    print('--- пакеты, которых нет в package.json окна (кто импортирует; * — файл отложен):')
    for p, fs in sorted(missing.items()):
        listed = sorted(f + ('*' if d else '') for f, d in fs)
        print(p, '::', ', '.join(listed[:5]) + (f' и ещё {len(listed) - 5}' if len(listed) > 5 else ''))
    return bad

manifest = json.load(open(os.path.join(T3DIR, 'manifest.json')))
legacy_deferred = {f"{r}/{e['file']}" for r, es in manifest['files'].items() for e in es if isinstance(e, dict)}
bad = check('прежний перенос', T3DIR, json.load(open(os.path.join(T3DIR, 'aliases.json'))), os.path.join(T3DIR, 'web'),
            ['web', 'shared', 'client-runtime', 'shims'], legacy_deferred)
if 'base' in manifest:
    base = os.path.join(UI, manifest['base']['out'])
    checked = set(json.load(open(os.path.join(base, 'tsconfig.json')))['files'])
    aliases = json.load(open(os.path.join(base, 'aliases.json')))
    code = {os.path.relpath(os.path.join(dp, n), base) for r in ('web', 'contracts', 'shared', 'client-runtime') for dp, _, ns in os.walk(os.path.join(base, r)) for n in ns}
    bad += check('основа HOH-40', base, aliases, os.path.join(base, 'web'), ['web', 'contracts', 'shared', 'client-runtime', 'stubs', 'shims'], code - checked)
sys.exit(1 if bad else 0)
