import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const source = await readFile(new URL('../src/t3-entry/public/boot-theme.js', import.meta.url), 'utf8');
const prismId = 'hoh.midnight-prism';

function bootTheme(preferences) {
  const splash = {};
  const documentElement = {
    dataset: {},
    classList: { toggle: (name, enabled) => { if (name === 'dark') splash.dark = enabled; } },
    style: { setProperty: (name, value) => { splash[name] = value; } },
  };
  runInNewContext(source, {
    document: { documentElement, querySelectorAll: () => [] },
    window: {
      localStorage: { getItem: (key) => preferences[key] ?? null },
      matchMedia: () => ({ matches: false }),
    },
    CSS: { supports: () => true },
  });
  return { ...splash, id: documentElement.dataset.themeId, chrome: documentElement.style.backgroundColor };
}

test('Midnight Prism paints its dark splash for Light, System Light, and an unset mode', () => {
  for (const mode of ['light', 'system', null]) {
    const splash = bootTheme({
      't3code:theme': prismId,
      't3code:theme-appearance-mode': mode,
    });
    assert.equal(splash.id, prismId);
    assert.equal(splash.dark, true);
    assert.equal(splash['--boot-background'], '#080809');
    assert.equal(splash['--boot-foreground'], '#f2f3f5');
    assert.equal(splash['--boot-accent'], '#4dacf6');
    assert.equal(splash.chrome, '#080809');
  }
});

test('saved custom and cached published palettes retain their midnight-prism ID', () => {
  const palette = {
    id: 'midnight-prism',
    label: 'Midnight Prism',
    appearance: 'light',
    colors: { canvas: '#dedede' },
  };
  for (const storageKey of ['t3code:themes:v1', 't3code:environment-themes:v1']) {
    const splash = bootTheme({
      't3code:theme': palette.id,
      [storageKey]: JSON.stringify([palette]),
    });
    assert.equal(splash.id, palette.id);
    assert.equal(splash.dark, false);
    assert.equal(splash['--boot-background'], palette.colors.canvas);
  }
});

test('startup uses the mix half for the resolved appearance', () => {
  const preferences = {
    't3code:theme': 'hoh',
    't3code:theme-appearance-mode': 'light',
  };
  const lightHalf = bootTheme({
    ...preferences,
    't3code:theme-halves:v1': JSON.stringify({ light: 'grove', dark: prismId }),
  });
  assert.equal(lightHalf.id, 'grove');
  assert.equal(lightHalf.dark, false);

  const darkHalf = bootTheme({
    ...preferences,
    't3code:theme-halves:v1': JSON.stringify({ dark: prismId }),
  });
  assert.equal(darkHalf.id, prismId);
  assert.equal(darkHalf.dark, true);
  assert.equal(darkHalf['--boot-background'], '#080809');
});
