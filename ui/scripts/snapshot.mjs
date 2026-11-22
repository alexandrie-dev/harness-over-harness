import { chromium } from 'playwright-core';
import { readFile, mkdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { tmpdir } from 'node:os';
import { mkdtemp, rm } from 'node:fs/promises';
import assert from 'node:assert/strict';

// Прежняя оболочка — dist/legacy.html (HOH-40: dist/index.html — веб-клиент T3).
const indexUrl = pathToFileURL(resolve('dist/legacy.html')).href;
const probeUrl = pathToFileURL(resolve('dist/probe.html')).href;
const home = await mkdtemp(join(tmpdir(), 'hoh-chromium-'));
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome',
    headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
    args: ['--allow-file-access-from-files', '--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--disable-domain-reliability', '--no-first-run', '--disable-default-apps', '--force-color-profile=srgb'],
  });
  const page = await browser.newPage({ viewport: { width: 1200, height: 800 }, deviceScaleFactor: 1, reducedMotion: 'reduce', locale: 'ru-RU' });
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.route('**/*', route => route.request().url().startsWith(pathToFileURL(resolve('dist') + '/').href) ? route.continue() : route.abort());
  await page.goto(indexUrl);
  await page.evaluate(() => document.fonts.ready);
  await page.getByRole('heading', { name: 'С чего начнём?' }).waitFor();
  assert.deepEqual(errors, []);
  await mkdir('test-results', { recursive: true });
  const screenshot = await page.screenshot({ path: 'test-results/shell.png', animations: 'disabled' });
  const reference = 'tests/reference/shell.png';
  if (process.argv.includes('--update-reference')) {
    await mkdir('tests/reference', { recursive: true });
    await page.screenshot({ path: reference, animations: 'disabled' });
    console.log('created reference: ' + reference);
  } else {
    assert(screenshot.equals(await readFile(reference)), 'shell differs from committed reference; see test-results/shell.png');
    console.log('passed: shell snapshot exactly matches reference');
  }
  await page.getByRole('button', { name: 'Задачи', exact: true }).click();
  await page.getByRole('heading', { name: 'Место для ваших задач' }).waitFor();
  await page.getByRole('button', { name: 'Светлая тема' }).click();
  assert.equal(await page.locator('html').getAttribute('data-theme'), 'light');
  await page.goto(probeUrl);
  await page.waitForFunction(() => document.title.startsWith('HOH_PROBE:'));
  const probe = JSON.parse((await page.title()).slice('HOH_PROBE:'.length));
  assert.equal(probe.pass, true);
  console.log('passed: Chromium CSS probe, navigation and theme toggle');
} finally {
  await browser?.close();
  await rm(home, { recursive: true, force: true });
}
