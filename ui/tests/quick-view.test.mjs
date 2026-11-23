import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/t3-entry/hoh-screens/quick-view-store.ts', import.meta.url), 'utf8');

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
