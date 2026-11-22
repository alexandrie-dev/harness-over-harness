// Проверки первого запуска и окна подтверждения (вариант A) в headless Chromium.
// Страницы — из ui/dist (firstrun.html, approval.html) или из HOH_FIRSTRUN_PAGE / HOH_APPROVAL_PAGE. firstrun.html окну не
// нужна и собирается только для проверок: `vite build --mode probe` (ревью 222000).
// Движок: если задан HOH_WINDOW_STUB — настоящая заглушка hoh-window-stub по stdio, иначе подмена в этом файле.
// Снимки 1440×900 — в ui/test-results/screens-a/ (design/ окно не меняет). Ничего не ходит в сеть: всё кроме file:// блокируется.
import { chromium } from 'playwright-core';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { startStub } from './stub-engine.mjs';
import { approvalRequest as request } from './fixtures.mjs';

const ui = process.env.HOH_UI_ROOT ?? resolve(import.meta.dirname, '../..');
const shots = resolve(ui, 'test-results/screens-a');
const t3shots = resolve(ui, 'test-results/t3-exact');
const pages = {
  firstrun: resolve(process.env.HOH_FIRSTRUN_PAGE ?? join(ui, 'dist/firstrun.html')),
  approval: resolve(process.env.HOH_APPROVAL_PAGE ?? join(ui, 'dist/approval.html')),
};
await mkdir(shots, { recursive: true });
await mkdir(t3shots, { recursive: true });
const home = await mkdtemp(join(tmpdir(), 'hoh-chromium-'));
const claudeFile = join(home, 'claude-state');
await writeFile(claudeFile, 'found');
const stub = process.env.HOH_WINDOW_STUB ? await startStub(process.env.HOH_WINDOW_STUB, { env: { HOH_STUB_CLAUDE_FILE: claudeFile } }) : null;
// Вторая заглушка: окно сообщает userQuestions, и в режиме «с подтверждением» Claude присылает вопрос.
const askingStub = process.env.HOH_WINDOW_STUB ? await startStub(process.env.HOH_WINDOW_STUB, { capabilities: { userQuestions: true } }) : null;
const browser = await chromium.launch({
  executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
  env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
  args: ['--allow-file-access-from-files', '--no-sandbox', '--disable-gpu', '--disable-background-networking', '--no-first-run', '--force-color-profile=srgb'],
});
let passed = 0;
try {
const ok = name => { passed++; console.log('passed: ' + name); };

async function open(kind, { viewport = { width: 1440, height: 900 }, init, hook } = {}) {
  const context = await browser.newContext({ viewport, deviceScaleFactor: 1, reducedMotion: 'reduce', locale: 'ru-RU' });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  const outside = [];
  await page.route('**/*', route => {
    if (route.request().url().startsWith('file://')) return route.continue();
    outside.push(route.request().url());
    return route.abort();
  });
  if (hook) await page.exposeFunction('__hoh_call', hook);
  if (init) await page.addInitScript(init);
  await page.goto(pathToFileURL(pages[kind]).href);
  await page.evaluate(() => document.fonts.ready);
  return { page, context, errors, outside };
}

// ───── первый запуск ─────
const engineCalls = [];
let refuse = null; let unanswered = false;
// What `engine_request` of the host rejects with: the object of apps/hoh-desktop/src/errors.rs (`HostError`), the same
// words and codes as its Rust test asserts. The page gets exactly this, not an Error with invented words.
const HOST = {
  rejected: { code: 'rejected', message: 'Действие отклонено' },
  expired: { code: 'expired', message: 'Подтверждение не получено' },
  refused: { code: 'refused', message: 'Помощник отклонил запрос' },
  failed: { code: 'failed', message: '' },
};
// Подмена без заглушки: те же четыре ответа discover, что у hoh-window-stub.
const MODELS = [{ id: 'default', isDefault: true }, { id: 'opus', displayName: 'Opus' }];
const CLAUDE = {
  found: { executor: 'claude', installed: true, version: '2.1.3', versionStatus: 'supported', auth: 'authenticated', models: MODELS, detail: null },
  noauth: { executor: 'claude', installed: true, version: '2.1.3', versionStatus: 'supported', auth: 'unauthenticated', models: [], detail: 'claude: вход не выполнен' },
  missing: { executor: 'claude', installed: false, version: null, versionStatus: 'unknown', auth: 'unknown', models: [], detail: 'claude: команда не найдена' },
  unsupported: { executor: 'claude', installed: true, version: '1.0.0', versionStatus: 'unsupported', auth: 'unknown', models: [], detail: 'Версия claude 1.0.0 не поддерживается' },
  unknown: { executor: 'claude', installed: true, version: null, versionStatus: 'unknown', auth: 'unknown', models: [], detail: 'Версию claude не удалось определить' },
};
let claudeState = 'found';
let accessSources = 'user'; // `--setting-sources` of the next start of Claude (access/get of the fake engine)
const setClaude = async state => { claudeState = state; await writeFile(claudeFile, state); };
const fake = async (method, params) => {
  engineCalls.push({ method, params });
  if (refuse && refuse.method === method) return { __hostError: refuse.error };
  if (unanswered && method === 'hoh/model/probe') return { ok: false, latencyMs: 0, toolCalls: false };
  if (stub) return stub.request(method, params);
  if (method === 'hoh/executor/discover') return CLAUDE[claudeState];
  if (method === 'hoh/executor/access/get') return { executor: 'claude', mode: 'confirm', effective: { mode: 'confirm', flags: ['--permission-mode', 'default', '--setting-sources', accessSources, '--permission-prompt-tool', 'stdio'], text: 'С подтверждением' }, selected: null };
  if (method === 'hoh/executor/select') return { executor: params.executor, model: params.model };
  if (method === 'hoh/settings/get') return { model: { baseUrl: 'http://127.0.0.1:8000', model: 'local' }, security: {}, view: {} };
  if (method === 'session/working-directory/get') return { cwd: '/home/me/Documents/HoH' };
  if (method === 'hoh/model/probe') return { ok: true, latencyMs: 2100, toolCalls: true, models: ['local', 'other'] };
  if (method === 'hoh/settings/set' || method === 'hoh/settings/rules/set' || method === 'hoh/model/key/set') return {};
  throw new Error('нет такого метода');
};
const SECRET = 'sk-test-SECRET-4242';
const keys = [];
const rules = [];
const firstrunHook = async (kind, a, b) => {
  if (kind === 'request') return fake(a, b);
  if (kind === 'storeApiKey') { keys.push(a); return; }
  if (kind === 'saveRules') { rules.push(a); return; }
  if (kind === 'pickFolder') return 'Документы/Проекты';
};
const withExtras = () => {
  window.__HOH_ENGINE__ = {
    // Tauri rejects the command's promise with the serialized `HostError` itself (an object), not with an Error.
    request: async (m, p) => { const r = await window.__hoh_call('request', m, p); if (r && r.__hostError) throw r.__hostError; return r; },
    storeApiKey: k => window.__hoh_call('storeApiKey', k),
    saveRules: r => window.__hoh_call('saveRules', r),
    pickFolder: () => window.__hoh_call('pickFolder'),
  };
};
// The production path: the key goes out as the params of `hoh/model/key/set` through `request`,
// there is no special key hook to hide behind.
const realPath = () => {
  window.__HOH_ENGINE__ = {
    request: async (m, p) => { const r = await window.__hoh_call('request', m, p); if (r && r.__hostError) throw r.__hostError; return r; },
    saveRules: r => window.__hoh_call('saveRules', r),
    pickFolder: () => window.__hoh_call('pickFolder'),
  };
};
const requestOnly = () => { window.__HOH_ENGINE__ = { request: async (m, p) => { const r = await window.__hoh_call('request', m, p); if (r && r.__hostError) throw r.__hostError; return r; } }; };

{
  const { page, context, errors, outside } = await open('firstrun', { init: realPath, hook: firstrunHook });
  const next = page.getByRole('button', { name: 'Дальше' });
  await page.getByRole('heading', { name: 'Кто будет отвечать' }).waitFor();
  assert.equal(await page.getByLabel('Название модели').isHidden(), true, 'ручное имя только в закрытом «Дополнительно»');
  assert.equal(await page.locator('.fr-advanced').getAttribute('open'), null);
  assert(await next.isEnabled(), '«Дальше» открыта: она сама проверяет');
  assert.equal(await page.getByRole('button', { name: 'Проверить', exact: true }).count(), 0, 'отдельной кнопки «Проверить» нет');
  assert.equal(await page.getByLabel('Ключ доступа').count(), 0, 'для модели на этом компьютере ключа нет');
  await page.screenshot({ path: join(t3shots, 'ux-g-wizard-model-1440.png'), animations: 'disabled' });
  // правка 12: слова первого экрана — по словарю отчёта
  const firstScreen = await page.locator('body').innerText();
  for (const word of [/языковая модель/i, /API/, /\/v1/, /запущена/, /\bмс\b/, /провайдер/i, /Облачный сервис/]) assert.doesNotMatch(firstScreen, word, `на первом экране мастера есть ${word}`);
  for (const phrase of ['Кто будет отвечать', 'Помощнику нужна программа, которая понимает текст и отвечает.', 'Модель на этом компьютере', 'Сервис в интернете (по ключу)']) assert(firstScreen.includes(phrase), phrase);
  ok('шаг 1: «Дальше» одна и сама проверяет, ключа у локальной модели нет, слова первого экрана по словарю');
  await page.locator('.fr-advanced summary').click();

  // P2: слова о том, куда уходят данные, не противоречат подписи исполнителя
  const addressHelp = page.locator('.field', { has: page.getByLabel('Адрес') }).locator('.help');
  assert.match(await addressHelp.innerText(), /данные никуда не уходят/, 'этот компьютер');
  await page.getByLabel('Адрес').fill('http://10.99.13.1:8000/v1');
  assert.match(await page.locator('[data-provider-line]').innerText(), /Модель на другом компьютере в вашей сети/);
  assert.doesNotMatch(await addressHelp.innerText(), /никуда не уходят/, 'частная сеть: данные уходят с этого компьютера');
  assert.match(await addressHelp.innerText(), /в вашей сети/);
  await page.getByLabel('Адрес').fill('http://localhost:8000/v1');
  // облачный режим: ключ обязателен, уходит только движку и не остаётся в окне
  await page.getByRole('button', { name: 'Сервис в интернете' }).click();
  assert.equal(await page.getByLabel('Адрес').inputValue(), 'https://');
  assert.equal(await page.getByLabel('Ключ доступа').getAttribute('type'), 'password');
  assert.match(await page.locator('[data-key-note]').innerText(), /Ключ хранится в файле на этом компьютере\. Читать его может только ваша учётная запись\./, 'фраза о хранении ключа уже при вводе');
  const providerLine = page.locator('[data-provider-line]');
  assert.equal(await providerLine.getAttribute('data-provider'), 'unknown', 'неизвестный адрес — нейтральный значок');
  await page.getByLabel('Адрес').fill('https://api.openai.com/v1');
  assert.equal(await providerLine.getAttribute('data-provider'), 'openai');
  assert.equal(await providerLine.locator('[data-provider-icon]').getAttribute('data-provider-icon'), 'openai');
  assert.match(await providerLine.innerText(), /OpenAI/);
  assert.doesNotMatch(await providerLine.innerText(), /local/i, 'имя модели по умолчанию «local» не стоит у облачного сервиса');
  await page.getByLabel('Адрес').fill('http://192.168.1.9:8000/v1');
  assert.equal(await providerLine.getAttribute('data-provider'), 'local');
  assert.match(await providerLine.innerText(), /Модель на другом компьютере в вашей сети/);
  await page.getByLabel('Адрес').fill('https://api.example.test/v1');
  await next.click();
  await page.getByText('Для облачного сервиса нужен ключ доступа').waitFor();
  await page.getByLabel('Адрес').fill('https://api.example.test/v1');
  await page.getByLabel('Ключ доступа').fill(SECRET);
  await next.click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  // Ключ неизбежно есть в параметрах key/set (так он попадает в хранилище движка) и больше нигде.
  const keySets = engineCalls.filter(call => call.method === 'hoh/model/key/set');
  assert.equal(keySets.length, 1, 'key/set вызван ровно один раз');
  assert.deepEqual(keySets[0].params, { apiKey: SECRET, baseUrl: 'https://api.example.test/v1' });
  assert(!JSON.stringify(engineCalls.filter(call => call.method !== 'hoh/model/key/set')).includes(SECRET), 'в других запросах ключа нет');
  await page.getByRole('button', { name: 'Назад' }).click();
  await page.getByRole('heading', { name: 'Кто будет отвечать' }).waitFor();
  assert.equal(await page.getByLabel('Ключ доступа').inputValue(), '', 'после сохранения поле ключа очищено');
  assert(!(await page.content()).includes(SECRET), 'ключа нет в разметке страницы');
  const stored = await page.evaluate(() => JSON.stringify([{ ...localStorage }, { ...sessionStorage }]));
  assert(!stored.includes(SECRET), 'ключа нет в localStorage и sessionStorage');
  const keyNote = await page.locator('[data-key-note]').innerText();
  assert.match(keyNote, /Ключ хранится в файле на этом компьютере\. Читать его может только ваша учётная запись\.$/);
  assert(!keyNote.includes(SECRET), 'в пояснении ключа самого ключа нет');
  ok('облачный режим: ключ уходит движку один раз (key/set), в окне, хранилищах и других запросах его нет');

  // правка адреса снимает проверку
  await page.getByLabel('Адрес').fill('https://api.example.test/v2');
  await page.getByRole('button', { name: 'Модель на этом компьютере' }).click();
  assert.equal(await page.getByLabel('Адрес').inputValue(), 'http://localhost:8000/v1');
  await page.screenshot({ path: join(shots, 'firstrun-1440.png'), animations: 'disabled' });
  await next.click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  ok('локальный режим: «Дальше» проверяет и сохраняет; снимок firstrun-1440');

  assert.equal(await page.evaluate(() => document.activeElement?.id), 'fr-h', 'фокус на заголовке нового шага');
  assert(engineCalls.some(c => c.method === 'hoh/settings/set' && c.params.model.model === 'local'), 'модель сохранена через settings/set');
  assert.equal(await page.locator('.fr-path').innerText(), stub ? '/tmp' : '/home/me/Documents/HoH', 'по умолчанию показан настоящий путь, не перевод');
  await page.getByRole('radio', { name: /Читать открытые сайты самому/ }).check();
  await page.getByRole('button', { name: 'Выбрать' }).click();
  await page.locator('.fr-path', { hasText: 'Документы/Проекты' }).waitFor();
  await page.getByRole('radio', { name: /Спрашивать про каждый новый сайт/ }).check();
  await page.screenshot({ path: join(shots, 'firstrun-2-1440.png'), animations: 'disabled' });
  await page.getByRole('button', { name: 'Назад' }).click();
  await page.getByRole('heading', { name: 'Кто будет отвечать' }).waitFor();
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  assert.equal(await page.locator('.fr-stepper-item').count(), 2, 'шага «Знакомство» нет');
  await page.getByRole('button', { name: 'Открыть чат' }).click();
  assert.equal(await page.evaluate(() => document.body.dataset.finished), '', 'мастер закончен: открывается чат, без отдельной страницы «Знакомство»');
  assert.deepEqual(rules.at(-1), { internet: 'ask', folder: 'Документы/Проекты' });
  assert.deepEqual(errors, []);
  assert.deepEqual(outside, [], 'страница не обращалась наружу');
  await context.close();
  ok('шаг 2 — последний: правила сохранены, «Назад» возвращает, «Открыть чат» завершает без ошибок страницы и внешних запросов');
}

{
  // The rules method is now in the protocol. Simulate refusal of a non-default
  // policy, while still accepting default rules; the UI must not lose the refusal.
  const { page, context } = await open('firstrun', { init: requestOnly, hook: async (kind, method, params) => {
    if (kind === 'request' && method === 'hoh/settings/rules/set' && params.internet !== 'ask') throw new Error('Нестандартные правила недоступны');
    return firstrunHook(kind, method, params);
  } });
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  await page.getByRole('radio', { name: /Читать открытые сайты самому/ }).check();
  await page.getByRole('button', { name: 'Выбрать' }).click();
  await page.getByText(/Выбор папки пока недоступен/).waitFor();
  await page.getByRole('button', { name: 'Открыть чат' }).click();
  await page.getByRole('alert').getByText(/Не удалось сохранить/).waitFor();
  assert.equal(await page.locator('[data-screen=firstrun]').getAttribute('data-step'), '2', 'остались на шаге 2');
  await page.getByRole('radio', { name: /Спрашивать про каждый новый сайт/ }).check();
  await page.getByRole('button', { name: 'Открыть чат' }).click();
  await page.waitForFunction(() => document.body.dataset.finished === '');
  await context.close();
  ok('сохранение правил: отказ для особого выбора не теряется молча, обычный сохраняется');
}

{
  const { page, context } = await open('firstrun', { init: requestOnly, hook: async (kind, m) => { if (m === 'hoh/model/probe') throw new Error('Модель недоступна'); return firstrunHook(kind, m); } });
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByText('Не удалось проверить. Модель недоступна').waitFor();
  assert.equal(await page.locator('[data-screen=firstrun]').getAttribute('data-step'), '1', 'остались на шаге «Кто будет отвечать»');
  await context.close();
  ok('сбой проверки: понятный текст, шаг не пройден');
}

{
  // 900×700 и узкое окно: ничего не обрезается
  const { page, context } = await open('firstrun', { viewport: { width: 520, height: 800 }, init: requestOnly, hook: firstrunHook });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
  assert.equal(overflow, false, 'нет горизонтальной прокрутки при ширине 520');
  await context.close();
  ok('узкое окно 520: нет горизонтальной прокрутки');
}

// Регрессии хранения ключа и блокировки сохранения. Только внешний прогон.
{
  let fail = true; const saved = [];
  const { page, context } = await open('firstrun', { init: realPath, hook: async (kind, a, b) => {
    if (kind === 'request' && a === 'hoh/model/key/set') { saved.push(b.apiKey); if (fail) throw new Error('Сохранение недоступно'); return {}; }
    return firstrunHook(kind, a, b);
  } });
  await page.getByRole('button', { name: 'Сервис в интернете' }).click();
  await page.getByLabel('Адрес').fill('https://api.example.test/v1');
  await page.getByLabel('Ключ доступа').fill(SECRET);
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByText(/Не удалось сохранить ключ/).waitFor();
  assert.equal(await page.getByLabel('Ключ доступа').inputValue(), SECRET);
  assert(!(await page.content()).includes(SECRET));
  fail = false; await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  await page.getByRole('button', { name: 'Назад' }).click();
  assert.equal(await page.getByLabel('Ключ доступа').inputValue(), '');
  assert.deepEqual(saved, [SECRET, SECRET]);
  await context.close(); ok('ошибка сохранения ключа: поле остаётся для повтора, успех очищает его');
}
{
  let releaseModel, releaseRules;
  const modelSave = new Promise(resolve => { releaseModel = resolve; });
  const rulesSave = new Promise(resolve => { releaseRules = resolve; });
  const { page, context } = await open('firstrun', { init: withExtras, hook: async (kind, a, b) => {
    if (kind === 'request' && a === 'hoh/settings/set') { await modelSave; return {}; }
    if (kind === 'saveRules') { await rulesSave; return; }
    return firstrunHook(kind, a, b);
  } });
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.waitForFunction(() => document.querySelector('fieldset.fr-card')?.disabled);
  assert(await page.getByLabel('Адрес').isDisabled()); assert(await page.getByLabel('Название модели').isDisabled());
  assert(await page.getByRole('button', { name: 'Назад', includeHidden: true }).isDisabled()); assert(await page.getByRole('button', { name: 'Дальше' }).isDisabled());
  releaseModel(); await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  await page.getByRole('button', { name: 'Открыть чат' }).click();
  assert(await page.getByRole('radio', { name: /Читать открытые сайты самому/ }).isDisabled());
  assert(await page.getByRole('button', { name: 'Выбрать' }).isDisabled());
  assert(await page.getByRole('button', { name: 'Назад', includeHidden: true }).isDisabled()); assert(await page.getByRole('button', { name: 'Открыть чат' }).isDisabled());
  releaseRules(); await page.waitForFunction(() => document.body.dataset.finished === '');
  await context.close(); ok('сохранение модели и правил блокирует поля, «Назад» и «Дальше»');
}

// ───── Claude как поставщик в мастере ─────
{
  const { page, context, errors, outside } = await open('firstrun', { init: realPath, hook: firstrunHook });
  await setClaude('found');
  const next = page.getByRole('button', { name: 'Дальше' });
  const result = page.locator('[data-claude-result]');
  // Готовый Claude кнопки «Проверить» не показывает: чтобы проверить заново, выбор делается снова.
  const recheck = async () => { await page.getByRole('button', { name: 'Модель на этом компьютере' }).click(); await page.locator('[data-mode-claude]').click(); };
  const discovers = () => engineCalls.filter(call => call.method === 'hoh/executor/discover');
  await page.getByRole('heading', { name: 'Кто будет отвечать' }).waitFor();
  const before = discovers().length;
  await page.locator('[data-mode-claude]').click();
  await page.getByText('Готово. Ничего вводить не нужно.').waitFor();
  assert.equal(discovers().length, before + 1, 'выбор Claude делает один настоящий discover');
  assert.deepEqual(discovers().at(-1).params, { executor: 'claude' });
  assert.equal(await page.locator('[data-mode-claude] [data-provider-icon]').getAttribute('data-provider-icon'), 'claude', 'значок Claude у варианта');
  assert.equal(await page.getByLabel('Адрес').count(), 0, 'адреса у Claude нет');
  assert.equal(await page.getByLabel('Ключ доступа').count(), 0, 'ключа Claude в окне нет');
  assert.match(await page.locator('[data-claude-version]').innerText(), /версия 2\.1\.3/);
  assert.equal(await page.locator('[data-claude-warning]').count(), 0, 'найден и вошёл: ни подсказки, ни предупреждения');
  assert(await next.isEnabled(), 'найден и вход выполнен: «Дальше» открыта');
  assert.deepEqual(await page.locator('[data-claude-model] option').allInnerTexts(), ['По умолчанию', 'Opus'], 'модели только из discover');
  await page.screenshot({ path: join(t3shots, 'claude-found-1440.png'), animations: 'disabled' });
  ok('Claude найден и вход выполнен: версия, модели из discover, «Дальше» открыта, адреса и ключа нет');

  await setClaude('noauth');
  await recheck();
  await page.getByText('Claude установлен, но в него не вошли.').waitFor();
  assert.equal(await result.innerText(), 'Claude установлен, но в него не вошли. Откройте Claude, войдите и нажмите «Проверить ещё раз».');
  assert.equal(await page.getByRole('button', { name: 'Проверить ещё раз' }).count(), 1, 'есть кнопка «Проверить ещё раз»');
  await next.click();
  await page.waitForFunction(() => /Claude установлен, но в него не вошли/.test(document.querySelector('.fr-foot-note')?.textContent ?? ''));
  assert.equal(await page.locator('[data-screen=firstrun]').getAttribute('data-step'), '1', '«Дальше» без входа не пускает дальше и говорит почему');
  assert.equal(await page.locator('[data-claude-model]').count(), 0, 'без входа список моделей не показан');
  await page.screenshot({ path: join(t3shots, 'claude-noauth-1440.png'), animations: 'disabled' });
  ok('Claude найден, входа нет: фраза «откройте Claude и войдите», «Проверить ещё раз», «Дальше» не пускает и говорит почему');

  await setClaude('missing');
  await recheck();
  await page.getByText('Claude на этом компьютере не установлен.').waitFor();
  assert.match(await result.innerText(), /Попросите того, кто настраивал вам компьютер, или выберите другой способ/);
  assert.equal(await page.getByRole('button', { name: 'Проверить ещё раз' }).count(), 1);
  await next.click();
  await page.waitForFunction(() => /Claude на этом компьютере не установлен/.test(document.querySelector('.fr-foot-note')?.textContent ?? ''));
  assert.equal(await page.locator('[data-screen=firstrun]').getAttribute('data-step'), '1');
  await page.screenshot({ path: join(t3shots, 'claude-missing-1440.png'), animations: 'disabled' });
  ok('Claude не найден: понятная фраза, «Дальше» не пускает');

  await setClaude('unknown');
  await recheck();
  await page.getByText('Не удалось определить версию Claude').waitFor();
  assert.equal(await result.innerText(), 'Не удалось определить версию Claude, поэтому работа с ним не начнётся. Нужна версия 2.x — обновите Claude и нажмите «Проверить ещё раз».');
  assert.equal(await page.getByRole('button', { name: 'Проверить ещё раз' }).count(), 1);
  await next.click();
  await page.waitForFunction(() => /Не удалось определить версию Claude/.test(document.querySelector('.fr-foot-note')?.textContent ?? ''));
  assert.equal(await page.locator('[data-screen=firstrun]').getAttribute('data-step'), '1', 'версия не распознана — старт отклонил бы, «Дальше» не пускает');
  await page.screenshot({ path: join(t3shots, 'claude-unknown-1440.png'), animations: 'disabled' });
  ok('версия не распознана: отказ, как у настоящего движка (auth unknown), «Дальше» не пускает');

  await setClaude('unsupported');
  await recheck();
  await page.getByText('Эта версия Claude не поддерживается.').waitFor();
  assert.equal(await result.innerText(), 'Эта версия Claude не поддерживается. Нужна версия 2.x — обновите Claude и нажмите «Проверить ещё раз».');
  assert.equal(await page.getByRole('button', { name: 'Проверить ещё раз' }).count(), 1);
  await next.click();
  await page.waitForFunction(() => /Эта версия Claude не поддерживается/.test(document.querySelector('.fr-foot-note')?.textContent ?? ''));
  assert.equal(await page.locator('[data-screen=firstrun]').getAttribute('data-step'), '1');
  ok('версия не поддерживается: отказ дословно, «Проверить ещё раз», «Дальше» не пускает');

  await setClaude('found');
  await recheck();
  await page.getByText('Готово. Ничего вводить не нужно.').waitFor();
  await page.locator('[data-claude-model]').selectOption('opus');
  const callsBefore = engineCalls.length;
  await next.click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  const sent = engineCalls.slice(callsBefore);
  assert.deepEqual(sent.map(c => c.method), ['hoh/executor/select'], 'на шаге «Модель» сохранение — только select');
  assert.deepEqual(sent[0].params, { executor: 'claude', model: 'opus' });
  ok('«Дальше» сохраняет выбор привилегированной hoh/executor/select {claude, opus}');
  const CONFIRM_LEAD = 'Помощник спрашивает перед изменениями.';
  const CONFIRM_FULL = `${CONFIRM_LEAD} Действуют и ваши собственные настройки Claude: их разрешающие правила могут пропускать действия без вопроса.`;
  await page.locator('.fr-lock').getByText(CONFIRM_LEAD).waitFor();
  assert((await page.locator('.fr-lock').innerText()).includes(CONFIRM_FULL), 'источник user: обе фразы');
  ok('мастер: источник user — подпись «Спрашивать» с двумя фразами');
  if (!stub) {
    accessSources = '';
    await page.getByRole('button', { name: 'Назад' }).click();
    await recheck();
    await page.getByText('Готово. Ничего вводить не нужно.').waitFor();
    await next.click();
    await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
    await page.locator('.fr-lock').getByText(CONFIRM_LEAD).waitFor();
    const lock = await page.locator('.fr-lock').innerText();
    assert(lock.includes(CONFIRM_LEAD) && !lock.includes('собственные настройки Claude'), 'изолированно: только первая фраза');
    accessSources = 'user';
    ok('мастер: изолированно (`--setting-sources ""`) — только первая фраза');
  }
  if (stub) {
    const got = await stub.request('hoh/executor/access/get', { executor: 'claude' });
    assert.deepEqual(got.selected, { executor: 'claude', model: 'opus' }, 'заглушка помнит выбор');
    assert.equal(got.effective.flags[got.effective.flags.indexOf('--setting-sources') + 1], 'user', 'движок запускает Claude только с настройками пользователя');
    assert.deepEqual(got.effective.flags.slice(0, 4), ['--permission-mode', 'default', '--setting-sources', 'user'], '--setting-sources сразу за --permission-mode, как в настоящем запуске');
    await stub.request('session/prompt', { sessionId: 'claude-chat-1', contentBlocks: [{ type: 'text', text: 'привет' }] });
    const { calls } = await stub.request('hoh/stub/executor-calls', {});
    assert.deepEqual(calls.map(c => [c.executor, c.model, c.sessionId]), [['claude', 'opus', 'claude-chat-1']], 'новый чат прошёл через Claude с выбранной моделью');
    ok('новый чат идёт через Claude (по вызовам заглушки)');
  }
  assert.deepEqual(errors, [], 'в консоли ошибок нет');
  assert.deepEqual(outside, [], 'обращений наружу нет');
  await context.close();
}
if (stub) {
  // правка 13: каждый пример — осмысленный ход заглушки (Claude выбран), не ошибка
  const { EXAMPLES } = await import(pathToFileURL(join(ui, 'src/lib/examples.mjs')).href);
  await stub.request('hoh/executor/select', { executor: 'claude', model: 'default' });
  await stub.request('hoh/executor/access/set', { executor: 'claude', mode: 'full' });
  const before = (await stub.request('hoh/stub/executor-calls', {})).calls.length;
  for (const [index, example] of EXAMPLES.entries()) {
    const answer = await stub.request('session/prompt', { sessionId: `example-${index}`, contentBlocks: [{ type: 'text', text: example.text }] });
    assert(answer.messageId, `${example.id}: ход начат`);
  }
  const { calls } = await stub.request('hoh/stub/executor-calls', {});
  assert.deepEqual(calls.slice(before).map(c => c.text), EXAMPLES.map(e => e.text), 'каждый пример дошёл до исполнителя как есть');
  assert(EXAMPLES.length === 3 && EXAMPLES.every(e => !/цен|расписан|регулярно|документ в папке/i.test(e.text)), 'в примерах нет обещаний про расписание, интернет и готовые документы');
  await stub.request('hoh/executor/access/set', { executor: 'claude', mode: 'confirm' });
  await stub.request('hoh/executor/select', { executor: null, model: 'default' });
  ok('каждый пример на заглушке даёт ход Claude, не ошибку');
}
if (askingStub) {
  await askingStub.request('hoh/executor/select', { executor: 'claude', model: 'default' });
  await askingStub.request('session/prompt', { sessionId: 'c1', contentBlocks: [{ type: 'text', text: 'сделай' }] });
  await new Promise(resolve => setTimeout(resolve, 200));
  assert.equal(askingStub.asked.length, 1, 'в режиме «с подтверждением» приходит вопрос подтверждения');
  assert.match(askingStub.asked[0].questions[0].question, /Разрешить\?/);
  await askingStub.request('hoh/executor/access/set', { executor: 'claude', mode: 'full' });
  await askingStub.request('session/prompt', { sessionId: 'c2', contentBlocks: [{ type: 'text', text: 'сделай' }] });
  await new Promise(resolve => setTimeout(resolve, 200));
  assert.equal(askingStub.asked.length, 1, 'в режиме «полный доступ» вопроса нет');
  const { calls } = await askingStub.request('hoh/stub/executor-calls', {});
  assert.deepEqual(calls.map(c => c.mode), ['confirm', 'full']);
  ok('сессия Claude в заглушке: confirm — вопрос подтверждения, full — без вопроса');
}

// ───── слова мастера и окна защиты для людей без опыта кода (HOH-35) ─────
{
  const { page, context, errors } = await open('firstrun', { init: realPath, hook: firstrunHook });
  await page.getByRole('heading', { name: 'Кто будет отвечать' }).waitFor();
  const body = () => page.locator('body').innerText();
  // правка 3 («сейчас»): адрес по умолчанию не ответил
  unanswered = true;
  await page.getByRole('button', { name: 'Дальше', exact: true }).click();
  await page.getByText(/По этому адресу никто не отвечает/).waitFor();
  assert.equal(await page.locator('.fr-result').innerText(), 'По этому адресу никто не отвечает. Если вы сами не запускали модель на этом компьютере, выберите другой способ или попросите того, кто ставил программу.');
  await page.screenshot({ path: join(t3shots, 'ux-wizard-no-answer-1440.png'), animations: 'disabled' });
  unanswered = false;
  ok('адрес по умолчанию не ответил: честный текст, шаг не пройден');
  // правка 2: «Отклонить» в окне защиты
  refuse = { method: 'hoh/model/probe', error: HOST.rejected };
  await page.getByRole('button', { name: 'Дальше', exact: true }).click();
  await page.getByText(/Вы не разрешили проверку/).waitFor();
  assert.equal(await page.locator('.fr-result').innerText(), 'Вы не разрешили проверку. Нажмите «Дальше» и выберите «Разрешить один раз».');
  assert(!(await body()).includes('Неизвестная ошибка'));
  await page.screenshot({ path: join(t3shots, 'ux-wizard-refusal-1440.png'), animations: 'disabled' });
  refuse = { method: 'hoh/model/probe', error: HOST.expired };
  await page.getByRole('button', { name: 'Дальше', exact: true }).click();
  await page.getByText(/Время на ответ вышло/).waitFor();
  refuse = null;
  await page.getByRole('button', { name: 'Дальше', exact: true }).click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  refuse = { method: 'hoh/settings/set', error: HOST.rejected };
  await page.getByRole('button', { name: 'Назад' }).click();
  await page.getByRole('button', { name: 'Дальше', exact: true }).click();
  await page.getByText('Вы не разрешили сохранение. Нажмите «Дальше» и выберите «Разрешить один раз».').waitFor();
  assert(!(await body()).includes('Неизвестная ошибка'));
  refuse = { method: 'hoh/settings/set', error: HOST.failed };
  await page.getByRole('button', { name: 'Дальше', exact: true }).click();
  await page.getByText(/Что-то пошло не так\. Попробуйте ещё раз\./).waitFor();
  assert(!(await body()).includes('Неизвестная ошибка'), 'пустая ошибка не даёт «Неизвестная ошибка»');
  refuse = { method: 'hoh/settings/set', error: HOST.refused };
  await page.getByRole('button', { name: 'Дальше', exact: true }).click();
  await page.getByText('Не удалось сохранить. Помощник отклонил запрос').waitFor();
  assert(!(await body()).includes('Неизвестная ошибка'));
  refuse = null;
  ok('отказ в окне защиты и истёкший срок: понятный текст на «Дальше» (проверка и сохранение), «Неизвестная ошибка» нет');
  // слова первого экрана: ничего из списка запрещённых слов, данные уходят в сервис — сказано
  await page.getByRole('button', { name: 'Сервис в интернете' }).click();
  const cloud = await body();
  for (const word of [/API-ключ/i, /провайдер/i, /движок/i, /\/v1/]) assert.doesNotMatch(cloud, word, String(word));
  assert.equal(await page.locator('[data-data-line]').innerText(), 'Ваши сообщения и то, что Помощник прочитает в файлах, отправляются в этот сервис.');
  await page.locator('[data-mode-claude]').click();
  await page.getByText('Готово. Ничего вводить не нужно.').waitFor();
  assert.equal(await page.locator('[data-data-line]').innerText(), 'Ваши сообщения и то, что Помощник прочитает в файлах, отправляются в сервис Claude (компания Anthropic).');
  ok('у облачного варианта и у Claude сказано, что сообщения и прочитанное уходят в сервис; запрещённых слов на экране нет');
  await page.getByRole('button', { name: 'Дальше', exact: true }).click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  assert.doesNotMatch(await body(), /нельзя отключить/);
  assert.match(await body(), /пока вы сами не включили «Полный доступ»/);
  ok('фраза «Это правило нельзя отключить» заменена');
  assert.deepEqual(errors, []);
  await context.close();
}
{
  // правка 1: собственное действие человека (идентификатор native-preflight-…) и просьба Помощника — разные окна
  const own = request({ approvalId: 'native-preflight-1', humanText: 'Разрешить проверку модели «local» у сервиса «http://localhost:8000/v1»?', reason: undefined, risk: 'high' });
  const { page, context } = await openApproval(own);
  await page.getByText('Подтвердите своё действие').waitFor();
  const text = await page.locator('body').innerText();
  for (const word of ['Высокий риск', 'опасно', 'Помощник просит', 'Помощник не объяснил', 'Объяснение Помощника', 'провайдер']) assert(!text.includes(word), `в окне собственного действия есть «${word}»`);
  await page.getByText('Это вы меняете настройки. Окно нужно, чтобы никто не поменял их за вас.').waitFor();
  await page.screenshot({ path: join(t3shots, 'ux-approval-own-1440.png'), animations: 'disabled' });
  await page.waitForTimeout(1200);
  await page.getByRole('button', { name: 'Отклонить' }).click();
  await page.getByText('Отклонено').waitFor();
  assert.doesNotMatch(await page.locator('body').innerText(), /Помощник получил отказ/, 'отказ в собственном действии не приписан Помощнику');
  await context.close();
  const asked = await openApproval(request());
  await asked.page.getByText('Помощник просит разрешение').waitFor();
  await asked.page.getByText('Высокий риск').waitFor();
  await asked.context.close();
  ok('окно защиты на собственное действие: без «Высокий риск», «опасно», «Помощник просит»; просьба Помощника осталась как была');
}

// ───── окно подтверждения ─────
async function openApproval(view, viewport, options = {}) {
  assert(!/[\p{Cc}\p{Cf}]/u.test(view.humanText), 'humanText без управляющих символов');
  const decisions = [];
  const context = await browser.newContext({ viewport: viewport ?? { width: 1440, height: 900 }, deviceScaleFactor: 1, reducedMotion: 'reduce', locale: 'ru-RU' });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.route('**/*', route => route.request().url().startsWith('file://') ? route.continue() : route.abort());
  await page.exposeFunction('__hoh_decide', async (id, outcome) => { decisions.push({ id, outcome }); await options.decide?.(id, outcome); });
  await page.addInitScript(({ clock }) => {
    window.__approvalActionSeen = false;
    new MutationObserver(() => { if (document.querySelector('.ap-actions button')) window.__approvalActionSeen = true; }).observe(document, { subtree: true, childList: true });
    if (clock !== undefined) { window.__approvalClock = { value: clock }; Date.now = () => window.__approvalClock.value; }
  }, { clock: options.clock });
  await page.addInitScript(`window.__HOH_APPROVAL__ = { current: async () => (${JSON.stringify(view)}), decide: (id, outcome) => window.__hoh_decide(id, outcome) };`);
  await page.goto(pathToFileURL(pages.approval).href);
  await page.evaluate(() => document.fonts.ready);
  return { page, context, decisions, errors };
}

{
  const { page, context, decisions, errors } = await openApproval(request());
  await page.getByRole('alertdialog').waitFor();
  const reject = page.getByRole('button', { name: 'Отклонить' });
  const allow = page.getByRole('button', { name: 'Разрешить один раз' });
  assert.equal(await page.evaluate(() => document.activeElement?.textContent), 'Отклонить', '«Отклонить» в фокусе');
  assert(await allow.isDisabled(), '«Разрешить» закрыта в первую секунду');
  await page.getByText('Защита HoH').waitFor();
  await page.getByText('Почему это опасно').waitFor();
  await page.getByText('Файлы не попадут в корзину. Восстановить их будет нельзя.').waitFor();
  await page.getByText('Объяснение Помощника').waitFor();
  await page.getByText(/Эти файлы старше года/).waitFor();
  await page.getByText('Выполнить команду в папке Документы/HoH: удалить 3 старых файла').waitFor();
  await page.getByText('черновик.docx').waitFor();
  await page.getByText(/Чат не может его открыть/).waitFor();
  // правка 16: до последней минуты окно не торопит
  assert.equal(await page.getByTestId('timer').count(), 0, 'счёта времени до последней минуты нет');
  assert.equal(await page.locator('[data-calm]').innerText(), 'Можно не спешить. Если не ответить, действие отклонится само.');
  assert(!(await page.locator('body').innerText()).includes('Ответьте за'));
  await page.waitForTimeout(1200);
  assert(await allow.isEnabled(), 'через секунду «Разрешить» доступна');
  await page.screenshot({ path: join(shots, 'approval-1440.png'), animations: 'disabled' });
  assert.deepEqual(decisions, [], 'пока никто ничего не нажал — ответа нет');
  ok('подтверждение: «Отклонить» в фокусе, «Разрешить» с задержкой, слова A и C на месте, снимок approval-1440');
  await page.screenshot({ path: join(t3shots, 'ux-g-timer-calm-1440.png'), animations: 'disabled' });
  {
    const last = await openApproval(request({ expiresAt: Date.now() + 45_000 }));
    await last.page.getByTestId('timer').waitFor();
    assert.match(await last.page.getByTestId('timer').innerText(), /^0:[34]\d$/, 'в последнюю минуту виден счёт');
    assert.equal(await last.page.locator('[data-calm]').count(), 0);
    await last.page.screenshot({ path: join(t3shots, 'ux-g-timer-last-minute-1440.png'), animations: 'disabled' });
    await last.context.close();
    ok('таймер окна защиты: до последней минуты «Можно не спешить…», в последнюю — счёт');
  }

  await page.keyboard.press('Enter');
  await page.getByText('Отклонено').waitFor();
  assert.deepEqual(decisions, [{ id: 'approval-1', outcome: 'rejected' }]);
  await page.keyboard.press('Escape');
  await reject.count();
  assert.equal(decisions.length, 1, 'повторный ответ не отправляется');
  assert.deepEqual(errors, []);
  await context.close();
  ok('Enter на сфокусированной «Отклонить» = отказ; повторов нет');
}

{
  const { page, context, decisions } = await openApproval(request({ risk: 'medium', reason: undefined }));
  await page.getByRole('alertdialog').waitFor();
  await page.getByText('Помощник не объяснил, зачем ему это нужно.').waitFor();
  await page.waitForTimeout(1200);
  await page.getByRole('button', { name: 'Разрешить один раз' }).click();
  await page.getByText('Разрешено один раз').waitFor();
  assert.deepEqual(decisions, [{ id: 'approval-1', outcome: 'allowed-once' }]);
  await context.close();
  ok('«Разрешить один раз» отправляет allowed-once ровно один раз; пустое объяснение показано честно');
}

{
  const { page, context, decisions } = await openApproval(request());
  await page.getByRole('alertdialog').waitFor();
  await page.keyboard.press('Escape');
  await page.getByText('Отклонено').waitFor();
  assert.deepEqual(decisions, [{ id: 'approval-1', outcome: 'rejected' }]);
  await context.close();
  ok('Esc = отказ');
}

{
  const { page, context, decisions } = await openApproval(request());
  await page.getByRole('alertdialog').waitFor();
  await page.close({ runBeforeUnload: true });
  await new Promise(r => setTimeout(r, 300));
  assert.deepEqual(decisions, [{ id: 'approval-1', outcome: 'rejected' }], 'pagehide страницы = отказ; нативное закрытие гарантирует ядро');
  await context.close();
  ok('pagehide страницы = отказ; нативное закрытие гарантирует ядро');
}

{
  const { page, context, decisions } = await openApproval(request({ expiresAt: Date.now() + 2000 }));
  await page.getByText('Время вышло, действие отклонено').waitFor({ timeout: 6000 });
  assert.deepEqual(decisions, [], 'срок вышел: ответ за пользователя окно не шлёт, это делает движок');
  assert.equal(await page.getByRole('button', { name: 'Разрешить один раз' }).count(), 0, 'после срока разрешить нельзя');
  await context.close();
  ok('срок вышел: кнопок нет, окно не отвечает за пользователя');
}

{
  const { page, context } = await openApproval(request({ humanText: '<img src=x onerror=alert(1)>Открыть сайт', reason: '<script>window.__hit=1</script>«обман»' }));
  await page.getByRole('alertdialog').waitFor();
  assert.equal(await page.evaluate(() => (window).__hit), undefined);
  assert.equal(await page.locator('.ap-what img, .ap-said script').count(), 0, 'разметка из текста не исполняется');
  await page.getByText('<img src=x onerror=alert(1)>Открыть сайт').waitFor();
  await context.close();
  ok('текст запроса и объяснение модели выводятся как текст, не как разметка');
}

{
  const { page, context } = await openApproval(request({ reason: 'Нужно убрать старые файлы.', details: 'Команда: rm -f old-1.txt old-2.txt' }));
  await page.getByRole('alertdialog').waitFor();
  const main = await page.locator('.ap-body').innerText();
  assert(main.includes('Нужно убрать старые файлы.'), 'объяснение на виду');
  assert(!main.includes('rm -f'), 'сырая команда не в основном тексте');
  await page.getByText('Подробности').click();
  await page.getByText('Команда: rm -f old-1.txt old-2.txt').waitFor();
  await context.close();
  ok('сырая команда только под «Подробностями», основной текст — объяснение');
}

// Просьба Claude (ядро ставит `risk: unassessed`; в заголовке — старое слово ядра «Исполнитель «claude»»): окно говорит человеческими словами.
{
  const ask = (patch = {}) => request({ risk: 'unassessed', humanText: 'Исполнитель «claude» просит выполнить команду', reason: 'Исполнитель «claude» просит выполнить команду', details: 'Команда: ls -la ~/Документы', ...patch });
  const { page, context, errors } = await openApproval(ask());
  await page.getByRole('alertdialog').waitFor();
  const body = await page.locator('body').innerText();
  assert.equal(await page.locator('.ap-what').innerText(), 'Claude просит выполнить команду', 'вместо «Исполнитель «claude»» — Claude');
  assert(!/исполнител/i.test(body), 'слова «исполнитель» в окне защиты нет');
  assert.equal(await page.locator('.ap-risk').innerText(), 'Риск не оценён');
  assert.equal(await page.locator('.ap-risk').getAttribute('data-risk'), 'unassessed');
  assert(!body.includes('Высокий риск'), 'не красный «Высокий риск»');
  assert.equal(await page.locator('.ap-why').getAttribute('data-risk'), 'unassessed');
  assert.equal(await page.locator('.ap-said').count(), 0, 'объяснение равно заголовку: блока нет');
  assert.equal(await page.locator('.ap-details').getAttribute('open'), '', '«Подробности» раскрыты');
  await page.getByText('Команда: ls -la ~/Документы').waitFor();
  assert(await page.getByText('Команда: ls -la ~/Документы').isVisible(), 'команда видна без нажатия');
  await page.screenshot({ path: join(t3shots, 'ux-approval-claude-1440.png'), animations: 'disabled' });
  assert.deepEqual(errors, []);
  await context.close();
  ok('просьба Claude: команда видна сразу, «Риск не оценён», без слова «исполнитель» и без повтора заголовка');
}
{
  const { page, context } = await openApproval(request({ risk: 'unassessed', humanText: 'Claude просит изменить файл заметки.txt', reason: '', details: 'Edit: заметки.txt' }));
  await page.getByRole('alertdialog').waitFor();
  assert.equal(await page.locator('.ap-said').count(), 0, 'пустое объяснение просьбы Claude не показывается');
  assert.equal(await page.locator('.ap-details').getAttribute('open'), '');
  await context.close();
  const next = await openApproval(request({ risk: 'unassessed', humanText: 'Claude просит записать файл новое.txt', reason: 'Нужно сохранить результат.', details: 'Write: новое.txt' }));
  await next.page.getByRole('alertdialog').waitFor();
  assert.equal(await next.page.locator('.ap-said p').innerText(), 'Нужно сохранить результат.', 'настоящее объяснение остаётся');
  assert(!(await next.page.locator('.ap-why').innerText()).includes('Прежний вариант'), 'про прежний вариант — только для существующего файла');
  await next.context.close();
  const old = await openApproval(request({ humanText: 'Изменить файл отчёт.docx', reason: 'Поправить цифры.' }));
  await old.page.getByRole('alertdialog').waitFor();
  await old.page.getByText('Содержимое файла изменится. Прежний вариант может быть потерян.').waitFor();
  assert.equal(await old.page.locator('.ap-details').count(), 0);
  await old.context.close();
  const fresh = await openApproval(request({ humanText: 'Записать файл новый.docx', reason: 'Нужен новый файл.' }));
  await fresh.page.getByRole('alertdialog').waitFor();
  assert(!(await fresh.page.locator('.ap-why').innerText()).includes('Прежний вариант может быть потерян'));
  await fresh.context.close();
  ok('пустое объяснение Claude скрыто, «Прежний вариант» — только у существующего файла');
}
{
  const { page, context } = await openApproval(request());
  await page.getByRole('alertdialog').waitFor();
  assert.equal(await page.locator('.ap-details').count(), 0, 'без details блока «Подробности» нет');
  await context.close();
  ok('без details блока «Подробности» нет');
}

{
  // The view is what the native core hands the window for an address with a secret in it
  // (apps/hoh-desktop/tests/grants.rs checks the core produces exactly this view from `raw`).
  const fixture = JSON.parse(await readFile(new URL('./secret-address-view.json', import.meta.url), 'utf8'));
  const { page, context } = await openApproval(request({ humanText: fixture.view.humanText, reason: fixture.view.reason }));
  await page.getByRole('alertdialog').waitFor();
  const shown = await page.locator('body').innerText();
  assert.match(shown, /user:•••@host\.example\/v1\?api_key=•••#•••/, 'в адресе на экране секрет закрыт');
  assert.match(shown, /token=•••/, 'в объяснении на экране секрет закрыт');
  const html = await page.content();
  for (const secret of fixture.secrets) {
    assert(!shown.includes(secret), `${secret} виден на экране`);
    assert(!html.includes(secret), `${secret} есть в DOM`);
  }
  await context.close();
  ok('окно подтверждения с секретным адресом: на экране •••, секрета нет ни в тексте, ни в DOM');
}

{
  const { page, context } = await openApproval(request({ humanText: 'Разрешить проверку модели «» у провайдера https://api.example.test/v1 в Documents/HoH?' }));
  await page.getByRole('alertdialog').waitFor();
  const headline = await page.locator('#ap-what').innerText();
  assert.equal(headline, 'Разрешить проверку модели у провайдера https://api.example.test/v1 в Documents/HoH?');
  await context.close();
  ok('окно защиты: пустые «» убраны, настоящий путь не переводится');
}

{
  const { page, context } = await openApproval(request(), { width: 800, height: 640 });
  await page.getByRole('alertdialog').waitFor();
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
  assert.equal(overflow, false);
  await page.screenshot({ path: join(shots, 'approval-800x640.png'), animations: 'disabled' });
  await context.close();
  ok('окно 800×640: помещается без горизонтальной прокрутки; снимок approval-800x640');
}

{
  const { page, context, decisions } = await openApproval(request({ expiresAt: Date.now() - 1 }));
  await page.getByText('Время вышло, действие отклонено').waitFor();
  assert.equal(await page.locator('.ap-actions button').count(), 0);
  assert.equal(await page.evaluate(() => window.__approvalActionSeen), false, 'просроченные кнопки не появлялись ни на миг');
  await page.keyboard.press('Escape'); assert.deepEqual(decisions, []);
  await context.close(); ok('уже просроченный запрос никогда не показывает действий');
}
{
  const now = Date.now();
  const { page, context, decisions } = await openApproval(request({ expiresAt: now + 2000 }), undefined, { clock: now });
  await page.waitForTimeout(1200);
  await page.evaluate(deadline => {
    window.__approvalClock.value = deadline;
    document.querySelector('.ap-risky').click();
  }, now + 2000);
  await page.getByText('Время вышло, действие отклонено').waitFor(); assert.deepEqual(decisions, []);
  await context.close(); ok('decide сверяет время непосредственно перед отправкой, между тиками');
}
for (const event of ['Escape', 'pagehide']) {
  let fail = true;
  const { page, context, decisions } = await openApproval(request(), undefined, { decide: async () => {
    if (fail) throw new Error('Передача не удалась');
  } });
  await page.waitForTimeout(1200); await page.getByRole('button', { name: 'Разрешить один раз' }).click();
  await page.getByRole('alert').getByText('Передача не удалась', { exact: false }).waitFor(); fail = false;
  if (event === 'Escape') await page.keyboard.press('Escape');
  else await page.evaluate(() => window.dispatchEvent(new PageTransitionEvent('pagehide')));
  await page.getByRole('status').getByText('Отклонено').waitFor();
  assert.deepEqual(decisions.map(value => value.outcome), ['allowed-once', 'rejected']);
  await context.close(); ok('после ошибки отправки ' + event + ' повторяет отказ');
}

console.log(`всего: ${passed} проверок прошло` + (stub ? ' (движок: настоящая hoh-window-stub)' : ' (движок: подмена в прогоне)'));
} finally {
  await browser.close();
  stub?.stop();
  askingStub?.stop();
  await rm(home, { recursive: true, force: true });
}
