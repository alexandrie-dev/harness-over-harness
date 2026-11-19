import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
const read = path => readFile(new URL(path, import.meta.url), 'utf8');

test('chat components contain no literal palette colors or inline visual styles', async () => {
  for (const path of ['../../src/screens/chat/ChatScreen.tsx', '../../src/screens/chat/icons.tsx', '../../src/screens/chat/chat.css']) {
    const source = await read(path);
    assert(!/#(?:[0-9a-f]{3,8})\b|\b(?:rgba?|hsla?|oklch|oklab)\s*\(/i.test(source), path);
    assert(!/style\s*=\s*\{/.test(source), path);
  }
});
test('chat CSS has no fixed length literals outside shared tokens.css', async () => {
  const source = await read('../../src/screens/chat/chat.css');
  assert(!/\b\d+(?:\.\d+)?(?:px|rem|em|dvh|vh|vw)\b/.test(source));
});
test('every referenced chat CSS variable is defined in shared tokens.css', async () => {
  const css = await read('../../src/screens/chat/chat.css');
  const definitions = await read('../../src/styles/tokens.css');
  for (const match of css.matchAll(/var\((--[\w-]+)/g)) assert(definitions.includes(match[1] + ':'), match[1]);
});
