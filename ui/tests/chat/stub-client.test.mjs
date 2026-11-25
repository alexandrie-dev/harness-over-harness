import assert from 'node:assert/strict';
import test from 'node:test';

import { DEFAULT_STUB, requireStub } from './stub-client.mjs';

test('chat replay uses the shared NVMe Claude-line stub by default', () => {
  assert.equal(DEFAULT_STUB, '/home/save/.cache/hoh-build/line-claude/debug/hoh-window-stub');
});

test('a missing replay stub fails before spawn with an actionable path and override', () => {
  const missing = '/tmp/hoh-window-stub-definitely-missing-cx-mn2';
  assert.throws(
    () => requireStub(missing),
    error => error instanceof Error
      && error.message.includes(`hoh-window-stub was not found at ${missing}`)
      && error.message.includes('HOH_WINDOW_STUB'),
  );
});
