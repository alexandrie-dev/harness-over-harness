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
const FULL = 'Помощник сам удаляет и меняет файлы в любых папках, куда есть доступ у вас, запускает программы и выходит в интернет, не спрашивая. Сделанное нельзя отменить.';
const UX = join(UI, 'test-results/t3-exact');
await mkdir(UX, { recursive: true });
const RESULTS = join(UI, 'test-results/chat');
// Прежняя оболочка — dist/legacy.html (HOH-40: dist/index.html — веб-клиент T3).
const INDEX = join(UI, 'dist/legacy.html');
await mkdir(RESULTS, { recursive: true });
let chromium;
const missing = [];
try { ({ chromium } = await import('playwright-core')); } catch { missing.push('playwright-core@1.65.0 is not installed'); }
try { await access(INDEX); } catch { missing.push('ui/dist/legacy.html has not been built'); }
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
    // A refusal of the host is an object ({code, message}); it crosses the binding as a value and is thrown again in the page.
    await page.exposeBinding('__hohChatRequest', async (_, method, params) => { try { return await stub.request(method, params); } catch (error) { if (error && !(error instanceof Error) && typeof error.code === 'string') return { __hostError: error }; throw error; } });
    await page.addInitScript(() => {
      const listeners = new Set(); const statusListeners = new Set();
      window.__HOH_CHAT_TEST_TRANSPORT__ = {
        request: async (method, params) => { const result = await window.__hohChatRequest(method, params); if (result && result.__hostError) throw result.__hostError; return result; },
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
      if (options.firstRun) await page.getByRole('heading', { name: 'Кто будет отвечать' }).waitFor();
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

  let wizardEmblem = '';
  await check('first-run-to-chat', 'text-turn', async (page, stub) => {
    const emblem = page.locator('.fr-brand [data-emblem]');
    await emblem.waitFor();
    assert.equal(await page.locator('.fr-brand .mark').count(), 0, 'квадрата «H» больше нет');
    assert.equal(await page.locator('.fr-brand b').textContent(), 'HoH', 'слово рядом осталось');
    wizardEmblem = await emblem.getAttribute('src');
    await mkdir(join(UI, 'test-results/t3-exact'), { recursive: true });
    await page.screenshot({ path: join(UI, 'test-results/t3-exact/emblem-wizard-1440.png'), animations: 'disabled' });
    // правка 12: одна кнопка — «Дальше» сама проверяет; отдельной «Проверить» нет
    assert.equal(await page.getByRole('heading', { name: 'Кто будет отвечать' }).count(), 1);
    assert.equal(await page.getByRole('button', { name: 'Проверить', exact: true }).count(), 0);
    await page.getByRole('button', { name: 'Дальше', exact: true }).click();
    await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
    assert.equal(await page.locator('.fr-stepper-item').count(), 2, 'шага «Знакомство» нет');
    await page.getByRole('button', { name: 'Открыть чат', exact: true }).click();
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
    // правка 13: примеры — в пустом чате
    assert.equal(await page.locator('[data-chat-example]').count(), 3);
    assert.match(await page.locator('[data-folder-line]').textContent(), /Папка Помощника: .+\. Кладите туда файлы/);
    await page.locator('[data-chat-example="file"]').click();
    assert.equal(await page.getByLabel('Сообщение Помощнику', { exact: true }).inputValue(), 'Создай файл «Идеи.txt» и запиши в нём пять идей для выходных');
    await page.screenshot({ path: join(UI, 'test-results/t3-exact/ux-g-empty-chat-examples-1440.png'), animations: 'disabled' });
    assert.equal(stub.requests.filter(r => r.method === 'session/prompt').length, 0, 'onFinish never starts work without Send');
    assert.equal(await page.evaluate(() => localStorage.getItem('hoh:first-run:done')), '1');
    await page.reload();
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
  }, [], { firstRun: true, viewport: { width: 1440, height: 900 } });

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
    await page.getByRole('heading', { name: 'Разрешения' }).waitFor();
    const rows = page.locator('.ap-history-row');
    assert.equal(await rows.count(), 2);
    assert.match(await rows.nth(0).innerText(), /Вы не разрешили: выполнить команду[\s\S]*удалить 2 файла[\s\S]*Просил Помощник[\s\S]*Не разрешено/);
    assert.match(await page.locator('[data-approvals-own] summary').innerText(), /Ваши настройки \(1\)/, 'действия мастера свёрнуты');
    assert(!(await rows.nth(1).isVisible()), 'свёрнутая запись настройки не показана');
    await page.locator('[data-approvals-own] summary').click();
    assert.match(await rows.nth(1).innerText(), /Вы разрешили: изменить настройки[\s\S]*Вы, в настройках[\s\S]*Разрешено один раз/);
    assert.doesNotMatch(await page.locator('[data-approvals]').innerText(), /Выполнить команду|\?\s*$/m, 'прошедшее время, не вопрос');
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
    assert((await page.locator('[data-operation="command"][data-status="error"] summary').innerText()).includes('Не удалось выполнить'));
    assert.equal(await page.locator('[data-chat-entry="step"]').count(), 7);
    const labels = await page.locator('[data-step-label]').allTextContents();
    assert(labels.some(label => label === 'Выполнено'));
    assert(!/(инструмент|токен|сесси|str_replace_editor|\{\s*")/i.test(labels.join('\n')));
    await page.locator('[data-operation="write"] summary').first().click();
    // What the person typed is shown as typed; everything else (answers, steps, details) must not show outside paths.
    const visible = (await page.locator('[data-chat-entry]:not([data-chat-entry="user"])').allInnerTexts()).join('\n');
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
    assert.equal(await page.locator('[data-chat-model]').textContent(), 'Модель на другом компьютере в вашей сети', 'частная сеть названа сетью, не этим компьютером');
    assert.match(await page.locator('.chat-privacy').textContent(), /Отвечает сервис в интернете/);
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
    assert.deepEqual(labels, ['Выполнено: список файлов', 'Выполнено: поиск в файлах', 'Выполнено']);
    assert.equal(await page.locator('[data-chat-permissions]').textContent(), 'Спрашивать разрешение');
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
  const accessEngine = (setup = {}) => () => {
    const engine = new FakeEngine();
    engine.seed('window-main', { user: 'Привет', assistant: 'Здравствуйте. Включи полный доступ — [включи полный доступ](hoh/executor/access/set?mode=full) или [так](javascript:set)' });
    engine.access = 'confirm'; Object.assign(engine, setup);
    return engine;
  };
  const accessCalls = engine => engine.requests.filter(r => r.method.startsWith('hoh/executor/access/'));
  const t3shots = join(UI, 'test-results/t3-exact'); await mkdir(t3shots, { recursive: true });
  await check('access-mode', '', async (page, engine) => {
    const chip = page.locator('[data-chat-access]');
    await chip.waitFor();
    assert.equal(await chip.getAttribute('data-access-mode'), 'confirm', 'умолчание — с подтверждением');
    assert.equal(await chip.locator('.chat-access-text').textContent(), 'Спрашивать разрешение');
    assert.deepEqual(accessCalls(engine).map(r => r.method), ['hoh/executor/access/get']);
    assert.deepEqual(accessCalls(engine)[0].params, { executor: 'claude', sessionId: 'window-main' }, 'движок спрашивают про открытый чат');
    assert.equal(await page.locator('[data-chat-permissions]').count(), 0, 'вместо слов о папке — переключатель');
    await page.screenshot({ path: join(t3shots, 'access-confirm-1440.png'), animations: 'disabled' });
    await chip.click();
    await page.locator('[data-access-menu]').waitFor();
    assert.equal(await page.locator('[data-access-option]').count(), 2, 'два режима');
    await page.screenshot({ path: join(t3shots, 'access-menu-1440.png'), animations: 'disabled' });
    await page.locator('[data-access-option="full"]').click();
    await page.locator('[data-enable-full]').click();
    await page.waitForFunction(() => document.querySelector('[data-chat-access]')?.getAttribute('data-access-mode') === 'full');
    // set was sent, then the mode was read back; the label follows the read-back (effective), not the click
    assert.deepEqual(accessCalls(engine).map(r => r.method), ['hoh/executor/access/get', 'hoh/executor/access/set', 'hoh/executor/access/get']);
    assert.deepEqual(accessCalls(engine)[1].params, { executor: 'claude', mode: 'full' });
    assert.equal(await chip.locator('.chat-access-text').textContent(), 'Полный доступ: без вопросов');
    await page.mouse.move(0, 0);
    await page.screenshot({ path: join(t3shots, 'access-full-1440.png'), animations: 'disabled' });
    // the engine remembers: a fresh window start reads full
    await page.reload();
    await page.waitForSelector('[data-chat-access][data-access-mode="full"]');
    // and back
    await page.locator('[data-chat-access]').click();
    await page.locator('[data-access-option="confirm"]').click();
    await page.waitForSelector('[data-chat-access][data-access-mode="confirm"]');
  }, [], { fake: accessEngine(), viewport: { width: 1440, height: 900 } });
  const say = async (page, text) => {
    await page.locator('[data-chat-input]').fill(text);
    await page.locator('[data-chat-send]').click();
    await page.getByText(`Ответ на: ${text}`).waitFor();
    await page.waitForFunction(() => !document.querySelector('[data-chat-stop]'));
  };
  await check('access-mode-follows-the-next-message-and-can-be-taken-back', '', async (page, engine) => {
    const chip = page.locator('[data-chat-access]');
    await chip.waitFor();
    await say(page, 'первое');
    assert.equal(await page.locator('[data-access-pending]').count(), 0, 'пока ничего не менялось, строки нет');
    await chip.click(); await page.locator('[data-access-option="full"]').click(); await page.locator('[data-enable-full]').click();
    await page.waitForSelector('[data-chat-access][data-access-mode="full"]');
    assert.equal(await page.locator('[data-access-pending]').textContent(), 'Вступит в силу со следующего сообщения', 'выбранный режим на чипе, процесс ещё в прежнем');
    assert.equal(await page.locator('[data-link-notice]').count(), 0, 'расхождения с процессом не выдаются за ошибку');
    // передумал до сообщения: «Спрашивать разрешение» можно выбрать снова
    await chip.click(); await page.locator('[data-access-option="confirm"]').click();
    await page.waitForSelector('[data-chat-access][data-access-mode="confirm"]');
    assert.equal(await page.locator('[data-access-pending]').count(), 0, 'вернули до сообщения: ждать нечего');
    await chip.click(); await page.locator('[data-access-option="full"]').click(); await page.locator('[data-enable-full]').click();
    await page.waitForSelector('[data-chat-access][data-access-mode="full"]');
    await page.screenshot({ path: join(UX, 'ux-access-pending-1440.png'), animations: 'disabled' });
    await page.locator('[data-chat-input]').fill('второе');
    await page.locator('[data-chat-send]').click();
    assert.equal(await page.locator('[data-access-pending]').count(), 1, 'отправка строку не стирает');
    await page.getByText('Ответ на: второе').waitFor();
    await page.waitForFunction(() => !document.querySelector('[data-access-pending]'));
    assert.equal(await page.locator('[data-chat-access]').getAttribute('data-access-mode'), 'full', 'следующий ход пошёл в выбранном режиме');
    await chip.click(); await page.locator('[data-access-option="confirm"]').click();
    await page.waitForSelector('[data-chat-access][data-access-mode="confirm"]');
    assert.equal(await page.locator('[data-access-pending]').count(), 1, 'и в обратную сторону');
    assert(engine.requests.filter(r => r.method === 'hoh/executor/access/get').every(r => r.params.sessionId === 'window-main'), 'движок спрашивают про открытый чат');
  }, [], { fake: accessEngine(), viewport: { width: 1440, height: 900 } });
  await check('two-chats-show-their-own-mode-and-provider', '', async (page, engine) => {
    const open = async title => { await page.locator('[data-chat-row]', { hasText: title }).click(); await page.waitForFunction(t => document.querySelector('[data-chat-row][aria-current="true"]')?.textContent.includes(t), title); await page.waitForSelector('[data-chat-input]:not([disabled])'); };
    await page.locator('[data-chat-access]').waitFor();
    await say(page, 'в первом');
    await page.locator('[data-chat-access]').click(); await page.locator('[data-access-option="full"]').click(); await page.locator('[data-enable-full]').click();
    await page.waitForSelector('[data-chat-access][data-access-mode="full"]');
    await open('Старый');
    await page.waitForSelector('[data-chat-access][data-access-mode="full"]');
    assert.equal(await page.locator('[data-access-pending]').count(), 0, 'второй чат ещё не шёл: вопросов нет');
    await say(page, 'во втором');
    await open('Привет');
    await page.waitForSelector('[data-access-pending]');
    assert.equal(await page.locator('[data-chat-access]').getAttribute('data-access-mode'), 'full');
    // новый выбор поставщика не меняет открытый чат: он остаётся на Claude и в интернете
    await page.locator('[data-chat-model-chip]').click();
    await page.locator('[data-new-chats-line]').waitFor();
    assert.match(await page.locator('[data-new-chats-line]').textContent(), /^Этот чат остаётся на модели «По умолчанию»\./);
    await page.locator('[data-change-provider]').click();
    await page.locator('[data-provider-choice="engine"]').click();
    await page.locator('[data-provider-apply]').click();
    await page.waitForFunction(() => !document.querySelector('[data-provider-dialog]'));
    assert.equal(await page.locator('[data-chat-model-chip][data-executor="claude"]').count(), 1, 'открытый чат — по-прежнему Claude');
    assert.equal(await page.locator('.chat-privacy').textContent(), 'Отвечает Claude, сервис в интернете');
    assert.equal(await page.locator('[data-chat-access]').count(), 1);
    await open('Старый');
    await page.waitForSelector('[data-chat-model-chip][data-executor="claude"]');
    await page.screenshot({ path: join(UX, 'ux-two-chats-1440.png'), animations: 'disabled' });
  }, [], { fake: () => { const engine = new FakeEngine(); engine.seed('window-main', { user: 'Привет', assistant: 'Здравствуйте.' }); engine.seed('chat-old', { user: 'Старый', assistant: 'Ответ' }); engine.access = 'confirm'; return engine; }, viewport: { width: 1440, height: 900 } });
  await check('access-mode-refused', '', async page => {
    await page.locator('[data-chat-access]').click();
    await page.locator('[data-access-option="full"]').click();
    await page.locator('[data-enable-full]').click();
    await page.waitForFunction(() => document.querySelector('[data-link-notice]'));
    assert.equal(await page.locator('[data-chat-access]').getAttribute('data-access-mode'), 'confirm');
    assert.match(await page.locator('[data-link-notice]').textContent(), /Режим доступа не изменён/);
  }, [], { fake: accessEngine({ accessRefuse: true }), viewport: { width: 1440, height: 900 } });
  await check('access-mode-hidden-without-engine-support', '', async (page, engine) => {
    assert.equal(await page.locator('[data-chat-access]').count(), 0);
    assert.equal(await page.locator('[data-chat-permissions]').count(), 1, 'как раньше');
  }, [], { fake: () => { const engine = new FakeEngine(); engine.seed('window-main', { user: 'Привет', assistant: 'Ок' }); return engine; }, viewport: { width: 1440, height: 900 } });
  await check('assistant-text-cannot-change-access', '', async (page, engine) => {
    await page.getByText('Здравствуйте.').waitFor();
    for (const link of await page.locator('[data-markdown] [data-markdown-link], [data-markdown] button').all()) await link.click().catch(() => {});
    await page.getByText('включи полный доступ').first().click().catch(() => {});
    assert.equal(accessCalls(engine).filter(r => r.method.endsWith('/set')).length, 0, 'ни текст, ни ссылки ответа не вызывают set');
    assert.equal(await page.locator('[data-chat-access]').getAttribute('data-access-mode'), 'confirm');
  }, [], { fake: accessEngine(), viewport: { width: 1440, height: 900 } });
  const FOUND = { executor: 'claude', installed: true, version: '2.1.3', versionStatus: 'supported', auth: 'authenticated',
    models: [{ id: 'default', isDefault: true }, { id: 'opus', displayName: 'Opus' }, { id: 'sonnet', displayName: 'Sonnet' }], detail: null };
  const claudeEngine = (setup = {}) => () => {
    const engine = new FakeEngine();
    engine.seed('window-main', { user: 'Привет', assistant: 'Здравствуйте.' });
    engine.access = 'confirm'; engine.claude = FOUND; Object.assign(engine, setup);
    return engine;
  };
  const executorCalls = engine => engine.requests.filter(r => r.method.startsWith('hoh/executor/') && !r.method.includes('/access/'));
  await check('claude-model-in-composer', '', async (page, engine) => {
    const chip = page.locator('[data-chat-model-chip]');
    await chip.waitFor();
    assert.equal(await chip.getAttribute('data-executor'), 'claude');
    assert.equal(await chip.locator('[data-provider-icon]').getAttribute('data-provider-icon'), 'claude', 'значок Claude');
    await page.waitForFunction(() => document.querySelector('[data-chat-model]')?.textContent === 'Opus');
    assert.equal(await page.locator('[data-chat-access]').count(), 1, 'рядом переключатель режима доступа');
    assert.deepEqual(executorCalls(engine).map(r => r.method), ['hoh/executor/discover'], 'имя модели — из selected движка и discover');
    await chip.click();
    await page.locator('[data-model-menu]').waitFor();
    assert.deepEqual(await page.locator('[data-model-option]').evaluateAll(items => items.map(item => item.getAttribute('data-model-option'))).then(ids => ids.sort()), ['default', 'opus', 'sonnet'], 'список — из discover');
    await page.waitForFunction(() => document.querySelectorAll('[data-model-option]').length === 3);
    assert.equal(executorCalls(engine).filter(r => r.method === 'hoh/executor/discover').length, 2, 'меню делает свежий discover');
    await page.screenshot({ path: join(t3shots, 'claude-composer-menu-1440.png'), animations: 'disabled' });
    await page.locator('[data-model-option="sonnet"]').click();
    await page.waitForFunction(() => document.querySelector('[data-chat-model]')?.textContent === 'Sonnet');
    const select = executorCalls(engine).find(r => r.method === 'hoh/executor/select');
    assert.deepEqual(select.params, { executor: 'claude', model: 'sonnet' });
    assert.deepEqual(engine.selected, { executor: 'claude', model: 'sonnet' });
    await page.screenshot({ path: join(t3shots, 'claude-composer-1440.png'), animations: 'disabled' });
  }, [], { fake: claudeEngine({ selected: { executor: 'claude', model: 'opus' } }), viewport: { width: 1440, height: 900 } });
  await check('claude-model-refused', '', async (page, engine) => {
    await page.locator('[data-chat-model-chip]').click();
    await page.locator('[data-model-option="sonnet"]').click();
    await page.waitForFunction(() => document.querySelector('[data-link-notice]'));
    assert.match(await page.locator('[data-link-notice]').textContent(), /Поставщик не изменён/);
    assert.equal(await page.locator('[data-chat-model]').textContent(), 'Opus', 'показан фактический выбор движка');
  }, [], { fake: claudeEngine({ selected: { executor: 'claude', model: 'opus' }, selectRefuse: true }), viewport: { width: 1440, height: 900 } });
  await check('provider-switch-from-the-chat', '', async (page, engine) => {
    const chip = page.locator('[data-chat-model-chip]');
    assert.equal(await chip.getAttribute('data-executor'), null, 'пока модель движка');
    assert.equal(await page.locator('[data-chat-access]').count(), 0, 'режим доступа — только у Claude');
    await chip.click();
    const dialog = page.locator('[data-provider-dialog]');
    await dialog.waitFor();
    assert.equal(await dialog.locator('[data-provider-choice="engine"]').getAttribute('aria-pressed'), 'true');
    assert(await dialog.locator('[data-provider-apply]').isDisabled(), 'ничего не менялось: «Применить» закрыта');
    await dialog.locator('[data-provider-choice="claude"]').click();
    await dialog.getByText('Готово. Ничего вводить не нужно.').waitFor();
    await page.screenshot({ path: join(t3shots, 'claude-settings-1440.png'), animations: 'disabled' });
    await dialog.locator('[data-claude-model]').selectOption('opus');
    await dialog.locator('[data-provider-apply]').click();
    await page.waitForSelector('[data-chat-model-chip][data-executor="claude"]');
    assert.deepEqual(executorCalls(engine).find(r => r.method === 'hoh/executor/select').params, { executor: 'claude', model: 'opus' });
    assert.equal(await page.locator('[data-chat-model]').textContent(), 'Opus');
    assert.equal(await page.locator('[data-chat-access]').count(), 1);
    await page.locator('[data-new-chat]').click();
    await page.locator('[data-chat-input]').fill('новый чат');
    await page.locator('[data-chat-send]').click();
    await page.waitForFunction(() => document.querySelector('[data-chat-entry="assistant"]'));
    assert.equal(engine.promptExecutors.at(-1), 'claude', 'новый чат отправлен, когда выбран Claude');
    await page.locator('[data-chat-model-chip]').click();
    await page.locator('[data-change-provider]').click();
    await page.locator('[data-provider-choice="engine"]').click();
    await page.locator('[data-provider-apply]').click();
    await page.waitForFunction(() => !document.querySelector('[data-provider-dialog]'));
    assert.deepEqual(executorCalls(engine).filter(r => r.method === 'hoh/executor/select').at(-1).params, { executor: null, model: 'default' });
    // The choice is for new chats: the open chat has had a turn through Claude and stays a chat of Claude.
    assert.equal(await page.locator('[data-chat-model-chip][data-executor="claude"]').count(), 1);
    assert.equal(await page.locator('[data-chat-access]').count(), 1);
    assert.equal(await page.locator('.chat-privacy').textContent(), 'Отвечает Claude, сервис в интернете');
    await page.locator('[data-new-chat]').click();
    await page.waitForFunction(() => !document.querySelector('[data-chat-model-chip][data-executor="claude"]'));
    assert.equal(await page.locator('[data-chat-access]').count(), 0, 'новый чат — на модели движка');
  }, [], { fake: claudeEngine({ selected: null }), viewport: { width: 1440, height: 900 } });
  await check('provider-switch-claude-not-ready', '', async (page, engine) => {
    await page.locator('[data-chat-model-chip]').click();
    await page.locator('[data-provider-choice="claude"]').click();
    await page.getByText('Claude установлен, но в него не вошли.').waitFor();
    assert(await page.locator('[data-provider-apply]').isDisabled(), 'без входа Claude применить нельзя');
    assert.match(await page.locator('[data-claude-result]').textContent(), /Откройте Claude, войдите/);
    engine.claude = FOUND;
    await page.getByRole('button', { name: 'Проверить ещё раз' }).click();
    await page.getByText('Готово. Ничего вводить не нужно.').waitFor();
    assert(await page.locator('[data-provider-apply]').isEnabled());
    await page.keyboard.press('Escape');
    assert.equal(await page.locator('[data-provider-dialog]').count(), 0, 'Escape закрывает окно');
    assert.equal(executorCalls(engine).filter(r => r.method === 'hoh/executor/select').length, 0, 'ничего не выбрано');
  }, [], { fake: claudeEngine({ selected: null, claude: { ...FOUND, auth: 'unauthenticated', models: [], detail: 'claude: вход не выполнен' } }), viewport: { width: 1440, height: 900 } });
  await check('emblem-in-the-sidebar-and-sizes', '', async page => {
    const emblem = page.locator('.chat-brand [data-emblem]');
    await emblem.waitFor();
    assert.equal(await page.locator('.chat-brand .mark').count(), 0);
    assert.equal(await page.locator('.chat-brand b').textContent(), 'HoH');
    assert.equal(await emblem.getAttribute('src'), wizardEmblem, 'шапка мастера и угол окна берут знак из одного источника');
    await page.screenshot({ path: join(t3shots, 'emblem-sidebar-1440.png'), animations: 'disabled' });
    for (const size of [16, 20, 24]) {
      await page.evaluate(({ size, src }) => {
        document.getElementById('emblem-probe')?.remove();
        const box = document.createElement('div'); box.id = 'emblem-probe';
        box.style.cssText = 'position:fixed;left:0;top:0;z-index:99;background:#000;padding:8px;line-height:0';
        const img = document.createElement('img'); img.src = src; img.style.height = size + 'px'; box.append(img); document.body.append(box);
      }, { size, src: wizardEmblem });
      await page.locator('#emblem-probe img').evaluate(img => img.decode());
      await page.locator('#emblem-probe').screenshot({ path: join(t3shots, `emblem-${size}.png`), animations: 'disabled' });
    }
  }, [], { fake: claudeEngine({ selected: null }), viewport: { width: 1440, height: 900 } });
  // ───── слова для людей без опыта кода (HOH-35) ─────
  await check('full-access-asks-once-in-a-window', '', async (page, engine) => {
    const chip = page.locator('[data-chat-access]');
    await chip.click();
    await page.locator('[data-access-option="full"]').hover();
    assert.ok((await page.locator('[data-access-menu]').textContent()).includes(FULL), 'подпись полного доступа — слова CEO дословно');
    await page.locator('[data-access-option="full"]').click();
    const dialog = page.locator('[data-full-access-dialog]');
    await dialog.waitFor();
    assert.equal(await dialog.locator('h2').textContent(), 'Включить полный доступ?');
    assert.ok((await dialog.textContent()).includes(FULL), 'окно включения — слова CEO дословно');
    assert.doesNotMatch(await dialog.textContent(), /в папке Помощника|в своей папке/);
    assert.equal(await page.evaluate(() => document.activeElement?.textContent), 'Оставить вопросы', 'выделена и в фокусе «Оставить вопросы»');
    await page.screenshot({ path: join(UX, 'ux-full-access-dialog-1440.png'), animations: 'disabled' });
    await page.locator('[data-keep-questions]').click();
    assert.equal(accessCalls(engine).filter(r => r.method.endsWith('/set')).length, 0, 'без согласия set не вызывается');
    assert.equal(await chip.getAttribute('data-access-mode'), 'confirm');
    await chip.click(); await page.locator('[data-access-option="full"]').click();
    await page.keyboard.press('Escape');
    assert.equal(await page.locator('[data-full-access-dialog]').count(), 0, 'Escape — оставить вопросы');
    assert.equal(accessCalls(engine).filter(r => r.method.endsWith('/set')).length, 0);
    await chip.click(); await page.locator('[data-access-option="full"]').click();
    await page.locator('[data-enable-full]').click();
    await page.waitForFunction(() => document.querySelector('[data-chat-access]')?.getAttribute('data-access-mode') === 'full');
    assert.equal(await chip.locator('.chat-access-text').textContent(), 'Полный доступ: без вопросов', 'включённый режим назван словом');
    await page.screenshot({ path: join(UX, 'ux-full-access-on-1440.png'), animations: 'disabled' });
    const setsBefore = accessCalls(engine).filter(r => r.method.endsWith('/set')).length;
    await chip.click(); await page.locator('[data-access-option="confirm"]').click();
    await page.waitForFunction(() => document.querySelector('[data-chat-access]')?.getAttribute('data-access-mode') === 'confirm');
    assert.equal(await page.locator('[data-full-access-dialog]').count(), 0, 'возврат без окна');
    assert.equal(accessCalls(engine).filter(r => r.method.endsWith('/set')).length, setsBefore + 1);
  }, [], { fake: accessEngine(), viewport: { width: 1440, height: 900 } });
  await check('full-access-word-stays-on-a-narrow-window', '', async page => {
    await page.locator('[data-chat-access]').click();
    await page.locator('[data-access-option="full"]').click(); await page.locator('[data-enable-full]').click();
    await page.waitForFunction(() => document.querySelector('[data-chat-access]')?.getAttribute('data-access-mode') === 'full');
    assert(await page.locator('[data-chat-access] .chat-access-text').isVisible(), 'на узком окне слово видно');
    assert.equal(await page.locator('[data-chat-access] .chat-access-text').textContent(), 'Полный доступ: без вопросов');
    await page.screenshot({ path: join(UX, 'ux-full-access-narrow-480x640.png'), animations: 'disabled' });
  }, [], { fake: accessEngine(), viewport: { width: 480, height: 640 } });
  await check('empty-chat-says-full-access', '', async page => {
    await page.locator('[data-full-access-line]').waitFor();
    assert.equal(await page.locator('[data-full-access-line]').textContent(), 'Включён полный доступ: Помощник не спрашивает разрешения.');
  }, [], { fake: () => { const engine = new FakeEngine(); engine.access = 'full'; return engine; }, viewport: { width: 1440, height: 900 } });
  await check('composer-words-folder-and-permissions', '', async (page, engine) => {
    assert.equal(await page.locator('[data-chat-folder]').textContent(), 'Документы → HoH');
    assert.equal(await page.locator('[data-chat-permissions]').textContent(), 'Спрашивать разрешение');
    assert(!(await page.locator('body').innerText()).includes('Только чтение'));
    await page.screenshot({ path: join(UX, 'ux-composer-1440.png'), animations: 'disabled' });
  }, [], { fake: () => { const engine = new FakeEngine(); engine.cwd = '/home/me/Documents/HoH'; engine.seed('window-main', { user: 'Привет', assistant: 'Ок' }); return engine; }, viewport: { width: 1440, height: 900 } });
  await check('claude-says-project-settings-are-not-applied', '', async page => {
    const line = page.locator('[data-project-line]');
    await line.waitFor();
    assert.equal(await line.textContent(), 'Настройки Claude из папки проекта не применяются');
    assert(await line.isVisible(), 'видимая строка, не подсказка');
    const [privacy, project] = await Promise.all([page.locator('.chat-privacy').boundingBox(), line.boundingBox()]);
    assert(project.y >= privacy.y && project.y - privacy.y < 80, 'строка рядом с «Отвечает Claude»');
    await page.screenshot({ path: join(UX, 'ux-project-line-1440.png'), animations: 'disabled' });
  }, [], { fake: claudeEngine({ settingSources: 'user' }), viewport: { width: 1440, height: 900 } });
  await check('confirm-caption-names-the-persons-claude-settings-only-when-they-apply', '', async page => {
    await page.locator('[data-chat-access]').click();
    await page.locator('[data-access-option="confirm"]').hover();
    assert.equal(await page.locator('.chat-access-preview').textContent(), 'Спрашивать разрешениеПомощник спрашивает перед изменениями. Действуют и ваши собственные настройки Claude: их разрешающие правила могут пропускать действия без вопроса.');
  }, [], { fake: claudeEngine({ settingSources: 'user' }), viewport: { width: 1440, height: 900 } });
  await check('confirm-caption-is-the-first-phrase-when-isolated', '', async page => {
    await page.locator('[data-chat-access]').click();
    await page.locator('[data-access-option="confirm"]').hover();
    assert.equal(await page.locator('.chat-access-preview').textContent(), 'Спрашивать разрешениеПомощник спрашивает перед изменениями.');
  }, [], { fake: claudeEngine({ settingSources: '' }), viewport: { width: 1440, height: 900 } });
  await check('claude-keeps-quiet-when-project-settings-apply', '', async page => {
    await page.locator('[data-chat-model-chip]').waitFor();
    assert.equal(await page.locator('[data-project-line]').count(), 0);
  }, [], { fake: claudeEngine({ settingSources: 'user,project,local' }), viewport: { width: 1440, height: 900 } });
  await check('claude-chat-keeps-its-folder', '', async (page, engine) => {
    await say(page, 'в папке');
    // the folder dialog of the host is a native one: the test hands the picked path to the page (as the folder test below does)
    await page.evaluate(() => { window.__HOH_ENGINE__ = { request: async () => ({}), pickFolder: async () => '/home/me/Другая' }; });
    await page.locator('[data-chat-folder]').click();
    const note = page.locator('.chat-error');
    await note.waitFor();
    assert.match(await note.textContent(), /^Чат с Claude работает в одной папке\. Чтобы работать в другой, начните новый чат\./);
    assert.doesNotMatch(await note.textContent(), /Не удалось сменить папку/);
    await page.screenshot({ path: join(UX, 'ux-folder-fixed-1440.png'), animations: 'disabled' });
    await page.locator('[data-new-chat-folder]').click();
    await page.waitForFunction(() => !document.querySelector('.chat-error'));
    for (let i = 0; i < 100 && engine.cwd !== '/home/me/Другая'; i++) await page.waitForTimeout(50);
    assert.equal(engine.cwd, '/home/me/Другая', 'в новом чате папка принята');
  }, [], { fake: claudeEngine({ folderFixed: true }), viewport: { width: 1440, height: 900 } });
  await check('header-names-who-answers', '', async page => {
    assert.equal(await page.locator('.chat-privacy').textContent(), 'Отвечает Claude, сервис в интернете');
    await page.screenshot({ path: join(UX, 'ux-header-claude-1440.png'), animations: 'disabled' });
  }, [], { fake: claudeEngine({ selected: { executor: 'claude', model: 'opus' } }), viewport: { width: 1440, height: 900 } });
  // ───── Г: ожидание решения и отказ человека в шагах чата (правки 7 и 8), примеры (13) ─────
  const deletion = (...after) => () => {
    const engine = new FakeEngine();
    const main = engine.seed('window-main', { user: 'Убери старые файлы' });
    engine.append(main, 'tool/call', { name: 'bash', callId: 'rm1', arguments: { command: 'rm -f old-1.txt old-2.txt' } });
    for (const [type, data] of after) engine.append(main, type, data);
    return engine;
  };
  await check('step-waits-for-the-protection-window', '', async page => {
    const label = page.locator('[data-step-label]');
    await label.waitFor();
    assert.equal(await label.textContent(), 'Жду вашего решения в окне «Защита HoH»');
    await page.screenshot({ path: join(UX, 'ux-g-waiting-1440.png'), animations: 'disabled' });
  }, [], { fake: deletion(['approval/asked', { id: 'a1', toolName: 'bash', reason: 'удалить' }]), viewport: { width: 1440, height: 900 } });
  await check('step-says-the-person-refused', '', async page => {
    const label = page.locator('[data-step-label]');
    await label.waitFor();
    assert.equal(await label.textContent(), 'Вы не разрешили. Ничего не удалено');
    assert.doesNotMatch(await page.locator('[data-chat-timeline]').innerText().catch(() => page.locator('body').innerText()), /Не удалось выполнить/);
    await page.screenshot({ path: join(UX, 'ux-g-refused-1440.png'), animations: 'disabled' });
  }, [], { fake: deletion(['approval/asked', { id: 'a1', toolName: 'bash' }], ['approval/decided', { id: 'a1', outcome: 'rejected' }],
    ['tool/result', { message: { toolCallId: 'rm1', isError: true, content: [{ type: 'text', text: 'rejected' }] } }]), viewport: { width: 1440, height: 900 } });
  await check('step-after-a-failure-is-still-a-failure', '', async page => {
    await page.locator('[data-step-label]').waitFor();
    assert.equal(await page.locator('[data-step-label]').textContent(), 'Не удалось выполнить', 'без отказа человека — обычная неудача');
  }, [], { fake: deletion(['tool/result', { message: { toolCallId: 'rm1', isError: true, content: [{ type: 'text', text: 'boom' }] } }]), viewport: { width: 1440, height: 900 } });
  await check('every-example-gives-a-turn-not-an-error', '', async (page, engine) => {
    const { EXAMPLES } = await import('../../src/lib/examples.mjs');
    for (const example of EXAMPLES) {
      await page.locator('[data-new-chat]').click();
      await page.locator(`[data-chat-example="${example.id}"]`).click();
      assert.equal(await page.locator('[data-chat-input]').inputValue(), example.text);
      await page.locator('[data-chat-send]').click();
      await page.waitForFunction(() => document.querySelector('[data-chat-entry="assistant"]'));
      assert.equal(await page.locator('[data-chat-error], .chat-error').count(), 0, `${example.id}: без ошибки`);
    }
    assert.equal(engine.requests.filter(r => r.method === 'session/prompt').length, EXAMPLES.length);
  }, [], { fake: () => new FakeEngine(), viewport: { width: 1440, height: 900 } });
  await check('no-engine-says-so', '', async page => {
    assert.match(await page.locator('.chat-privacy').textContent(), /Нет связи: не знаю, где обрабатываются данные/);
    assert.equal(await page.getByText('Проверяю, где обрабатываются данные').count(), 0, 'не «проверяю» навсегда');
  }, [], { fake: () => new FakeEngine({ unavailable: ['session/working-directory/get'] }), connection: 'error', viewport: { width: 1440, height: 900 }, shot: 'p2-no-engine-1440.png' });
  await check('composer-line', '', async (page, engine) => {
    await page.locator('[data-chat-folder]').waitFor();
    assert.equal(await page.getByText(/Enter отправляет|Shift\+Enter/).count(), 0, 'подсказки про Enter нет');
    assert.equal(await page.getByRole('button', { name: 'Стоп', exact: true }).count(), 0, 'отдельной кнопки «Стоп» нет');
    assert.equal(await page.locator('[data-chat-permissions]').textContent(), 'Спрашивать разрешение');
    assert.equal(await page.locator('[data-chat-model]').textContent(), 'Модель на этом компьютере');
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
    // правка 19: Enter во время ответа не молчит
    await page.locator('[data-chat-input]').press('Enter');
    assert.equal(await page.locator('[data-send-hint]').textContent(), 'Помощник ещё отвечает. Дождитесь ответа или нажмите «Стоп».');
    await page.screenshot({ path: join(UX, 'ux-send-hint-1440.png'), animations: 'disabled' });
    await page.getByRole('button', { name: 'Стоп', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('[data-chat-status]')?.textContent === 'Можно написать сообщение');
    // правка 9: остановленный ответ помечен
    assert.equal(await page.locator('[data-stopped-note]').textContent(), 'Вы остановили ответ. Уже сделанное не отменяется.');
    await page.waitForFunction(() => !document.querySelector('[data-send-hint]'));
    await page.screenshot({ path: join(UX, 'ux-stopped-1440.png'), animations: 'disabled' });
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
