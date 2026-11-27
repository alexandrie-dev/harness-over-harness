import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';
import { createStore } from 'zustand/vanilla';

const source = await readFile(new URL('../src/t3-entry/desktop-update.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText;

test('desktop checks at startup and on the timer, and installs only on request', async () => {
  const commands = [];
  const timers = [];
  let tauri = false;
  let answerCheck;
  const available = { kind: 'available', version: '0.2.0' };
  const modules = {
    '@tauri-apps/api/core': {
      isTauri: () => tauri,
      Channel: class Channel {
        constructor(onmessage) { this.onmessage = onmessage; }
      },
      invoke(command, args) {
        commands.push(command);
        if (command === 'check_for_update') return new Promise(resolve => { answerCheck = resolve; });
        assert.equal(command, 'auto_update');
        assert.equal(args.expectedVersion, '0.2.0');
        args.onProgress.onmessage({ kind: 'downloading', version: '0.2.0' });
        return Promise.resolve({ kind: 'restarting', version: '0.2.0' });
      },
    },
    'zustand/vanilla': { createStore },
  };
  const controls = {};
  Function('exports', 'require', 'window', compiled)(controls, id => modules[id], {
    setInterval: (callback, delay) => timers.push({ callback, delay }),
  });

  controls.startUpdateChecks();
  assert.deepEqual(commands, []);
  assert.deepEqual(timers, []);

  tauri = true;
  controls.startUpdateChecks();
  assert.deepEqual(commands, ['check_for_update']);
  assert.deepEqual(controls.desktopUpdate.getState(), { kind: 'checking' });
  await controls.installUpdate();
  assert.deepEqual(commands, ['check_for_update']);

  answerCheck(available);
  await Promise.resolve();
  assert.deepEqual(controls.desktopUpdate.getState(), available);
  assert.equal(timers.length, 1);
  assert.equal(timers[0].delay, 6 * 60 * 60 * 1000);
  timers[0].callback();
  answerCheck(available);
  await Promise.resolve();
  assert.deepEqual(commands, ['check_for_update', 'check_for_update']);

  await controls.installUpdate();
  assert.deepEqual(commands, ['check_for_update', 'check_for_update', 'auto_update']);
  assert.deepEqual(controls.desktopUpdate.getState(), { kind: 'restarting', version: '0.2.0' });
});
