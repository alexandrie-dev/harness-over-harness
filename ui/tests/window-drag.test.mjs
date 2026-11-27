import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadConfigFromFile } from 'vite';

const ui = fileURLToPath(new URL('../', import.meta.url));

test('the desktop entry marks its existing titlebars as native drag regions', async () => {
  const config = await loadConfigFromFile(
    { command: 'build', mode: 't3' },
    join(ui, 'vite.config.ts'),
    ui,
    'silent',
  );
  const plugin = config?.config.plugins?.find(plugin => plugin?.name === 'hoh-t3-entry');
  assert.equal(typeof plugin?.transform, 'function');

  const transform = async file => {
    const path = join(ui, 'src/t3/base/web', file);
    const patched = await plugin.transform(await readFile(path, 'utf8'), path);
    assert.equal(typeof patched?.code, 'string', file);
    return patched.code;
  };

  for (const file of [
    'components/ChatView.tsx',
    'components/sidebar/SidebarChrome.tsx',
    'components/WorkspacePageHeader.tsx',
    'routes/_chat.settings.tsx',
  ]) {
    assert.match(await transform(file), /data-tauri-drag-region="deep"/, file);
  }
  for (const file of [
    'components/onboarding/WelcomePage.tsx',
    'components/NoProjectsHero.tsx',
    'routes/_chat.index.tsx',
  ]) {
    assert.match(await transform(file), /isElectron \|\| isTauri\(\)\) && <WorkspacePageHeader/, file);
  }
  const rightPanel = await transform('components/RightPanelTabs.tsx');
  assert.match(rightPanel, /data-right-panel-tabbar\s+data-tauri-drag-region=\{props.mode === "inline" \? "deep"/);
  assert.match(rightPanel, /data-right-panel-tab-list\s+data-tauri-drag-region=\{tabScrollState.hasOverflow \? "false"/);
});

test('the main window alone can start native dragging and handle a titlebar double-click', async () => {
  const capabilities = new URL('../../apps/hoh-desktop/capabilities/', import.meta.url);
  const main = JSON.parse(await readFile(new URL('main.json', capabilities), 'utf8'));
  const approval = JSON.parse(await readFile(new URL('approval.json', capabilities), 'utf8'));

  assert.deepEqual(main.windows, ['main']);
  for (const permission of [
    'core:window:allow-start-dragging',
    'core:window:allow-internal-toggle-maximize',
  ]) {
    assert(main.permissions.includes(permission), permission);
    assert(!approval.permissions.includes(permission), permission);
  }
});
