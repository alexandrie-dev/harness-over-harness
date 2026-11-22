import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFile, readdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';

const UI = resolve(import.meta.dirname, '../..');
const DESKTOP = resolve(UI, '../apps/hoh-desktop');
async function sources(dir) {
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...await sources(path));
    else if (/\.(tsx?|mjs)$/.test(entry.name)) out.push(path);
  }
  return out;
}

test('the guard window stays a separate window the chat can neither open nor press', async () => {
  const main = JSON.parse(await readFile(join(DESKTOP, 'capabilities/main.json'), 'utf8'));
  const approval = JSON.parse(await readFile(join(DESKTOP, 'capabilities/approval.json'), 'utf8'));
  assert.deepEqual(main.windows, ['main']); assert.deepEqual(approval.windows, ['approval-*']);
  for (const right of ['allow-approval-current', 'allow-approval-decide']) assert(approval.permissions.includes(right));
  assert(!main.permissions.includes('allow-approval-current'), 'the main window has allow-approval-current');
  // HOH-40: the T3 chat answers a request shown in the conversation; the host accepts from `main` only those (native.rs decide_in_chat)
  assert(main.permissions.includes('allow-approval-decide'), 'the main window lacks allow-approval-decide');
  // no code of the chat, the first run or the shared parts talks to the guard's commands or its host object
  for (const dir of ['screens/chat', 'screens/firstrun', 'screens/shared', 'lib']) {
    for (const file of await sources(join(UI, 'src', dir))) {
      const text = await readFile(file, 'utf8');
      assert(!/approval_decide|approval_current|__HOH_APPROVAL__|approval\/bridge/.test(text), `${file} reaches the guard window`);
    }
  }
  // the guard has its own entry page and the chat page does not contain it
  const html = await readFile(join(UI, 'approval.html'), 'utf8');
  assert(/approval/i.test(html));
  const chatScreen = await readFile(join(UI, 'src/screens/chat/ChatScreen.tsx'), 'utf8');
  assert(!/ApprovalWindow/.test(chatScreen), 'the chat renders the guard itself');
});

test('the guard card is calm: red only as the small risk label', async () => {
  const css = await readFile(join(UI, 'src/screens/approval/approval.css'), 'utf8');
  const reds = css.split('\n').filter(line => /--color-wait/.test(line) && !/^\/\*/.test(line));
  assert.equal(reds.length, 1, reds.join('\n'));
  assert(/\.ap-risk\[data-risk='high'\]/.test(reds[0]));
  assert(!/ap-tape/.test(css));
});
