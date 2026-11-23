import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const root = new URL('../src/t3-entry/hoh-screens/', import.meta.url);
const [sampleSource, pageSource, sidebarSource, css] = await Promise.all([
  readFile(new URL('plugins-sample.ts', root), 'utf8'),
  readFile(new URL('PluginsPage.tsx', root), 'utf8'),
  readFile(new URL('SidebarTop.tsx', root), 'utf8'),
  readFile(new URL('plugins.css', root), 'utf8'),
]);

function loadSampleModule(source) {
  const output = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  Function('exports', output)(exports);
  return exports;
}

const { SAMPLE_PLUGINS, PLUGIN_STATE_LABELS, pluginTone } = loadSampleModule(sampleSource);

test('sample inventory is one complete seven-plugin engine-replacement fixture', () => {
  assert.equal(SAMPLE_PLUGINS.length, 7);
  assert.equal(SAMPLE_PLUGINS.filter((plugin) => plugin.builtIn).length, 3);
  assert.deepEqual(new Set(SAMPLE_PLUGINS.map((plugin) => plugin.state)), new Set(['enabled', 'quarantined', 'update-pending', 'blocked', 'disabled']));
  assert.deepEqual(new Set(SAMPLE_PLUGINS.flatMap((plugin) => plugin.languages)), new Set(['MD', 'TS', 'Rust', 'Go']));
  for (const plugin of SAMPLE_PLUGINS) {
    assert(plugin.id && plugin.name && plugin.publisher && plugin.version);
    assert(plugin.contributions.length > 0, `${plugin.id} has contributions`);
    assert(plugin.permissions.length > 0, `${plugin.id} explains permissions`);
    assert.match(plugin.fingerprint, /^sha256:[0-9a-f]{64}$/);
    assert.equal(PLUGIN_STATE_LABELS[plugin.state].length > 0, true);
    assert.equal(pluginTone(plugin.id), pluginTone(plugin.id), 'folder tone is stable by id');
    assert(pluginTone(plugin.id) >= 0 && pluginTone(plugin.id) < 6);
  }
});

test('Plugins replaces Widgets and exposes the three remembered live presentations', () => {
  assert.match(sidebarSource, /plugins:\s*PuzzleIcon/);
  assert.match(sidebarSource, /\['learning', 'plugins'\]/);
  assert.doesNotMatch(sidebarSource, /widgets/i);
  assert.match(pageSource, /hoh:plugins-design-variant/);
  assert.match(pageSource, /window\.localStorage\.setItem\(VARIANT_KEY, next\)/);
  assert.match(pageSource, /variant === 'A'/);
  assert.match(pageSource, /variant === 'B'/);
  assert.match(pageSource, /className="hoh-plugin-split"/);
  assert.match(pageSource, /aria-expanded=\{open\}/);
});

test('plugin details and mock system-approval controls cover the requested behavior', () => {
  assert.match(pageSource, /data-add-plugin-folder/);
  assert.match(pageSource, /Add plugin from folder/);
  assert.match(pageSource, /DropdownMenu/);
  assert.match(pageSource, /Copy full fingerprint/);
  assert.match(pageSource, /writeTextToClipboard/);
  assert.match(pageSource, /if \(didCopy\) setCopied\(label\)/);
  assert.match(pageSource, /aria-pressed=\{plugin\.id === selectedId\}/);
  assert.match(pageSource, /Versions and fingerprint/);
  assert.match(pageSource, /setStatuses\(\(current\) => \(\{ \.\.\.current, \[id\]: 'approval-pending' \}\)\)/);
  assert.match(pageSource, /not connected to the engine yet/);
  assert.doesNotMatch(pageSource, /__HOH_HOST__|request\(|engine\./);
});

test('plugin cards reuse the state glow language without changing home cards', () => {
  assert.match(css, /\.hoh-plugin-card\[data-state="enabled"\][^}]*--plugin-glow:\s*var\(--plugin-blue\)/);
  assert.match(css, /\.hoh-plugin-card\[data-state="quarantined"\][\s\S]*--plugin-glow:\s*var\(--plugin-red\)/);
  assert.match(css, /\.hoh-plugin-card\[data-state="disabled"\][^}]*box-shadow:\s*none/);
  assert.match(css, /\.hoh-plugin-card\[data-state="blocked"\][^}]*border-color:\s*var\(--plugin-red\)[^}]*box-shadow:\s*none/);
  assert.match(css, /grid-template-columns:\s*repeat\(auto-fill, 288px\)/);
  assert.match(css, /\.hoh-plugin-folder__front > span[^}]*border-top:/);
});
