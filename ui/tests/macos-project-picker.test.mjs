import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const pickerUrl = new URL('../src/t3-entry/macos-project-picker.ts', import.meta.url);
const pickerSource = await readFile(pickerUrl, 'utf8');
const pickerModule = ts.transpileModule(pickerSource, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText;

test('the project palette picker only uses native IPC for the local macOS environment', async () => {
  const calls = [];
  let tauri = false;
  let selection = '/Users/test/My Project';
  const modules = {
    '@tauri-apps/api/core': {
      isTauri: () => tauri,
      invoke: async (command) => {
        calls.push(command);
        return selection;
      },
    },
    '../hoh-host/config.ts': { HOH_ENVIRONMENT_ID: 'hoh-local' },
  };
  const picker = {};
  Function('exports', 'require', pickerModule)(picker, (id) => modules[id]);

  picker.configureMacProjectPicker({ os: 'darwin', arch: 'arm64' });
  assert.equal(picker.macProjectPicker('hoh-local'), undefined);

  tauri = true;
  picker.configureMacProjectPicker(undefined);
  assert.equal(picker.macProjectPicker('hoh-local'), undefined);
  picker.configureMacProjectPicker({ os: 'linux', arch: 'x64' });
  assert.equal(picker.macProjectPicker('hoh-local'), undefined);

  picker.configureMacProjectPicker({ os: 'darwin', arch: 'arm64' });
  assert.equal(picker.macProjectPicker('other-environment'), undefined);
  assert.equal(picker.macProjectPicker(null), undefined);
  assert.deepEqual(calls, []);

  const pickFolder = picker.macProjectPicker('hoh-local');
  assert.equal(await pickFolder(), '/Users/test/My Project');
  selection = null;
  assert.equal(await pickFolder(), null);
  assert.deepEqual(calls, ['pick_folder', 'pick_folder']);
});
