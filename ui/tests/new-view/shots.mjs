// Снимки всех экранов нового вида (docs/window-screens.md) в двух размерах: 1440×900 и 800×640.
// Результат — ui/test-results/new-view/. Страницы берутся из ui/dist; движок подменён (FakeEngine, подмена мастера).
// Запуск: /usr/bin/env -i /usr/bin/python3 -I scripts/offline.py node tests/new-view/shots.mjs  (после pnpm -C ui build:offline).
import { chromium } from 'playwright-core';
import { mkdir, mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { FakeEngine } from '../chat/fake-engine.mjs';
import { approvalRequest } from '../screens-a/fixtures.mjs';

const ui = resolve(import.meta.dirname, '../..');
const out = join(ui, 'test-results/new-view');
await mkdir(out, { recursive: true });
const sizes = [['1440', { width: 1440, height: 900 }], ['800x640', { width: 800, height: 640 }]];
const home = await mkdtemp(join(tmpdir(), 'hoh-newview-'));
const browser = await chromium.launch({
  executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
  env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
  args: ['--allow-file-access-from-files', '--no-sandbox', '--disable-gpu', '--disable-background-networking', '--no-first-run', '--force-color-profile=srgb'],
});
const written = [];
const errors = [];
async function newPage(viewport) {
  const context = await browser.newContext({ viewport, deviceScaleFactor: 1, reducedMotion: 'reduce', locale: 'ru-RU' });
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/*', route => route.request().url().startsWith('file://') ? route.continue() : route.abort());
  return { page, context };
}
async function shot(page, name, size) {
  await page.evaluate(() => document.fonts.ready); await page.mouse.move(0, 0);
  const file = `${name}-${size}.png`;
  await page.screenshot({ path: join(out, file), animations: 'disabled' }); written.push(file);
}
const dist = file => pathToFileURL(join(ui, 'dist', file)).href;

function seededEngine() {
  const engine = new FakeEngine();
  const main = engine.seed('window-main', { user: 'Прочитай заметки и составь план недели' });
  engine.append(main, 'tool/call', { name: 'read_file', callId: 'c1', arguments: { path: 'заметки.md' } });
  engine.append(main, 'tool/result', { message: { toolCallId: 'c1', content: [{ type: 'text', text: '     1  Позвонить в банк\n     2  Купить билеты' }] } });
  engine.append(main, 'tool/call', { name: 'bash', callId: 'c2', arguments: { command: 'ls' } });
  engine.append(main, 'tool/result', { message: { toolCallId: 'c2', content: [{ type: 'text', text: 'plan.md\nzametki.md' }] } });
  engine.cwd = '/home/me/Проекты/сайт';
  engine.append(main, 'tool/call', { name: 'write_file', callId: 'c3', arguments: { path: '../итог.txt', content: 'x' } });
  engine.append(main, 'tool/result', { message: { toolCallId: 'c3', isError: true, content: [{ type: 'text', text: '[sandbox: file access denied under workspace-write mode]' }] } });
  engine.append(main, 'assistant/message', { message: { id: 'a1', content: [{ type: 'text', text: 'План готов: **три дела** на понедельник и два на среду.\n\n1. Позвонить в банк — см. [заметки](zametki.md:1), [чужой файл](../итог.txt)\n2. Купить билеты — [сайт](https://example.com/tickets)\n\n![схема недели](https://example.com/week.png)\n\nИтог записан в `plan.md`.\n\nПлощадь: $S=\\pi r^2$.\n\n| День | Дел |\n|---|---|\n| Пн | 3 |\n| Ср | 2 |' }] } });
  engine.seed('chat-2', { title: 'Отчёт по расходам', user: 'Отчёт по расходам', assistant: 'Готово.' });
  engine.seed('chat-3', { title: 'Письмо в налоговую', user: 'Письмо в налоговую', assistant: 'Черновик готов.' });
  engine.seed('chat-4', { title: 'Сравнить два договора', user: 'Сравнить два договора', assistant: 'Различия перечислены.' });
  return engine;
}
async function openChat(viewport, { engine = seededEngine(), history, failAll = false } = {}) {
  const { page, context } = await newPage(viewport);
  await page.exposeBinding('__hohChatRequest', (_, method, params) => { if (failAll) throw new Error('offline'); return engine.request(method, params); });
  await page.exposeBinding('__hohPickFolder', () => '/home/me/Документы/Проекты/сайт');
  await page.addInitScript(({ history }) => {
    localStorage.setItem('hoh:first-run:done', '1');
    const listeners = new Set();
    window.__HOH_CHAT_TEST_TRANSPORT__ = {
      request: (m, p) => window.__hohChatRequest(m, p),
      subscribe: async l => { listeners.add(l); return () => listeners.delete(l); },
      subscribeStatus: async () => () => {},
      subscribeQuestions: async () => () => {},
      answerQuestion: async () => {},
      approvalHistory: async () => history ?? [],
      openLink: async () => {},
    };
    window.__HOH_ENGINE__ = { request: (m, p) => window.__hohChatRequest(m, p), pickFolder: () => window.__hohPickFolder() };
    window.__hohChatNotify = n => { for (const l of listeners) l(n); };
  }, { history });
  engine.subscribe(n => page.evaluate(value => window.__hohChatNotify(value), n).catch(() => {}));
  await page.goto(dist('legacy.html')); // прежняя оболочка (HOH-40: index.html — веб-клиент T3)
  return { page, context, engine };
}

for (const [size, viewport] of sizes) {
  // чат с ответом, ссылками, картинкой и шагами; в той же колонке — список чатов
  {
    const { page, context } = await openChat(viewport);
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
    await page.getByText('План готов').waitFor();
    await page.waitForSelector('[data-math="inline"] .katex', { timeout: 5000 }).catch(() => undefined); // формула рисуется, когда включён рендер T3
    await page.locator('[data-chat-folder]').waitFor();
    await shot(page, 'chat', size);
    // список чатов: раскрыт шаг и открыт другой чат
    await page.locator('[data-operation="write"] summary').first().click();
    await page.locator('[data-operation="read"] summary').first().click();
    await shot(page, 'chat-steps-open', size);
    if (size !== '1440') await page.locator('[data-chat-menu]').click();
    await shot(page, 'chat-list', size);
    await page.locator('[data-chat-id="chat-2"]').click();
    // выбор папки (кнопка в нижней строке поля ввода → системный выбор → новое название)
    await page.locator('[data-chat-folder]').click();
    await page.waitForFunction(() => document.querySelector('[data-chat-folder]')?.textContent === 'сайт');
    await shot(page, 'folder-chat', size);
    await context.close();
  }
  // «Подтверждения» в главном окне
  {
    const history = [
      { at: Date.UTC(2026, 10, 19, 18, 4), asker: 'assistant', method: 'bash', outcome: 'rejected', humanText: 'Выполнить команду в папке Документы/HoH: удалить 2 файла', reason: 'Эти файлы старые', details: 'Команда: rm -f old-1.txt old-2.txt' },
      { at: Date.UTC(2026, 10, 19, 18, 1), asker: 'you', method: 'hoh/settings/set', outcome: 'allowed-once', humanText: 'Изменить настройки: провайдер — «https://user:•••@host.example/v1?api_key=•••»' },
    ];
    const { page, context } = await openChat(viewport, { history });
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
    if (size !== '1440') await page.locator('[data-chat-menu]').click();
    await page.locator('[data-nav-approvals]').click();
    await page.locator('[data-approvals]').waitFor();
    await shot(page, 'approvals', size);
    await context.close();
  }
  // статус: нет связи
  {
    const { page, context } = await openChat(viewport, { failAll: true });
    await page.waitForSelector('[data-chat-screen][data-connection="error"]');
    await shot(page, 'status-no-connection', size);
    await context.close();
  }
  // статус: Помощник работает (идёт ответ, круглая кнопка стала «Стоп»)
  {
    const { page, context, engine } = await openChat(viewport);
    await page.waitForSelector('[data-chat-screen][data-connection="ready"]');
    await page.getByLabel('Сообщение Помощнику').fill('Собери всё в одну таблицу');
    engine.answer = () => {}; // ответ не придёт: снимок во время работы
    await page.locator('[data-chat-send]').click();
    await page.locator('[data-chat-stop]').waitFor();
    await shot(page, 'status-working', size);
    await context.close();
  }
  // мастер подключения: три шага
  {
    const { page, context } = await newPage(viewport);
    await page.exposeBinding('__hohWizard', (_, method) => {
      if (method === 'hoh/settings/get') return { model: { baseUrl: 'http://localhost:8000/v1', model: 'local' }, security: {}, view: {} };
      if (method === 'hoh/model/probe') return { ok: true, latencyMs: 2100, toolCalls: true, models: ['local', 'other'] };
      return {};
    });
    await page.exposeBinding('__hohFolder', () => 'Документы/Проекты');
    await page.addInitScript(() => { window.__HOH_ENGINE__ = { request: (m, p) => window.__hohWizard(m, p), saveRules: async () => {}, pickFolder: () => window.__hohFolder() }; });
    await page.goto(dist('firstrun.html'));
    await page.getByRole('heading', { name: 'Подключите модель' }).waitFor();
    await shot(page, 'wizard-1-model-start', size);
    await page.getByRole('button', { name: 'Проверить' }).click();
    await page.getByText(/Отвечает за/).waitFor();
    await shot(page, 'wizard-1-model', size);
    await page.getByRole('button', { name: 'Дальше' }).click();
    await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
    await shot(page, 'wizard-2-rules', size);
    await page.getByRole('button', { name: 'Выбрать' }).click();
    await page.locator('.fr-path', { hasText: 'Документы/Проекты' }).waitFor();
    await shot(page, 'folder-wizard', size);
    await page.getByRole('button', { name: 'Дальше' }).click();
    await page.getByRole('heading', { name: 'Я ваш Помощник.' }).waitFor();
    await shot(page, 'wizard-3-intro', size);
    await context.close();
  }
  // отдельное окно защиты
  {
    const { page, context } = await newPage(viewport);
    await page.exposeBinding('__hohDecide', () => {});
    await page.addInitScript(`window.__HOH_APPROVAL__ = { current: async () => (${JSON.stringify(approvalRequest())}), decide: (id, outcome) => window.__hohDecide(id, outcome) };`);
    await page.goto(dist('approval.html'));
    await page.getByRole('alertdialog').waitFor();
    await page.waitForTimeout(1200);
    await shot(page, 'guard-window', size);
    await context.close();
  }
}
await browser.close();
assert.deepEqual(errors, [], 'исключения страниц');
console.log(`new-view: ${written.length} снимков в ui/test-results/new-view/\n` + written.join('\n'));
