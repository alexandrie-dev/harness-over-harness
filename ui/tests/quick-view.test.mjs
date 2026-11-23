import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/t3-entry/hoh-screens/quick-view-store.ts', import.meta.url), 'utf8');
const dialogSource = await readFile(new URL('../src/t3-entry/hoh-screens/QuickViewDialog.tsx', import.meta.url), 'utf8');
const styles = await readFile(new URL('../src/t3-entry/hoh-screens/quick-view.css', import.meta.url), 'utf8');

function loadStore() {
  const output = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  let activeElement = null;
  const microtasks = [];
  const document = { get activeElement() { return activeElement; } };
  const exports = {};
  Function('exports', 'document', 'queueMicrotask', output)(exports, document, (task) => microtasks.push(task));
  return {
    ...exports,
    setActiveElement(value) { activeElement = value; },
    flushMicrotasks() { while (microtasks.length) microtasks.shift()(); },
  };
}

test('quick view is opened by thread id, publishes immutable snapshots, and restores its trigger after close', () => {
  const store = loadStore();
  let changes = 0;
  const unsubscribe = store.quickViewStore.subscribe(() => { changes += 1; });
  const trigger = { isConnected: true, focusCalls: 0, focus() { this.focusCalls += 1; } };
  store.setActiveElement(trigger);

  store.openQuickView('thread-17');
  const open = store.quickViewStore.getSnapshot();
  assert.equal(open.threadId, 'thread-17');
  assert.equal(open.returnFocus, trigger);
  assert(Object.isFrozen(open));
  assert.equal(store.quickViewStore.getSnapshot(), open, 'unchanged reads keep the same snapshot');
  assert.equal(changes, 1);

  store.closeQuickView();
  assert.equal(store.quickViewStore.getSnapshot().threadId, null);
  assert.equal(trigger.focusCalls, 0, 'focus waits until the dialog has closed');
  store.flushMicrotasks();
  assert.equal(trigger.focusCalls, 1);
  assert.equal(changes, 2);
  unsubscribe();
});

test('full navigation can close without refocusing, and detached triggers are never focused', () => {
  const store = loadStore();
  const trigger = { isConnected: true, focusCalls: 0, focus() { this.focusCalls += 1; } };
  store.setActiveElement(trigger);
  store.openQuickView('thread-full');
  store.closeQuickView(false);
  store.flushMicrotasks();
  assert.equal(trigger.focusCalls, 0);

  trigger.isConnected = false;
  store.openQuickView('thread-detached');
  store.closeQuickView();
  store.flushMicrotasks();
  assert.equal(trigger.focusCalls, 0);
});

test('empty ids do not open quick view or notify subscribers', () => {
  const store = loadStore();
  let changes = 0;
  store.quickViewStore.subscribe(() => { changes += 1; });
  store.openQuickView('');
  assert.equal(store.quickViewStore.getSnapshot().threadId, null);
  assert.equal(changes, 0);
});

test('quick view keeps T3 DialogPopup and ChatView while exposing state and frame variants', () => {
  assert.match(dialogSource, /<DialogPopup/);
  assert.match(dialogSource, /<ChatView/);
  assert.match(dialogSource, /data-frame-state=\{frameState\}/);
  assert.match(dialogSource, /data-frame-variant=\{frameVariant\}/);
  assert.match(dialogSource, /FRAME_VARIANT_KEY = 'hoh\.quick-view-frame'/);
  assert.match(dialogSource, /=== '2' \? '2' : '1'/, 'variant 1 remains the default');
  assert.match(dialogSource, /homeState === 'waiting'/);
  assert.match(dialogSource, /\['preparing', 'queued', 'starting', 'running'\]/);
  assert.doesNotMatch(dialogSource, /\[data-slot="sidebar-gap"\]/);
  assert.doesNotMatch(dialogSource, /--quick-view-content-inset/);
});

test('quick view restores upstream T3 lane width and gutters inside a bounded modal viewport', () => {
  assert.match(styles, /var\(--chat-content-max-width\) \+ 6rem/);
  assert.match(styles, /\.hoh-quick-view \.chat-composer-lane/);
  assert.match(styles, /\.hoh-quick-view \[data-chat-canvas\] \.messages-timeline-scroll/);
  assert.match(styles, /env\(safe-area-inset-left\) \+ 3rem/);
  assert.match(styles, /inset-block-start: var\(--hoh-notch-height, 0px\)/);
  assert.doesNotMatch(styles, /inset-inline-start/);
  assert.match(styles, /body:has\(\.hoh-quick-view\) \.hoh-notch-band \{ z-index: 40; \}/);
  assert.match(styles, /padding: 24px/);
  assert.match(styles, /padding: 8px/);
  assert.match(styles, /width: calc\(100% \+ 18px\); margin-inline: -9px/);
  assert.match(styles, /data-frame-variant="1"/);
  assert.match(styles, /data-frame-variant="2"/);
  assert.match(styles, /data-frame-state="working"/);
  assert.match(styles, /data-frame-state="waiting"/);
});
