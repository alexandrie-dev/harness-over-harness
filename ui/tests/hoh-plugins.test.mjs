import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const root = new URL('../src/t3-entry/hoh-screens/', import.meta.url);
const [pageSource, sidebarSource, css] = await Promise.all([
  readFile(new URL('PluginsPage.tsx', root), 'utf8'),
  readFile(new URL('SidebarTop.tsx', root), 'utf8'),
  readFile(new URL('plugins.css', root), 'utf8'),
]);

test('Plugins replaces Widgets and contains no sample-data or design-round controls', () => {
  assert.match(sidebarSource, /plugins:\s*PuzzleIcon/);
  assert.match(sidebarSource, /\['learning', 'plugins'\]/);
  assert.doesNotMatch(sidebarSource, /widgets/i);
  assert.doesNotMatch(pageSource, /Sample data|design round|design-round|Variant [ABC]|data-plugin-variant|plugins-design-variant/);
  assert.doesNotMatch(css, /hoh-plugins__sample|hoh-plugins__round|hoh-plugin-split|hoh-plugin-master|hoh-plugin-row/);
});

test('no engine inventory produces an honest install-oriented empty state', () => {
  assert.match(pageSource, /export const INSTALLED_PLUGINS: readonly Plugin\[\] = \[\]/);
  assert.match(pageSource, /data-plugin-empty/);
  assert.match(pageSource, /No plugins installed/);
  assert.match(pageSource, /added from a local folder once plugin installation is available/);
  assert.match(pageSource, /How to install a plugin/);
  assert.match(pageSource, /Plugin installation is not connected to the HoH engine yet/);
  assert.doesNotMatch(pageSource, /alexandrie-orbit-tools|northlight-draftsmith|SAMPLE_PLUGINS/);
});

test('the retained list is card variant A and its destination is the variant-C detail presentation', () => {
  assert.match(pageSource, /function CardGrid/);
  assert.match(pageSource, /className="hoh-plugin-card"/);
  assert.match(pageSource, /function PluginDetail/);
  assert.match(pageSource, /The retained variant-C detail presentation, opened from the variant-A card list/);
  assert.match(pageSource, /selected \? <PluginDetail/);
  assert.match(pageSource, /<CardGrid plugins=\{plugins\} onOpen=\{openPlugin\}/);
  assert.match(pageSource, /Versions and fingerprint/);
  assert.match(pageSource, /Copy full fingerprint/);
});

test('plugin cards retain the state glow language and empty state has a contained T3-style surface', () => {
  assert.match(css, /\.hoh-plugin-card\[data-state="enabled"\][^}]*--plugin-glow:\s*var\(--plugin-blue\)/);
  assert.match(css, /\.hoh-plugin-card\[data-state="quarantined"\][\s\S]*--plugin-glow:\s*var\(--plugin-red\)/);
  assert.match(css, /\.hoh-plugin-card\[data-state="disabled"\][^}]*box-shadow:\s*none/);
  assert.match(css, /grid-template-columns:\s*repeat\(auto-fill, 288px\)/);
  assert.match(css, /\.hoh-plugins-empty\s*\{[^}]*border:\s*1px dashed var\(--border\)[^}]*text-align:\s*center/);
});
