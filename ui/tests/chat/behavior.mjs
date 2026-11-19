// Headless behavior checks plus the requested A review image; no HTTP server.
import assert from 'node:assert/strict';
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { StubClient, RestartingStubHost, recording, recordedPrompt } from './stub-client.mjs';
import { pathCases, technicalCases } from './privacy-cases.mjs';
import { harmfulMarkdownCases } from './markdown-cases.mjs';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const RESULTS = join(UI, 'test-results/chat');
const INDEX = join(UI, 'dist/index.html');
await mkdir(RESULTS, { recursive: true });
let chromium;
const missing = [];
try { ({ chromium } = await import('playwright-core')); } catch { missing.push('playwright-core@1.65.0 is not installed'); }
try { await access(INDEX); } catch { missing.push('ui/dist/index.html has not been built'); }
if (missing.length) {
  await writeFile(join(RESULTS, 'behavior.json'), JSON.stringify({ status: 'blocked', missing, passed: 0 }, null, 2) + '\n');
  console.error('BLOCKED: ' + missing.join('; '));
  process.exit(2);
}
const home = await mkdtemp(join(tmpdir(), 'hoh-chat-chromium-'));
const results = [];
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
    args: ['--allow-file-access-from-files', '--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run', '--disable-default-apps', '--force-color-profile=srgb'],
  });
  async function check(name, scenario, callback, extra = [], options = {}) {
    const stub = options.restart ? await RestartingStubHost.open() : await StubClient.open(scenario, extra);
    const context = await browser.newContext({ viewport: options.viewport ?? { width: 1200, height: 800 }, locale: 'ru-RU', reducedMotion: 'reduce' });
    await context.tracing.start({ screenshots: false, snapshots: false, sources: false });
    const page = await context.newPage();
    page.setDefaultTimeout(15000);
    const errors = []; const remote = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/*', route => {
      if (route.request().url().startsWith(pathToFileURL(join(UI, 'dist') + '/').href)) return route.continue();
      remote.push(route.request().url()); return route.abort();
    });
    await page.exposeBinding('__hohChatRequest', (_, method, params) => stub.request(method, params));
    await page.addInitScript(() => {
      const listeners = new Set(); const statusListeners = new Set();
      window.__HOH_CHAT_TEST_TRANSPORT__ = {
        request: (method, params) => window.__hohChatRequest(method, params),
        subscribe: async listener => { listeners.add(listener); return () => listeners.delete(listener); },
        subscribeStatus: async listener => { statusListeners.add(listener); return () => statusListeners.delete(listener); },
      };
      window.__hohChatNotify = notification => { for (const listener of listeners) listener(notification); };
      window.__hohChatEngineStatus = status => { for (const listener of statusListeners) listener(status); };
      window.__chatTextSamples = [];
      document.addEventListener('DOMContentLoaded', () => {
        new MutationObserver(() => {
          const value = document.querySelector('[data-chat-entry="assistant"] [data-markdown]')?.textContent;
          if (value && window.__chatTextSamples.at(-1) !== value) window.__chatTextSamples.push(value);
        }).observe(document.body, { subtree: true, childList: true, characterData: true });
      });
    });
    await page.addInitScript(({ skip }) => {
      if (skip) localStorage.setItem('hoh:first-run:done', '1');
    }, { skip: !options.firstRun });
    if (options.firstRun) {
      await page.exposeBinding('__hohWizardRequest', (_, method) => {
        if (method === 'hoh/settings/get') return { model: { baseUrl: 'http://localhost:8000/v1', model: 'local' } };
        if (method === 'hoh/model/probe') return { ok: true, latencyMs: 20, toolCalls: true, models: ['local'] };
        if (method === 'hoh/settings/set') return {};
        throw new Error('Unsupported wizard test method');
      });
      await page.addInitScript(() => {
        window.__HOH_ENGINE__ = { request: (m, p) => window.__hohWizardRequest(m, p), saveRules: async () => {} };
      });
    }
    const deliveries = [];
    const off = stub.subscribe(notification => {
      // No synthesized notifications: every frame is emitted by the Rust stub.
      const delivery = page.evaluate(value => window.__hohChatNotify(value), notification);
      deliveries.push(delivery);
      delivery.catch(() => {}); // Asserted below before the context is closed.
    });
    const offStatus = stub.subscribeStatus?.(status => {
      const delivery = page.evaluate(value => window.__hohChatEngineStatus(value), status);
      deliveries.push(delivery); delivery.catch(() => {});
    });
    try {
      await page.goto(pathToFileURL(INDEX).href);
      if (options.firstRun) await page.getByRole('heading', { name: 'Подключите модель' }).waitFor();
      else await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
      await callback(page, stub);
      await Promise.all(deliveries);
      assert.deepEqual(errors, [], 'renderer exceptions'); assert.deepEqual(remote, [], 'unexpected resource loads');
      assert.equal(await page.locator('[data-chat-sidebar]').count(), 1, 'A sidebar contains only the current assistant');
      assert.equal(await page.getByRole('link', { name: /Сотрудники|Задачи|Расписание|Результаты/ }).count(), 0);
      if (options.shot) {
        await page.evaluate(() => document.fonts.ready);
        await page.mouse.move(0, 0);
        const shots = resolve(UI, '../design/build-a'); await mkdir(shots, { recursive: true });
        await page.screenshot({ path: join(shots, options.shot), animations: 'disabled' });
        await page.evaluate(() => { document.documentElement.dataset.theme = 'light'; });
        assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).colorScheme), 'light');
        await page.screenshot({ path: join(shots, 'chat-1440-light.png'), animations: 'disabled' });
      }
      results.push({ name, status: 'passed' }); console.log('PASS ' + name);
    } catch (error) {
      results.push({ name, status: 'failed', error: String(error) }); throw error;
    } finally {
      off(); offStatus?.(); await context.tracing.stop({ path: join(RESULTS, name + '.trace.zip') });
      await context.close(); await stub.close();
    }
  }

  await check('first-run-to-chat', 'text-turn', async (page, stub) => {
    await page.getByRole('button', { name: 'Проверить', exact: true }).click();
    await page.getByText(/Отвечает за/).waitFor();
    await page.getByRole('button', { name: 'Дальше', exact: true }).click();
    await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
    await page.getByRole('button', { name: 'Дальше', exact: true }).click();
    await page.getByRole('button', { name: /Прочитать документ/ }).click();
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
    assert.equal(await page.getByLabel('Сообщение Помощнику', { exact: true }).inputValue(), 'Прочитай документ в папке и перескажи в трёх строках');
    assert.equal(stub.requests.filter(r => r.method === 'session/prompt').length, 0, 'onFinish never starts work without Send');
    assert.equal(await page.evaluate(() => localStorage.getItem('hoh:first-run:done')), '1');
    await page.reload();
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
  }, [], { firstRun: true });

  await check('text-stream', 'text-turn', async (page, stub) => {
    const prompt = await recordedPrompt('text-turn');
    await page.getByLabel('Сообщение Помощнику', { exact: true }).fill(prompt);
    await page.getByRole('button', { name: 'Отправить', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('[data-chat-entry="assistant"] [data-markdown]')?.textContent === 'SDK snapshot OK');
    await page.waitForFunction(() => document.querySelector('[data-chat-status]')?.textContent === 'Можно написать сообщение');
    const samples = await page.evaluate(() => window.__chatTextSamples);
    assert(samples.length >= 2 && samples.some(value => value !== 'SDK snapshot OK'), 'partial text must be observed');
    assert(samples.every(value => 'SDK snapshot OK'.startsWith(value)), JSON.stringify(samples));
    assert.equal(stub.requests.filter(r => r.method === 'session/prompt').length, 1);
    assert.equal(await page.locator('[data-chat-entry="user"]').count(), 1, 'receipt and echo must not duplicate the message');
    assert.equal(await page.locator('[data-privacy="local"]').textContent(), 'Данные на этом компьютере');
  });

  await check('engine-restart', 'text-turn', async (page, host) => {
    const prompt = await recordedPrompt('text-turn');
    await page.getByLabel('Сообщение Помощнику', { exact: true }).fill(prompt);
    await page.getByRole('button', { name: 'Отправить', exact: true }).click();
    await page.waitForSelector('[data-chat-screen][data-connection="recovering"]');
    assert.equal(host.client.child.signalCode, 'SIGKILL');
    assert.equal(await page.getByRole('alert').textContent(), 'Связь с Помощником прервалась, восстанавливаю…');
    assert.equal(await page.getByRole('button', { name: 'Стоп', exact: true }).isDisabled(), true);
    assert.equal(await page.getByRole('button', { name: 'Отправить', exact: true }).isDisabled(), true);
    assert.equal(await page.getByLabel('Сообщение Помощнику', { exact: true }).isDisabled(), true);
    assert.equal(await page.locator('[data-chat-entry="user"]').count(), 2);
    await host.restart();
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
    assert.deepEqual(await page.locator('[data-chat-entry="user"] .chat-user-text').allTextContents(), ['Предыдущий вопрос', prompt]);
    assert.equal(await page.getByText('Предыдущий ответ', { exact: true }).count(), 1);
    assert.equal(await page.getByRole('alert').count(), 0);
    await page.getByLabel('Сообщение Помощнику', { exact: true }).fill(prompt);
    assert.equal(await page.getByRole('button', { name: 'Отправить', exact: true }).isEnabled(), true);
    await page.getByRole('button', { name: 'Отправить', exact: true }).click();
    await page.getByText('SDK snapshot OK', { exact: true }).waitFor();
    await page.waitForFunction(() => document.querySelector('[data-chat-status]')?.textContent === 'Можно написать сообщение');
    assert.equal(host.requests.filter(request => request.method === 'session/prompt').length, 2);
  }, [], { restart: true });

  await check('work-steps', 'persistent-tools', async page => {
    await page.getByLabel('Сообщение Помощнику', { exact: true }).fill(await recordedPrompt('persistent-tools'));
    await page.getByRole('button', { name: 'Отправить', exact: true }).click();
    await page.waitForFunction(() => document.body.textContent.includes('PERSISTENT_TOOLS_OK'));
    await page.getByText('Прочитал note.txt', { exact: true }).click();
    const read = page.locator('[data-operation="read"]');
    assert((await read.locator('[data-step-detail]').innerText()).includes('target:'));
    await page.locator('[data-operation="write"] summary').first().click();
    assert((await page.locator('[data-operation="write"] [data-step-detail]').first().innerText()).includes('target:'));
    assert((await page.locator('[data-operation="command"][data-status="error"] summary').innerText()).includes('код девять'));
    assert.equal(await page.locator('[data-chat-entry="step"]').count(), 7);
    const labels = await page.locator('[data-step-label]').allTextContents();
    assert(labels.some(label => label === 'Команда выполнена без ошибок'));
    assert(!/(инструмент|токен|сесси|str_replace_editor|\{\s*")/i.test(labels.join('\n')));
    await page.locator('[data-operation="write"] summary').first().click();
    const visible = await page.locator('body').innerText();
    assert(!visible.includes('/workspace/') && !visible.includes('/tmp'), 'external absolute paths must be hidden');
  }, [], { viewport: { width: 1440, height: 900 }, shot: 'chat-1440.png' });

  // Extended recording is a planted test input, based on text-turn, to keep an
  // observable playback window for a real click. Original source is untouched.
  const fixtures = await mkdtemp(join(home, 'fixtures-'));
  const cancelRows = await recording('text-turn');
  const reply = cancelRows.find(row => row.type === 'assistant/message');
  const longText = 'Продолжаю ответ. '.repeat(120);
  reply.data.message.content = [{ type: 'text', text: longText }];
  reply.data.stream = [{ type: 'text-chunks', index: 0, time0: 0, dt: [], texts: Array.from(longText) }];
  const cancelFile = join(fixtures, 'cancel.jsonl');
  await writeFile(cancelFile, cancelRows.map(row => JSON.stringify(row)).join('\n') + '\n');
  await check('cancel', 'text-turn', async (page, stub) => {
    await page.getByLabel('Сообщение Помощнику', { exact: true }).fill(await recordedPrompt('text-turn'));
    await page.getByRole('button', { name: 'Отправить', exact: true }).click();
    await page.waitForSelector('[data-chat-entry="assistant"][data-streaming="true"]');
    await page.getByRole('button', { name: 'Стоп', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('[data-chat-status]')?.textContent === 'Можно написать сообщение');
    const after = await page.locator('[data-chat-entry="assistant"] [data-markdown]').textContent();
    await page.waitForTimeout(150);
    assert.equal(await page.locator('[data-chat-entry="assistant"] [data-markdown]').textContent(), after);
    assert.equal(after, longText, 'Stop retains all text received before cancellation');
    await page.reload();
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
    assert.equal(await page.locator('[data-chat-entry="assistant"] [data-markdown]').textContent(), after);
    assert.equal(await page.locator('[data-chat-entry="assistant"]').count(), 1);
    assert.equal(await page.locator('[data-chat-entry="user"]').count(), 1);
    assert.equal(stub.requests.filter(r => r.method === 'session/cancel').length, 1);
    assert.equal(await page.locator('[data-chat-entry="assistant"][data-streaming="true"]').count(), 0);
  }, ['--recording', cancelFile]);

  async function historyFile(name, messages) {
    const base = await recording('text-turn');
    const history = messages.map((value, index) => ({
      type: index % 2 === 0 && name === 'many' ? 'user/message' : 'assistant/message',
      data: index % 2 === 0 && name === 'many'
        ? { id: 'history-' + index, role: 'user', source: { kind: 'user' }, content: [{ type: 'text', text: value }] }
        : { message: { id: 'history-' + index, role: 'assistant', source: { kind: 'model' }, content: [{ type: 'text', text: value }] } },
    }));
    const path = join(fixtures, name + '.jsonl');
    await writeFile(path, [base[0], ...history, ...base.slice(1)].map(row => JSON.stringify(row)).join('\n') + '\n');
    return path;
  }
  const harmful = harmfulMarkdownCases.map(({ source }) => source);
  const markdownFile = await historyFile('markdown', harmful);
  await check('safe-markdown', 'text-turn', async page => {
    const timeline = page.getByTestId('chat-timeline');
    await timeline.evaluate(element => { element.scrollTop = 0; });
    const seen = new Set();
    for (let scan = 0; scan < 30 && seen.size < harmful.length; scan++) {
      for (const { source, expected } of harmfulMarkdownCases) {
        if (await page.getByText(expected, { exact: true }).count()) seen.add(source);
      }
      assert.equal(await page.locator('[data-markdown] script, [data-markdown] img, [data-markdown] svg, [data-markdown] iframe, [data-markdown] a, [data-markdown] form, [data-markdown] input, [data-markdown] style').count(), 0);
      await timeline.evaluate(async element => {
        element.scrollTop += Math.max(1, element.clientHeight / 2);
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      });
    }
    assert.equal(seen.size, harmful.length, 'all harmful sources must remain observable as literal text');
    assert.equal(await page.evaluate(() => Boolean(window.__markdownRan)), false);
  }, ['--recording', markdownFile]);

  const privacyRows = await recording('text-turn');
  const privacyHistory = [];
  const assistantProse = 'Сессия агента: 12 токенов, инструмент bash. {"tool":"read_file"}';
  privacyHistory.push({ type: 'assistant/message', data: { message: { id: 'prose', content: [{ type: 'text', text: assistantProse }] } } });
  for (const [index, [source, expected]] of pathCases.entries()) {
    privacyHistory.push({ type: 'assistant/message', data: { message: { id: 'path-' + index, content: [{ type: 'text', text: 'Строка ' + index + ': ' + source }] } } });
    privacyHistory.push({ type: 'tool/call', data: { name: 'bash', callId: 'path-' + index, arguments: {} } });
    privacyHistory.push({ type: 'tool/result', data: { message: { toolCallId: 'path-' + index, content: [{ type: 'text', text: source }] } } });
  }
  for (const [index, source] of technicalCases.entries()) {
    privacyHistory.push({ type: 'tool/call', data: { name: 'write_file', callId: 'technical-' + index, arguments: { path: '/workspace/chat/agent.json', content: source } } });
    privacyHistory.push({ type: 'tool/result', data: { message: { toolCallId: 'technical-' + index, content: [{ type: 'text', text: source }] } } });
  }
  const privacyFile = join(fixtures, 'privacy.jsonl');
  await writeFile(privacyFile, [privacyRows[0], ...privacyHistory, ...privacyRows.slice(1)].map(row => JSON.stringify(row)).join('\n') + '\n');
  await check('paths-and-window-language', 'text-turn', async page => {
    const timeline = page.getByTestId('chat-timeline');
    await timeline.evaluate(element => { element.scrollTop = 0; });
    await page.getByText(assistantProse, { exact: true }).waitFor();
    const seen = new Set();
    const details = new Set();
    for (let scan = 0; scan < 100 && (seen.size < pathCases.length || details.size < pathCases.length + technicalCases.length); scan++) {
      const paths = await page.locator('[data-chat-entry="assistant"]').allTextContents();
      for (const [index] of pathCases.entries()) {
        // Exact plain-text redaction is covered by the shared unit corpus;
        if (paths.some(value => value.includes('Строка ' + index + ':'))) seen.add(index);
      }
      for (const row of await page.locator('[data-chat-entry="step"]').all()) {
        const id = await row.getAttribute('data-entry-id');
        await row.locator('details').evaluate(element => { element.open = true; });
        const value = await row.locator('[data-step-detail]').innerText();
        assert(!/\/etc|file:\/\/|server|Someone|secret\.txt|private folder/.test(value), value);
        assert(!/агент|сесси|токен|инструмент|agent|session|token|tool|bash|read_file|str_replace_editor|\{|\[\s*(?:\]|\[|"|-?\d)/.test(value), value);
        details.add(id);
      }
      const visible = await timeline.innerText();
      assert(!/\/etc|file:\/\/|\\\\server|C:\\Users/.test(visible), visible);
      await timeline.evaluate(async element => {
        element.scrollTop += Math.max(1, element.clientHeight / 2);
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      });
    }
    assert.equal(seen.size, pathCases.length);
    assert.equal(details.size, pathCases.length + technicalCases.length);
  }, ['--recording', privacyFile]);

  const manyFile = await historyFile('many', Array.from({ length: 5000 }, (_, i) => 'Сообщение ' + String(i).padStart(4, '0')));
  await check('5000-messages', 'text-turn', async page => {
    const timeline = page.getByTestId('chat-timeline');
    await page.getByText('Сообщение 4999', { exact: true }).waitFor();
    assert(await page.locator('[data-chat-entry]').count() < 200, 'timeline must be virtualized');
    await timeline.evaluate(element => { element.scrollTop = 0; });
    await page.getByText('Сообщение 0000', { exact: true }).waitFor();
    const metrics = await timeline.evaluate(async element => {
      const gaps = []; let previous = performance.now();
      for (let index = 0; index < 120; index++) {
        element.scrollTop = (element.scrollHeight - element.clientHeight) * index / 119;
        await new Promise(resolve => requestAnimationFrame(resolve));
        const now = performance.now(); gaps.push(now - previous); previous = now;
      }
      gaps.sort((a, b) => a - b);
      return { count: 5000, frames: gaps.length, p95: gaps[Math.floor(gaps.length * 0.95)], max: gaps.at(-1), thresholdMs: 100 };
    });
    await writeFile(join(RESULTS, 'scroll-performance.json'), JSON.stringify(metrics, null, 2) + '\n');
    assert(metrics.p95 <= 100, `p95 frame gap ${metrics.p95.toFixed(1)} ms exceeds 100 ms`);
    await page.getByText('Сообщение 4999', { exact: true }).waitFor();
  }, ['--recording', manyFile]);

  await writeFile(join(RESULTS, 'behavior.json'), JSON.stringify({ status: 'passed', passed: results.length, results }, null, 2) + '\n');
  console.log(`Behavior: ${results.length} passed, 0 failed; A review images: design/build-a/chat-1440.png, design/build-a/chat-1440-light.png`);
} catch (error) {
  await writeFile(join(RESULTS, 'behavior.json'), JSON.stringify({ status: 'failed', results, error: String(error) }, null, 2) + '\n');
  console.error(error); process.exitCode = 1;
} finally {
  await browser?.close(); await rm(home, { recursive: true, force: true });
}
