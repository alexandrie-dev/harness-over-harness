import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const root = new URL('../', import.meta.url);
const base = path => new URL(`src/t3/base/${path}`, root);

test('an explicit empty provider mode list stays empty while undefined keeps T3 legacy modes', async () => {
  const source = await readFile(base('web/components/chat/runtimeModeConfig.ts'), 'utf8');
  const resolver = source.match(
    /export function resolveCompatibleRuntimeMode\([\s\S]*?\n}\n(?=\nexport function resolveRuntimeModeOption)/,
  )?.[0];
  assert(resolver, 'the patched runtime mode resolver must be present');
  const unit = `const runtimeModeOptions = ["approval-required", "auto-accept-edits", "auto", "full-access"];\n${resolver}`;
  const js = ts.transpileModule(unit, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  new Function('exports', js)(exports);
  const resolve = exports.resolveCompatibleRuntimeMode;
  assert.deepEqual(resolve('auto', undefined), {
    modes: ['approval-required', 'auto-accept-edits', 'auto', 'full-access'], mode: 'auto',
  });
  assert.deepEqual(resolve('auto', []), { modes: [], mode: 'auto' });
  assert.deepEqual(resolve('full-access', ['approval-required', 'auto']), {
    modes: ['approval-required', 'auto'], mode: 'approval-required',
  });
});

test('every real permissions surface renders or owns nothing for an explicit empty list', async () => {
  const [controls, footer, context, manifestText, map] = await Promise.all([
    readFile(base('web/components/chat/ComposerControlsBar.tsx'), 'utf8'),
    readFile(base('web/components/chat/ComposerFooterControls.tsx'), 'utf8'),
    readFile(base('web/components/ComposerContextPermissions.tsx'), 'utf8'),
    readFile(new URL('src/t3/manifest.json', root), 'utf8'),
    readFile(new URL('T3-MAP.md', root), 'utf8'),
  ]);
  assert.match(controls, /runtimeModesAvailable = compatibleRuntimeModeOptions\.length > 0/);
  assert.match(controls, /runtimeModesAvailable && pickerOwnsModeShortcut/);
  assert.match(controls, /traitsInput \|\| runtimeModesAvailable \? \(/);
  assert.equal((footer.match(/runtimeModeOptions\.length === 0\) return null/g) ?? []).length, 2);
  assert.match(context, /if \(modes\.length === 0\) return null/);
  for (const [name, source] of [['ComposerControlsBar.tsx', controls], ['ComposerFooterControls.tsx', footer], ['ComposerContextPermissions.tsx', context]]) {
    const { diagnostics } = ts.transpileModule(source, {
      fileName: name,
      reportDiagnostics: true,
      compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
    });
    assert.deepEqual(diagnostics ?? [], [], `${name} must remain valid TSX after its base patch`);
  }

  const patched = JSON.parse(manifestText).base.patched;
  for (const path of [
    'web/components/chat/runtimeModeConfig.ts',
    'web/components/chat/ComposerControlsBar.tsx',
    'web/components/chat/ComposerFooterControls.tsx',
    'web/components/ComposerContextPermissions.tsx',
  ]) assert(patched[path], `${path} must remain a declared T3 patch`);
  assert.match(map, /runtimeModeConfig\.ts\.patch[^\n]*явный `\[\]`/);
});
