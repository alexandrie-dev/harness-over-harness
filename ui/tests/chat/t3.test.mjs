import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawnSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const UI = fileURLToPath(new URL('../../', import.meta.url));
test('code taken from T3: every import has a target, and nothing outside the window packages is imported by compiled files', () => {
  const result = spawnSync('/usr/bin/python3', ['-I', 'scripts/t3-check.py'], { cwd: UI, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stdout + result.stderr);
});
test('the chat takes its answer markup only from the T3 switch module', async () => {
  const chat = await readFile(new URL('../../src/screens/chat/ChatScreen.tsx', import.meta.url), 'utf8');
  assert(chat.includes("from '../../t3/render'") && !chat.includes("lib/markdown'"));
});
test('NOTICE carries the full MIT text and the T3 copyright', async () => {
  const notice = await readFile(new URL('../../NOTICE.md', import.meta.url), 'utf8');
  assert(notice.includes('Copyright (c) 2026 T3 Tools Inc.') && notice.includes('Permission is hereby granted, free of charge') && notice.includes('WITHOUT WARRANTY OF ANY KIND'));
});
