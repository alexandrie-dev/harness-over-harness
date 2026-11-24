import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import vm from 'node:vm';

const require = createRequire(import.meta.url);
const ts = require('typescript');
const reactRouter = require.resolve('@tanstack/react-router');
const coreEntry = createRequire(reactRouter).resolve('@tanstack/router-core');
const routerSource = await readFile(resolve(dirname(coreEntry), '../esm/scroll-restoration.js'), 'utf8');
const patchSource = await readFile(new URL('../../src/t3-entry/router-scroll.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(patchSource, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
const { deferRouterScrollStorage } = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);

function loadRouter({ broken = false } = {}) {
  let reads = 0;
  const writes = [];
  const listeners = {};
  const storage = {
    getItem(key) {
      assert.equal(key, 'tsr-scroll-restoration-v1_3');
      return JSON.stringify({ '/chat': { window: { scrollX: 4, scrollY: 42 } } });
    },
    setItem(key, value) { writes.push([key, value]); },
  };
  const context = {
    isServer: false,
    locationHistoryActions: new WeakMap(),
    history: {},
    window: {},
    document: { addEventListener() {}, querySelector() {} },
    addEventListener(name, callback) { listeners[name] = callback; },
    console,
  };
  Object.defineProperty(context, 'sessionStorage', { get() { reads++; if (broken) throw Error('unavailable'); return storage; } });
  const moduleBody = deferRouterScrollStorage(routerSource)
    .replace(/^import .*;\s*$/gm, '')
    .replace(/^export \{.*\};\s*$/m, '')
    + '\n;globalThis.hohTest = { setupScrollRestoration, getElementScrollRestorationEntry };';
  vm.runInNewContext(moduleBody, context);
  const router = {
    options: {}, latestLocation: { href: '/chat', state: {} },
    subscribe() {}, stores: { resolvedLocation: { get() {} }, location: { get() { return { href: '/chat', state: {} }; } } },
  };
  return { context, router, listeners, writes, get reads() { return reads; } };
}

test('disabled router does not touch sessionStorage at import, setup or pagehide', () => {
  const run = loadRouter();
  assert.equal(run.reads, 0);
  run.context.hohTest.setupScrollRestoration(run.router);
  run.listeners.pagehide();
  assert.equal(run.reads, 0);
  assert.deepEqual(run.writes, []);
});

test('enabled router loads its saved cache once and persists it on pagehide', () => {
  const run = loadRouter();
  run.router.options.scrollRestoration = true;
  run.context.hohTest.setupScrollRestoration(run.router);
  assert.equal(run.reads, 1);
  const entry = run.context.hohTest.getElementScrollRestorationEntry(run.router, { getElement: () => run.context.window });
  assert.equal(entry.scrollX, 4);
  assert.equal(entry.scrollY, 42);
  assert.equal(run.reads, 1);
  run.listeners.pagehide();
  assert.equal(run.writes.length, 1);
  assert.equal(JSON.parse(run.writes[0][1])['/chat'].window.scrollY, 42);
});

test('explicit restoration hook initializes storage even if router restoration is disabled', () => {
  const run = loadRouter();
  const entry = run.context.hohTest.getElementScrollRestorationEntry(run.router, { getElement: () => run.context.window });
  assert.equal(entry.scrollY, 42);
  assert.equal(run.reads, 1);
});

test('unavailable storage remains harmless and changed dependency fails the patch', () => {
  const run = loadRouter({ broken: true });
  run.router.options.scrollRestoration = true;
  assert.doesNotThrow(() => run.context.hohTest.setupScrollRestoration(run.router));
  assert.equal(run.reads, 1);
  assert.throws(() => deferRouterScrollStorage('changed dependency'), /expected exactly one/);
});
