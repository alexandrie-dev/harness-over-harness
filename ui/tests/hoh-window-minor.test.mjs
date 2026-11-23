import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const root = new URL('../', import.meta.url);

test('composer command drawers override glass opacity to an opaque surface', async () => {
  const css = await readFile(new URL('src/t3-entry/brand.css', root), 'utf8');
  assert.match(css, /\[data-composer-command-drawer="true"\]\s*\{[^}]*--glass-opacity:\s*100%/s);
});
