// Headless behavior checks plus the requested A review image; no HTTP server.
import assert from 'node:assert/strict';
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { StubClient, RestartingStubHost, recording, recordedPrompt } from './stub-client.mjs';
import { FakeEngine } from './fake-engine.mjs';
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
    const stub = options.fake ? options.fake() : options.restart ? await RestartingStubHost.open() : await StubClient.open(scenario, extra);
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
        subscribeQuestions: async listener => { window.__hohAsk = listener; return () => { window.__hohAsk = undefined; }; },
        answerQuestion: async (requestId, answer) => { window.__hohAnswers.push({ requestId, answer }); },
        approvalHistory: async () => window.__hohApprovalHistory ?? [],
        openLink: async (kind, target, sessionId) => { window.__hohOpened.push({ kind, target, sessionId }); if (window.__hohOpenFail) throw new Error(window.__hohOpenFail); },
      };
      window.__hohAnswers = []; window.__hohOpened = [];
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
      else await page.waitForSelector(`[data-chat-screen][data-connection="${options.connection ?? 'ready'}"]`);
      await callback(page, stub);
      await Promise.all(deliveries);
      assert.deepEqual(errors, [], 'renderer exceptions'); assert.deepEqual(remote, [], 'unexpected resource loads');
      assert.equal(await page.locator('[data-chat-sidebar]').count(), 1, 'one sidebar');
      // Nothing the engine cannot do is shown: no employees, tasks, schedule, widgets, learning.
      assert.equal(await page.locator('[data-chat-sidebar]').getByText(/Сотрудники|Задачи|Расписание|Виджеты|Обучение|Быстрые задачи/).count(), 0);
      assert.equal(await page.getByRole('link', { name: /Сотрудники|Задачи|Расписание|Результаты/ }).count(), 0);
      if (options.shot) {
        await page.evaluate(() => document.fonts.ready);
        await page.mouse.move(0, 0);
        // Review images stay inside ui/: design/ is read-only for window work.
        const shots = join(RESULTS, 'shots'); await mkdir(shots, { recursive: true });
        await page.screenshot({ path: join(shots, options.shot), animations: 'disabled' });
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

  await check('approvals-section', 'text-turn', async page => {
    // what the native core returns from the journal: newest first, addresses already closed
    await page.evaluate(() => {
      window.__hohApprovalHistory = [
        { at: Date.UTC(2026, 10, 19, 18, 4), asker: 'assistant', method: 'bash', outcome: 'rejected', humanText: 'Выполнить команду в папке Документы/HoH: удалить 2 файла', reason: 'Эти файлы старые', details: 'Команда: rm -f old-1.txt old-2.txt' },
        { at: Date.UTC(2026, 10, 19, 18, 1), asker: 'you', method: 'hoh/settings/set', outcome: 'allowed-once', humanText: 'Изменить настройки: провайдер — «https://user:•••@host.example/v1?api_key=•••»' },
      ];
    });
    assert.equal(await page.locator('[data-approvals]').count(), 0, 'раздел открывается по нажатию');
    await page.locator('[data-nav-approvals]').click();
    await page.getByRole('heading', { name: 'Подтверждения' }).waitFor();
    const rows = page.locator('.ap-history-row');
    assert.equal(await rows.count(), 2);
    assert.match(await rows.nth(0).innerText(), /удалить 2 файла[\s\S]*Просил Помощник[\s\S]*Отклонено/);
    assert.match(await rows.nth(1).innerText(), /Вы, в настройках[\s\S]*Разрешено один раз/);
    const shown = await page.locator('[data-approvals]').innerText();
    assert(!shown.includes('rm -f'), 'сырая команда скрыта до раскрытия «Подробностей»');
    assert(shown.includes('api_key=•••'));
    await rows.nth(0).locator('summary').click();
    assert((await rows.nth(0).innerText()).includes('rm -f old-1.txt old-2.txt'));
    if (process.argv[2]) await page.screenshot({ path: join(process.argv[2], 'approvals-section.png'), animations: 'disabled' });
    await page.getByRole('button', { name: /Помощник/ }).first().click();
    await page.getByLabel('Сообщение Помощнику', { exact: true }).waitFor();
    assert.equal(await page.locator('[data-approvals]').count(), 0);
  });

  await check('live-answer-parts', 'text-turn', async page => {
    // the engine's live frames (session.assistant-stream): the text grows, a stop keeps the received part
    const { sessionId } = await page.evaluate(() => window.__HOH_CHAT_TEST_TRANSPORT__.request('hoh/employee/open', { id: 'employee-1' }));
    const send = (method, params) => page.evaluate(([m, p]) => window.__hohChatNotify({ method: m, params: p }), [method, { sessionId, ...params }]);
    const frame = (index, text) => send('session.assistant-stream', { frame: { type: 'chunk', attemptId: 'a1', revision: index + 2, index, time: 1, chunk: { type: 'text-delta', text } } });
    // the markdown chunk loads lazily: read the text only once the T3 render has replaced the plain placeholder
    const answer = async () => { await page.locator('[data-chat-entry="assistant"]').last().locator('[data-renderer="t3"]').waitFor(); return page.locator('[data-chat-entry="assistant"]').last().innerText(); };
    await send('session.status', { status: 'running' });
    await send('session.assistant-stream', { frame: { type: 'start', attemptId: 'a1', revision: 1, turn: 1, step: 1 } });
    await frame(0, 'Я умею читать ');
    await page.waitForFunction(() => document.querySelector('[data-chat-entry="assistant"][data-streaming="true"]'));
    const first = await answer();
    await frame(1, 'и писать файлы в вашей папке.');
    await page.waitForFunction(text => document.querySelector('[data-chat-entry="assistant"]:last-of-type')?.textContent.length > text.length, first);
    const second = await answer();
    assert(second.length > first.length && second.startsWith(first.trim()), `текст должен расти: «${first}» → «${second}»`);
    if (process.argv[2]) await page.screenshot({ path: join(process.argv[2], 'stream-part-2.png'), animations: 'disabled' });
    await page.locator('[data-chat-stop]').click();
    await page.waitForFunction(() => !document.querySelector('[data-chat-entry="assistant"][data-streaming="true"]'));
    assert.equal(await answer(), second, 'после «Стоп» остаётся полученная часть');
    if (process.argv[2]) await page.screenshot({ path: join(process.argv[2], 'stream-stopped.png'), animations: 'disabled' });
  });

  await check('gap-incomplete-markdown', 'text-turn', async page => {
    const { sessionId } = await page.evaluate(() => window.__HOH_CHAT_TEST_TRANSPORT__.request('hoh/employee/open', { id: 'employee-1' }));
    const send = (method, params) => page.evaluate(([m, p]) => window.__hohChatNotify({ method: m, params: p }), [method, { sessionId, ...params }]);
    await send('session.status', { status: 'running' });
    await send('session.assistant-stream', { frame: { type: 'start', attemptId: 'gap-markdown', revision: 1, turn: 9, step: 1 } });
    await send('session.assistant-stream', { frame: { type: 'chunk', attemptId: 'gap-markdown', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Пример:\n```js\nalert(1)' } } });
    await send('session.assistant-stream', { frame: { type: 'gap', attemptId: 'gap-markdown', revision: 3, index: 1 } });
    await send('session.assistant-stream', { frame: { type: 'end', attemptId: 'gap-markdown', revision: 4, index: 2, outcome: 'committed' } });
    await send('session.event', { event: { type: 'turn/end', seq: 9901, time: 1, data: { reason: { kind: 'completed' } } } });
    const answer = page.locator('[data-chat-entry="assistant"]').last();
    await answer.locator('em').waitFor();
    assert.equal(await answer.locator('em').textContent(), 'Ответ получен не полностью.');
    assert.equal(await answer.locator('pre em').count(), 0, 'the marker must render outside an unfinished code fence');
    assert.equal((await answer.locator('pre code').textContent()).trimEnd(), 'alert(1)');
    assert.equal(await answer.getAttribute('data-streaming'), 'false');
  });

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
    assert.equal(await page.getByRole('button', { name: 'Стоп', exact: true }).count(), 0, 'отдельной кнопки «Стоп» нет, пока ответа нет');
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

  const twoChats = () => {
    const engine = new FakeEngine();
    engine.seed('window-main', { user: 'Первый вопрос', assistant: 'Первый ответ' });
    engine.seed('chat-old', { user: 'Старый вопрос', assistant: 'Старый ответ' });
    return engine;
  };
  await check('chat-list', '', async (page, engine) => {
    const rows = page.locator('[data-chat-row]');
    const current = () => page.locator('[data-chat-row][aria-current="true"]');
    const feed = () => page.locator('[data-chat-entry]').evaluateAll(nodes => nodes.map(node => node.querySelector('[data-markdown], .chat-user-text')?.textContent));
    await rows.nth(1).waitFor();
    assert.equal(await rows.count(), 2);
    assert.deepEqual((await rows.locator('.chat-row-label').allTextContents()).sort(), ['Первый вопрос', 'Старый вопрос']);
    assert.equal(await current().locator('.chat-row-label').textContent(), 'Первый вопрос');
    assert.equal(await page.locator('.chat-title').textContent(), 'Первый вопрос');
    assert.deepEqual(await feed(), ['Первый вопрос', 'Первый ответ']);
    // switching
    await page.locator('[data-chat-id="chat-old"]').click();
    await page.getByText('Старый ответ', { exact: true }).waitFor();
    assert.deepEqual(await feed(), ['Старый вопрос', 'Старый ответ']);
    assert.equal(await page.locator('.chat-title').textContent(), 'Старый вопрос');
    assert.equal(await page.getByText('Первый ответ', { exact: true }).count(), 0, 'histories are not mixed');
    // a new chat is empty, listed and current
    await page.locator('[data-new-chat]').click();
    await page.locator('[data-chat-empty]').waitFor();
    await page.waitForFunction(() => document.querySelectorAll('[data-chat-row]').length === 3);
    assert.equal(await page.locator('[data-chat-entry]').count(), 0);
    assert.equal(await current().locator('.chat-row-label').textContent(), 'Новый чат');
    await page.locator('[data-new-chat]').click();      // empty chat is reused
    await page.waitForTimeout(100);
    assert.equal(await rows.count(), 3);
    // write in the new chat; its title follows the first message; the other chats are untouched
    await page.getByLabel('Сообщение Помощнику', { exact: true }).fill('Третья тема');
    await page.getByRole('button', { name: 'Отправить', exact: true }).click();
    await page.getByText('Ответ на: Третья тема', { exact: true }).waitFor();
    await page.waitForFunction(() => [...document.querySelectorAll('[data-chat-row] .chat-row-label')].some(node => node.textContent === 'Третья тема'));
    assert.equal(await page.locator('.chat-title').textContent(), 'Третья тема');
    await page.screenshot({ path: join(RESULTS, 'shots', 'chat-list-1440.png'), animations: 'disabled' });
    await page.locator('[data-chat-id="window-main"]').click();
    await page.getByText('Первый ответ', { exact: true }).waitFor();
    assert.deepEqual(await feed(), ['Первый вопрос', 'Первый ответ']);
    await page.locator('[data-chat-row]', { hasText: 'Третья тема' }).click();
    await page.getByText('Ответ на: Третья тема', { exact: true }).waitFor();
    assert.deepEqual(await feed(), ['Третья тема', 'Ответ на: Третья тема']);
    // keyboard: Ctrl+N starts a chat; one while the open chat is empty changes nothing
    await page.keyboard.press('Control+n');
    await page.locator('[data-chat-empty]').waitFor();
    await page.waitForFunction(() => document.querySelectorAll('[data-chat-row]').length === 4);
    await page.keyboard.press('Control+n'); await page.waitForTimeout(100);
    assert.equal(await rows.count(), 4);
    // nothing the engine cannot do is offered
    assert.equal(await page.locator('body').getByText(/Сотрудник|Расписание|Виджет|Обучение|Быстрые задачи/).count(), 0);
    assert.equal(await page.locator('[data-nav-chat] .chat-state').textContent(), 'готов к работе');
    assert.equal(engine.requests.filter(r => r.method === 'session/prompt').length, 1);
  }, [], { fake: twoChats, viewport: { width: 1440, height: 900 } });
  await check('model-label-follows-the-executor', '', async page => {
    await page.locator('[data-chat-model]').waitFor();
    assert.equal(await page.locator('[data-chat-model]').textContent(), 'Локальная модель в сети', 'частная сеть названа сетью, не этим компьютером');
    assert.match(await page.locator('.chat-privacy').textContent(), /на другом компьютере/);
  }, [], { fake: () => { const engine = twoChats(); engine.baseUrl = 'http://192.168.1.5:8000/v1'; return engine; }, viewport: { width: 1440, height: 900 } });
  await check('narrow-sidebar-closes', '', async page => {
    const sidebar = page.locator('[data-chat-sidebar]');
    assert.equal(await sidebar.isVisible(), false);
    await page.locator('[data-chat-menu]').click();
    assert.equal(await sidebar.isVisible(), true);
    assert.equal(await page.locator('[data-chat-scrim]').count(), 1, 'под колонкой затемнение');
    await page.mouse.click(700, 300);                       // a click on the feed side closes it
    assert.equal(await sidebar.isVisible(), false);
    await page.locator('[data-chat-menu]').click();
    await page.keyboard.press('Escape');                    // so does Escape
    assert.equal(await sidebar.isVisible(), false);
    assert.equal(await page.locator('[data-chat-scrim]').count(), 0);
    await page.locator('[data-chat-menu]').click();
    await page.screenshot({ path: join(RESULTS, 'shots', 'narrow-menu-800x640.png'), animations: 'disabled' });
  }, [], { fake: twoChats, viewport: { width: 800, height: 640 } });
  await check('assistant-state-label-not-cut-on-the-left', '', async page => {
    await page.evaluate(() => window.__hohChatEngineStatus({ state: 'restarting' }));
    await page.waitForSelector('[data-chat-screen][data-connection="recovering"]');
    const box = await page.evaluate(() => {
      const row = document.querySelector('[data-nav-chat]').getBoundingClientRect();
      const text = document.querySelector('[data-nav-chat] .chat-state-text');
      const rect = text.getBoundingClientRect();
      return { rowLeft: row.left, rowRight: row.right, left: rect.left, right: rect.right, ellipsis: getComputedStyle(text).textOverflow, full: text.textContent };
    });
    assert.equal(box.full, 'восстанавливаю связь');
    assert(box.left >= box.rowLeft && box.right <= box.rowRight, 'подпись целиком внутри строки: ' + JSON.stringify(box));
    assert.equal(box.ellipsis, 'ellipsis', 'лишнее обрезается справа многоточием');
  }, [], { fake: twoChats, viewport: { width: 1440, height: 900 } });
  await check('command-steps-differ', '', async page => {
    await page.locator('[data-operation="command"]').nth(2).waitFor();
    const labels = await page.locator('[data-operation="command"] [data-step-label]').allTextContents();
    assert.deepEqual(labels, ['Команда завершена: список файлов', 'Команда завершена: поиск в файлах', 'Команда завершена']);
    assert.equal(await page.locator('[data-chat-permissions]').textContent(), 'Работа только в этой папке');
  }, [], { fake: () => {
    const engine = new FakeEngine();
    const main = engine.seed('window-main', { user: 'Что в папке?' });
    for (const [id, command] of [['c1', 'ls -la'], ['c2', 'grep -r план .'], ['c3', 'curl https://x.example/y']]) {
      engine.append(main, 'tool/call', { name: 'bash', callId: id, arguments: { command } });
      engine.append(main, 'tool/result', { message: { toolCallId: id, content: [{ type: 'text', text: 'ok' }] } });
    }
    engine.append(main, 'assistant/message', { message: { id: 'a1', content: [{ type: 'text', text: 'Готово.' }] } });
    return engine;
  }, viewport: { width: 1440, height: 900 }, shot: 'p2-command-steps-1440.png' });
  await check('no-engine-says-so', '', async page => {
    assert.match(await page.locator('.chat-privacy').textContent(), /Нет связи: не знаю, где обрабатываются данные/);
    assert.equal(await page.getByText('Проверяю, где обрабатываются данные').count(), 0, 'не «проверяю» навсегда');
  }, [], { fake: () => new FakeEngine({ unavailable: ['session/working-directory/get'] }), connection: 'error', viewport: { width: 1440, height: 900 }, shot: 'p2-no-engine-1440.png' });
  await check('composer-line', '', async (page, engine) => {
    await page.locator('[data-chat-folder]').waitFor();
    assert.equal(await page.getByText(/Enter отправляет|Shift\+Enter/).count(), 0, 'подсказки про Enter нет');
    assert.equal(await page.getByRole('button', { name: 'Стоп', exact: true }).count(), 0, 'отдельной кнопки «Стоп» нет');
    assert.equal(await page.locator('[data-chat-permissions]').textContent(), 'Работа только в этой папке');
    assert.equal(await page.locator('[data-chat-model]').textContent(), 'Локальная модель');
    assert.equal(await page.locator('[data-chat-sidebar] [data-chat-folder], .chat-side-foot').count(), 0, 'папки нет в боковой колонке');
    await page.evaluate(() => { window.__HOH_ENGINE__ = { request: async () => ({}), pickFolder: async () => '/home/me/Проекты/сайт' }; });
    await page.locator('[data-chat-folder]').click();
    await page.waitForFunction(() => document.querySelector('[data-chat-folder]')?.textContent === 'сайт');
    assert.deepEqual(engine.requests.find(r => r.method === 'session/working-directory/set')?.params.path, '/home/me/Проекты/сайт');
    assert.equal(await page.locator('[data-chat-screen] .chat-composer button[type="submit"]').count(), 1, 'одна круглая кнопка');
  }, [], { fake: twoChats, viewport: { width: 1440, height: 900 } });

  await check('assistant-question', 'text-turn', async page => {
    const { sessionId } = await page.evaluate(() => window.__HOH_CHAT_TEST_TRANSPORT__.request('hoh/employee/open', { id: 'employee-1' }));
    // A question of another chat is not shown in this one.
    await page.evaluate(() => window.__hohAsk({ requestId: 'r0', sessionId: 'another-chat', questions: [{ id: 'q', question: 'Чужой вопрос?' }] }));
    await page.waitForTimeout(100);
    assert.equal(await page.locator('[data-chat-question]').count(), 0, 'only the question of the open chat is shown');
    await page.evaluate(id => window.__hohAsk({ requestId: 'r1', sessionId: id, questions: [
      { id: 'format', question: 'Какой формат отчёта?', options: [{ label: 'PDF' }, { label: 'Word', description: 'Для правок' }] },
      { id: 'note', question: 'Что подчеркнуть? <b>тег</b>', multiSelect: true, options: [{ label: 'Сроки' }, { label: 'Цена' }] }] }), sessionId);
    const card = page.locator('[data-chat-question]');
    await card.waitFor();
    assert.equal(await card.locator('b').count(), 0, 'question text is plain text');
    const send = card.locator('[data-question-send]');
    assert.equal(await send.isDisabled(), true);
    await card.getByLabel('PDF').check();
    await card.getByLabel('Сроки').check(); await card.getByLabel('Цена').check();
    await card.locator('[data-question-id="note"] .chat-question-custom').fill('  и ещё про риски ');
    assert.equal(await send.isDisabled(), false);
    await send.click();
    await card.waitFor({ state: 'detached' });
    const answers = await page.evaluate(() => window.__hohAnswers);
    assert.deepEqual(answers, [{ requestId: 'r1', answer: { answers: [
      { id: 'format', selected: ['PDF'] }, { id: 'note', selected: ['Сроки', 'Цена'], custom: 'и ещё про риски' }] } }]);
    await page.evaluate(id => window.__hohAsk({ requestId: 'r2', sessionId: id, questions: [{ id: 'q', question: 'Продолжать?' }] }), sessionId);
    await page.locator('[data-question-skip]').click();
    await page.locator('[data-chat-question]').waitFor({ state: 'detached' });
    assert.deepEqual((await page.evaluate(() => window.__hohAnswers)).at(-1), { requestId: 'r2', answer: { cancelled: true } });
  });

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
    assert.equal(after.trimEnd(), longText.trimEnd(), 'Stop retains all text received before cancellation');
    await page.reload();
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
    assert.equal((await page.locator('[data-chat-entry="assistant"] [data-markdown]').textContent()).trimEnd(), after.trimEnd());
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
  const linksFile = await historyFile('links', ['Смотри [код](a.ts:12), [док](https://example.com/a) и ![схема](https://example.com/i.png) [плохо](javascript:1)']);
  await check('links-and-images', 'text-turn', async page => {
    await page.locator('[data-markdown-link="file"]').waitFor();
    assert.equal(await page.locator('[data-markdown-link="file"]').textContent(), 'a.ts:12');
    assert.equal(await page.locator('[data-markdown-link="url"]').textContent(), 'докexample.com');
    assert.equal(await page.locator('[data-markdown-image="placeholder"]').textContent(), 'Картинка: схема');
    assert.equal(await page.locator('[data-markdown-link="inert"]').textContent(), '[плохо](javascript:1)');
    assert.equal(await page.locator('[data-markdown] a, [data-markdown] img').count(), 0);
  }, ['--recording', linksFile]);
  const linkEngine = () => {
    const engine = new FakeEngine();
    const main = engine.seed('window-main', { user: 'Покажи файлы' });
    engine.cwd = '/home/me/Проекты/сайт';
    engine.append(main, 'tool/call', { name: 'write_file', callId: 'w1', arguments: { path: '../qa-outside.txt', content: 'x' } });
    engine.append(main, 'tool/result', { message: { toolCallId: 'w1', isError: true, content: [{ type: 'text', text: '[sandbox: file access denied under workspace-write mode]\n[sandbox: escalation available — retry this exact operation once]' }] } });
    engine.append(main, 'tool/call', { name: 'write_file', callId: 'w2', arguments: { path: 'ok.txt', content: 'x' } });
    engine.append(main, 'tool/result', { message: { toolCallId: 'w2', isError: true, content: [{ type: 'text', text: 'disk full' }] } });
    engine.append(main, 'tool/call', { name: 'read_file', callId: 'r1', arguments: { path: 'notes.txt' } });
    engine.append(main, 'tool/result', { message: { toolCallId: 'r1', content: [{ type: 'text', text: '<path>notes.txt</path>\n<type>file</type>\n<content>\n1: alpha\n2: beta\n\n(End of file - total 2 lines)\n</content>' }] } });
    engine.append(main, 'assistant/message', { message: { id: 'a1', content: [{ type: 'text', text: 'Файлы: [заметки](notes.txt), [внутри](sub/inner.txt), [чужой](/etc/passwd), [снаружи](../qa-outside.txt), [сайт](https://example.com/a), [плохо](javascript:1) и [удалённый](file:///etc/passwd)' }] } });
    return engine;
  };
  await check('denied-step-and-links', '', async page => {
    await page.locator('[data-operation="write"]').nth(1).waitFor();
    const steps = page.locator('[data-operation="write"]');
    // F-02: a refusal outside the folder says so and says what to do; other failures stay generic
    assert.equal(await steps.nth(0).locator('[data-step-label]').textContent(), 'Запись запрещена за пределами папки');
    await steps.nth(0).locator('summary').click();
    const detail = await steps.nth(0).locator('[data-step-detail]').textContent();
    assert.match(detail, /работает только в папке «сайт»/); assert.match(detail, /выберите другую папку внизу поля ввода/);
    assert(!/ещё раз/.test(detail), 'не «попробуйте ещё раз»');
    assert.equal(await steps.nth(1).locator('[data-step-label]').textContent(), 'Не удалось записать ok.txt');
    await steps.nth(1).locator('summary').click();
    assert.match(await steps.nth(1).locator('[data-step-detail]').textContent(), /Попробуйте ещё раз/);
    // F-04: no engine markup in an opened read step
    await page.locator('[data-operation="read"] summary').click();
    const read = await page.locator('[data-operation="read"] [data-step-detail]').textContent();
    assert.equal(read, 'alpha\nbeta'); assert(!/<path>|<content>/.test(read));
    // F-03: links look like links and open only through the host
    const opened = () => page.evaluate(() => window.__hohOpened);
    await page.locator('[data-markdown-link="url"]').click();
    await page.locator('[data-markdown-link="file"]').first().click();
    await page.locator('[data-markdown-reveal]').first().click();
    assert.deepEqual(await opened(), [
      { kind: 'url', target: 'https://example.com/a', sessionId: 'window-main' },
      { kind: 'file', target: 'notes.txt', sessionId: 'window-main' },
      { kind: 'reveal', target: 'notes.txt', sessionId: 'window-main' },
    ]);
    // P1-3: a file in a sub-folder keeps its path and its name on the chip
    const inner = page.locator('[data-markdown-link="file"]', { hasText: 'inner.txt' });
    assert.equal(await inner.textContent(), 'inner.txt');
    await inner.click();
    assert.deepEqual((await opened()).at(-1), { kind: 'file', target: 'sub/inner.txt', sessionId: 'window-main' });
    await page.evaluate(() => { window.__hohOpened.length = 3; });
    const outside = page.locator('[data-markdown-link="file-outside"]');
    assert.equal(await outside.count(), 3, 'внешние файлы помечены: чужой, снаружи и file: вне папки');
    await outside.first().click();
    assert.match(await page.locator('[data-link-notice]').textContent(), /вне папки «сайт».*выберите другую папку внизу поля ввода/);
    assert.equal((await opened()).length, 3, 'файл вне папки ничего не открывает');
    assert.equal(await page.locator('[data-markdown-link="inert"]').count(), 1, 'javascript: остаётся текстом');
    assert.equal(await page.locator('[data-markdown] a').count(), 0);
    // a refusal of the host is shown in words
    await page.evaluate(() => { window.__hohOpenFail = 'Файл не найден'; });
    await page.locator('[data-markdown-link="file"]').first().click();
    await page.waitForFunction(() => document.querySelector('[data-link-notice]')?.textContent.startsWith('Файл не найден'));
    // N-2: a blocked link says why, and opens nothing
    const before = (await opened()).length;
    await page.locator('[data-markdown-link="inert"]').click();
    assert.match(await page.locator('[data-link-notice]').textContent(), /Эта ссылка не открывается/);
    assert.equal((await opened()).length, before);
    // N-7: the next message removes the banner; so does «Закрыть»
    await page.getByLabel('Сообщение Помощнику', { exact: true }).fill('Дальше');
    await page.getByRole('button', { name: 'Отправить', exact: true }).click();
    await page.waitForFunction(() => !document.querySelector('[data-link-notice]'));
    await page.locator('[data-markdown-link="inert"]').click();
    await page.locator('[data-link-notice]').getByRole('button', { name: 'Закрыть' }).click();
    assert.equal(await page.locator('[data-link-notice]').count(), 0);
  }, [], { fake: linkEngine, viewport: { width: 1440, height: 900 }, shot: 'links-1440.png' });

  const richFile = await historyFile('rich', ['Формула $E=mc^2$ в строке.\n\n$$\\int_0^1 x\\,dx = \\frac12$$\n\n| Имя | Число |\n|---|---|\n| один | 1 |\n| два | 2 |\n\n```js\nconst a = 1;\n```\n\n- [x] сделано\n- [ ] ждёт']);
  await check('markdown-formula-table-code', 'text-turn', async page => {
    const markdown = page.locator('[data-markdown]').last();
    await markdown.locator('table').waitFor();
    assert.deepEqual(await markdown.locator('th').allTextContents(), ['Имя', 'Число']);
    assert.deepEqual(await markdown.locator('td').allTextContents(), ['один', '1', 'два', '2']);
    assert.equal((await markdown.locator('pre code').textContent()).trim(), 'const a = 1;');
    await markdown.locator('[data-renderer="t3"]').or(markdown).first().waitFor();
    await page.waitForSelector('[data-math="inline"] math', { timeout: 20000 });
    await page.waitForSelector('[data-math="display"] math[display="block"]');
    assert.equal(await markdown.locator('[data-math] [style]').count(), 0, 'в формулах нет встроенных style (CSP окна их запрещает)');
    assert.equal(await markdown.locator('[data-math]').count(), 2);
    assert.equal(await markdown.locator('[data-math="pending"]').count(), 0);
    assert.match(await markdown.locator('[data-math="inline"]').textContent(), /E=mc/);
    assert.equal(await markdown.locator('a, img, script').count(), 0);
    const shots = join(UI, 'test-results/t3-exact'); await mkdir(shots, { recursive: true });
    for (const [name, viewport] of [['1440', { width: 1440, height: 900 }], ['800x640', { width: 800, height: 640 }]]) {
      await page.setViewportSize(viewport);
      await page.waitForTimeout(300);
      await page.screenshot({ path: join(shots, `t3md-formula-table-code-${name}.png`), animations: 'disabled' });
    }
  }, ['--recording', richFile]);

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
  console.log(`Behavior: ${results.length} passed, 0 failed; review image: ui/test-results/chat/shots/chat-1440.png`);
} catch (error) {
  await writeFile(join(RESULTS, 'behavior.json'), JSON.stringify({ status: 'failed', results, error: String(error) }, null, 2) + '\n');
  console.error(error); process.exitCode = 1;
} finally {
  await browser?.close(); await rm(home, { recursive: true, force: true });
}
