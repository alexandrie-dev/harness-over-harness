// Check the installed catalog and plugin details against the built desktop frontend.
import assert from 'node:assert/strict';
import { chromium } from 'playwright-core';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { FakeEngine } from '../tests/chat/fake-engine.mjs';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DIST = process.env.HOH_T3_DIST ? resolve(process.env.HOH_T3_DIST) : join(UI, 'dist');
const shapes = process.argv.includes('--shapes');
const preview = shapes || process.argv.includes('--variants');
const withExamples = preview || process.argv.includes('--examples');
const output = process.argv.slice(2).find(arg => !arg.startsWith('--'));
const defaultOutput = shapes ? 'plugin-shapes' : preview ? 'plugin-variants' : withExamples ? 'plugin-reference' : 'plugins';
const OUT = resolve(output ?? join(UI, 'test-results', defaultOutput));
const DESIGN = join(UI, '..', 'design', 'plugin-variants');
const variants = shapes ? ['folder', 'ticket', 'inset'] : preview ? ['cards', 'list', 'services'] : [null];
const typography = preview ? await readFile(join(DESIGN, 'typography.css'), 'utf8') : '';
const shapeStyles = shapes ? await readFile(join(DESIGN, 'shapes.css'), 'utf8') : '';
const ORIGIN = 'http://hoh-window.test';
const SESSION = 'plugin-preview';
const SIZES = [{ width: 1440, height: 900 }, { width: 1920, height: 1080 }, { width: 800, height: 550 }]
  .filter(size => !preview || size.width !== 1920);
const TYPES = {
  '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.html': 'text/html',
  '.png': 'image/png', '.svg': 'image/svg+xml', '.webp': 'image/webp', '.woff2': 'font/woff2',
  '.woff': 'font/woff', '.ttf': 'font/ttf', '.json': 'application/json', '.wasm': 'application/wasm',
};

const manifests = await Promise.all(['turn-note', 'whisper-dictation'].map(async id =>
  JSON.parse(await readFile(join(UI, '..', 'plugins', id, 'hoh-plugin.json'), 'utf8'))));

const examplePlugins = [
  {
    id: 'github-review',
    manifest: {
      name: 'GitHub Review', version: '0.4.2',
      description: 'Собирает комментарии к pull request в GitHub и готовит сводку с помощью Claude.',
      permissions: { tools: ['review_summary'] },
    },
    status: 'configuration-error', diagnostic: 'The GitHub account needs to be connected.', data: null,
  },
  {
    id: 'notion-notes',
    manifest: {
      name: 'Заметки в Notion', version: '0.2.1',
      description: 'Сохраняет ключевые решения из разговора в базу Notion.',
      permissions: { observe: ['turn/finished'] },
    },
    status: 'disabled', diagnostic: null, data: null,
  },
];

const invalidRepositories = [
  42, 'not a URL', 'https://user:secret@github.com/owner/repo',
  'https://github.com/' + 'x'.repeat(2048), 'https://-repository.example/source', 'javascript:void(0)',
];

function sampleEngine(invalidRepository) {
  const engine = new FakeEngine();
  engine.access = 'confirm';
  engine.claude = { installed: true, version: '2.1.0', auth: 'authenticated', models: [{ id: 'opus', displayName: 'Claude Opus', isDefault: true }] };
  engine.seed(SESSION, { title: 'Plugin preview', user: 'Show the last completed turn.', assistant: 'The conversation is ready.' });
  const request = engine.request.bind(engine);
  const requestedSessions = [];
  engine.request = (method, params) => {
    if (method !== 'hoh/plugins/list') return request(method, params);
    requestedSessions.push(params.sessionId);
    const installed = manifests.map(manifest => ({
      id: manifest.id, manifest, status: 'mounted', diagnostic: null,
      data: manifest.id === 'turn-note' && params.sessionId === SESSION
        ? { turn: 3, engine: 'codex', outcome: 'completed' }
        : null,
    }));
    const [legacy, invalid] = examplePlugins;
    return Promise.resolve(withExamples ? [
      ...installed,
      legacy,
      {
        ...invalid,
        manifest: {
          ...invalid.manifest,
          repository: invalidRepository,
          panel: { title: 42, fields: [{ key: 'turn', label: 17 }] },
        },
        data: { turn: 8 },
      },
    ] : installed);
  };
  return { engine, requestedSessions };
}

async function open(browser, viewport, theme, invalidRepository) {
  const { engine, requestedSessions } = sampleEngine(invalidRepository);
  const context = await browser.newContext({ viewport, deviceScaleFactor: 1, locale: 'en-US', reducedMotion: 'reduce', colorScheme: theme });
  const page = await context.newPage();
  page.setDefaultTimeout(12000);
  const problems = [];
  page.on('pageerror', error => problems.push(`pageerror: ${error.message}`));
  page.on('console', message => { if (message.type() === 'error') problems.push(`console: ${message.text()}`); });
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) { problems.push(`remote: ${url.href}`); return route.abort(); }
    const file = extname(url.pathname) ? url.pathname : '/index.html';
    try {
      await route.fulfill({ body: await readFile(join(DIST, file)), contentType: TYPES[extname(file)] ?? 'application/octet-stream' });
    } catch {
      problems.push(`missing: ${url.pathname}`);
      await route.fulfill({ status: 404, body: '' });
    }
  });
  await page.exposeBinding('__hohT3Request', (_, method, params) =>
    engine.request(method, params).catch(error => ({ __hohError: String(error?.message ?? error) })));
  await page.addInitScript(appearance => {
    localStorage.setItem('t3code:theme', appearance === 'dark' ? 'hoh' : 'ocean');
    localStorage.setItem('t3code:theme-appearance-mode', appearance);
    const listeners = new Set();
    window.__hohT3Notify = notification => { for (const listener of listeners) listener(notification); };
    window.__HOH_T3_TEST_ENGINE__ = {
      request: async (method, params) => {
        const result = await window.__hohT3Request(method, params);
        if (result?.__hohError) throw new Error(result.__hohError);
        return result;
      },
      subscribe: async listener => { listeners.add(listener); return () => listeners.delete(listener); },
      subscribeStatus: async listener => { setTimeout(() => listener({ state: 'ready' }), 0); return () => {}; },
      subscribeQuestions: async () => () => {}, answerQuestion: async () => {}, decideApproval: async () => {},
    };
  }, theme);
  const off = engine.subscribe(notification => { page.evaluate(value => window.__hohT3Notify?.(value), notification).catch(() => {}); });
  return { context, page, problems, requestedSessions, off };
}

async function checkSourceLink(detail, id) {
  const source = detail.getByRole('link', { name: 'View on GitHub' });
  await source.locator('[data-entity="github"] svg').waitFor();
  assert.equal(await source.getAttribute('href'), manifests.find(manifest => manifest.id === id).repository);
  assert.equal(await source.getAttribute('target'), '_blank');
  assert.equal(await source.getAttribute('rel'), 'noopener noreferrer');
  return source;
}

async function checkCardAlignment(catalog) {
  const layouts = await catalog.locator('.hoh-plugin-card').evaluateAll(cards => cards.map(card => {
    const box = card.getBoundingClientRect();
    const relativeTop = selector => card.querySelector(selector).getBoundingClientRect().top - box.top;
    return {
      width: box.width,
      height: box.height,
      logo: relativeTop('.hoh-plugin-artwork'),
      title: relativeTop('strong'),
      description: relativeTop('.hoh-plugin-card__summary'),
      status: relativeTop('.hoh-plugin-status'),
      version: relativeTop('.hoh-plugin-version'),
    };
  }));
  const reference = layouts[0];
  for (const layout of layouts) {
    for (const [key, offset] of Object.entries(layout)) {
      assert(Math.abs(offset - reference[key]) <= 1, `card ${key} misaligned: ${offset} vs ${reference[key]}`);
    }
  }
}

async function checkPuzzleEdges(page, catalog, card) {
  const joint = await catalog.evaluate(grid => {
    const layout = getComputedStyle(grid);
    const columns = layout.gridTemplateColumns.split(' ').length;
    const tile = grid.firstElementChild;
    return {
      gap: layout.gap,
      display: getComputedStyle(tile, columns === 2 ? '::before' : '::after').display,
    };
  });
  assert.equal(joint.gap, '5px');
  assert.equal(joint.display, 'block');

  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await card.hover();
  assert.equal(await card.evaluate(node => getComputedStyle(node, '::before').animationName), 'hoh-plugin-edge');
  const moved = await card.evaluate(async node => {
    const angle = () => getComputedStyle(node, '::before').getPropertyValue('--plugin-edge-angle');
    const before = angle();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return before !== angle();
  });
  assert(moved, 'the edge highlight should move along the border');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  assert.equal(await card.evaluate(node => getComputedStyle(node, '::before').animationName), 'none');
  await page.mouse.move(0, 0);
}

async function snapshot(page, name) {
  await page.evaluate(() => document.fonts.ready);
  const overflow = await page.locator('.hoh-plugins').evaluate(node => node.scrollWidth > node.clientWidth + 1);
  assert(!overflow, 'plugin page has horizontal overflow');
  const overflowing = await page.locator('.hoh-plugin-card, .hoh-plugin-info').evaluateAll(nodes =>
    nodes.filter(node => node.scrollWidth > node.clientWidth + 1).map(node => node.textContent.trim()));
  assert.deepEqual(overflowing, [], 'plugin card or information section has horizontal overflow');
  await page.screenshot({ path: join(OUT, `${name}.png`), animations: 'disabled' });
}

await mkdir(OUT, { recursive: true });
const browserHome = await mkdtemp(join(tmpdir(), 'hoh-plugin-chromium-'));
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome',
    headless: true,
    env: { ...process.env, HOME: browserHome, XDG_CONFIG_HOME: join(browserHome, 'config'), XDG_CACHE_HOME: join(browserHome, 'cache') },
    args: ['--no-sandbox', '--disable-gpu', '--disable-audio-output', '--mute-audio', '--disable-background-networking', '--disable-component-update', '--no-first-run', '--force-color-profile=srgb'],
  });
  for (const variant of variants) {
    for (const theme of ['light', 'dark']) {
      for (const size of SIZES) {
        const label = [variant, theme, `${size.width}x${size.height}`].filter(Boolean).join('-');
        const invalidRepository = invalidRepositories[SIZES.indexOf(size) + (theme === 'dark' ? SIZES.length : 0)];
        const { context, page, problems, requestedSessions, off } = await open(browser, size, theme, invalidRepository);
        try {
          await page.goto(`${ORIGIN}/hoh-local/${SESSION}`);
          if (variant) {
            await page.locator('html').evaluate((node, { variant, shapes }) => {
              node.dataset.pluginPreview = variant;
              if (shapes) node.dataset.pluginShape = variant;
            }, { variant, shapes });
            await page.addStyleTag({ content: typography });
            if (shapes) await page.addStyleTag({ content: shapeStyles });
          }
          await page.locator('[data-hoh-page-item="plugins"]').click();
          const catalog = page.locator('[data-plugin-list]');
          const turn = catalog.locator('[data-plugin-id="turn-note"]');
          const whisper = catalog.locator('[data-plugin-id="whisper-dictation"]');
          await turn.waitFor();
          assert.equal(await page.locator('html').evaluate(node => node.classList.contains('dark')), theme === 'dark');
          await whisper.locator('.hoh-plugin-artwork [data-entity="openai"] svg').waitFor();
          await whisper.locator('.hoh-plugin-card__summary [data-entity="openai"]').waitFor();
          assert.equal(await catalog.locator('.hoh-plugin-card').count(), withExamples ? 4 : 2);
          assert.equal(requestedSessions[0], SESSION);
          if (shapes || !preview) {
            await page.evaluate(() => document.fonts.ready);
            await checkCardAlignment(catalog);
          }
          await snapshot(page, `plugins-list-${label}`);
          if (!preview) await checkPuzzleEdges(page, catalog, turn);

          const search = page.getByRole('searchbox', { name: 'Search installed plugins' });
          await search.fill('whisper');
          await turn.waitFor({ state: 'detached' });
          await search.fill('no match');
          await page.getByRole('heading', { name: 'No matching plugins' }).waitFor();
          await page.getByRole('button', { name: 'Clear search' }).click();
          await turn.waitFor();
          assert(await search.evaluate(node => node === document.activeElement));

          if (preview) {
            await turn.focus();
          } else {
            await page.emulateMedia({ reducedMotion: 'no-preference' });
            await search.press('Tab');
            assert(await turn.evaluate(node => node.matches(':focus-visible')));
            assert.equal(await turn.evaluate(node => getComputedStyle(node, '::before').animationName), 'hoh-plugin-edge');
            await page.emulateMedia({ reducedMotion: 'reduce' });
          }
          await page.keyboard.press('Enter');
          const detail = page.locator('[data-plugin-detail="turn-note"]');
          const back = detail.getByRole('button', { name: 'All plugins' });
          const overview = detail.getByRole('tab', { name: 'Overview' });
          const session = detail.getByRole('tab', { name: 'Session data' });
          await detail.getByText('last_turn', { exact: true }).waitFor();
          assert.equal(await overview.getAttribute('aria-selected'), 'true');
          assert(await back.evaluate(node => node === document.activeElement));
          await snapshot(page, `plugins-detail-${label}`);
          const source = await checkSourceLink(detail, 'turn-note');
          const sourceUrl = await source.getAttribute('href');
          const originalUrl = page.url();
          await context.route(sourceUrl, route => route.fulfill({ body: 'Plugin source fixture' }));
          const [sourcePage] = await Promise.all([context.waitForEvent('page'), source.click()]);
          await sourcePage.waitForURL(sourceUrl);
          await sourcePage.close();
          assert.equal(page.url(), originalUrl);
          assert(await detail.isVisible());

          await overview.focus();
          await page.keyboard.press('ArrowRight');
          await detail.getByRole('tabpanel').getByText('completed', { exact: true }).waitFor();
          assert.equal(await session.getAttribute('aria-selected'), 'true');
          await detail.getByRole('tabpanel').locator('[data-entity="openai"]').waitFor();
          await snapshot(page, `plugins-session-${label}`);

          await back.press('Enter');
          await turn.waitFor();
          assert(await turn.evaluate(node => node === document.activeElement));
          await whisper.click();
          const whisperDetail = page.locator('[data-plugin-detail="whisper-dictation"]');
          await whisperDetail.getByText('No permissions declared.').waitFor();
          assert.equal(await whisperDetail.getByRole('tab').count(), 1);
          await checkSourceLink(whisperDetail, 'whisper-dictation');
          if (withExamples) {
            await whisperDetail.getByRole('button', { name: 'All plugins' }).click();
            for (const id of examplePlugins.map(plugin => plugin.id)) {
              await catalog.locator(`[data-plugin-id="${id}"]`).click();
              const example = page.locator(`[data-plugin-detail="${id}"]`);
              await example.waitFor();
              assert.equal(await example.locator('.hoh-plugin-source').count(), 0);
              if (id === 'notion-notes') {
                await example.getByRole('tab', { name: 'Session data' }).click();
                const panel = example.getByRole('tabpanel');
                await panel.getByRole('heading', { name: '42' }).waitFor();
                await panel.getByText('17', { exact: true }).waitFor();
              }
              await example.getByRole('button', { name: 'All plugins' }).click();
            }
          }
          assert.deepEqual(problems, []);
          console.log(`PASS ${label}`);
        } catch (error) {
          await page.screenshot({ path: join(OUT, `plugins-failure-${label}.png`) }).catch(() => {});
          throw error;
        } finally {
          off();
          await context.close();
        }
      }
    }
  }
  if (preview) {
    const gallery = await readFile(join(DESIGN, 'index.html'), 'utf8');
    await writeFile(join(OUT, 'index.html'), shapes ? gallery.replace('data-round="typography"', 'data-round="shapes"') : gallery);
  }
} finally {
  await browser?.close();
  await rm(browserHome, { recursive: true, force: true });
}
