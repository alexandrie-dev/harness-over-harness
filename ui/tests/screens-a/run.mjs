// Проверки первого запуска и окна подтверждения (вариант A) в headless Chromium.
// Страницы — из ui/dist (firstrun.html, approval.html) или из HOH_FIRSTRUN_PAGE / HOH_APPROVAL_PAGE.
// Движок: если задан HOH_WINDOW_STUB — настоящая заглушка hoh-window-stub по stdio, иначе подмена в этом файле.
// Снимки 1440×900 — в ui/test-results/screens-a/ (design/ окно не меняет). Ничего не ходит в сеть: всё кроме file:// блокируется.
import { chromium } from 'playwright-core';
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { startStub } from './stub-engine.mjs';
import { approvalRequest as request } from './fixtures.mjs';

const ui = process.env.HOH_UI_ROOT ?? resolve(import.meta.dirname, '../..');
const shots = resolve(ui, 'test-results/screens-a');
const pages = {
  firstrun: resolve(process.env.HOH_FIRSTRUN_PAGE ?? join(ui, 'dist/firstrun.html')),
  approval: resolve(process.env.HOH_APPROVAL_PAGE ?? join(ui, 'dist/approval.html')),
};
await mkdir(shots, { recursive: true });
const home = await mkdtemp(join(tmpdir(), 'hoh-chromium-'));
const stub = process.env.HOH_WINDOW_STUB ? await startStub(process.env.HOH_WINDOW_STUB) : null;
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
const fake = async (method, params) => {
  engineCalls.push({ method, params });
  if (stub) return stub.request(method, params);
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
    request: (m, p) => window.__hoh_call('request', m, p),
    storeApiKey: k => window.__hoh_call('storeApiKey', k),
    saveRules: r => window.__hoh_call('saveRules', r),
    pickFolder: () => window.__hoh_call('pickFolder'),
  };
};
// The production path: the key goes out as the params of `hoh/model/key/set` through `request`,
// there is no special key hook to hide behind.
const realPath = () => {
  window.__HOH_ENGINE__ = {
    request: (m, p) => window.__hoh_call('request', m, p),
    saveRules: r => window.__hoh_call('saveRules', r),
    pickFolder: () => window.__hoh_call('pickFolder'),
  };
};
const requestOnly = () => { window.__HOH_ENGINE__ = { request: (m, p) => window.__hoh_call('request', m, p) }; };

{
  const { page, context, errors, outside } = await open('firstrun', { init: realPath, hook: firstrunHook });
  const next = page.getByRole('button', { name: 'Дальше' });
  await page.getByRole('heading', { name: 'Подключите модель' }).waitFor();
  assert.equal(await page.getByLabel('Название модели').isHidden(), true, 'ручное имя только в закрытом «Дополнительно»');
  assert.equal(await page.locator('.fr-advanced').getAttribute('open'), null);
  assert(await next.isDisabled(), 'без проверки «Дальше» закрыта');
  assert.match(await page.locator('.fr-foot-note').innerText(), /Сначала проверьте подключение/);
  assert.equal(await page.getByLabel('API-ключ').isDisabled(), true, 'для локальной модели ключ не нужен');
  ok('шаг 1: «Дальше» закрыта до проверки, ключ локальной модели отключён');

  // облачный режим: ключ обязателен, уходит только движку и не остаётся в окне
  await page.getByRole('button', { name: 'Облачный сервис' }).click();
  assert.equal(await page.getByLabel('Адрес').inputValue(), 'https://');
  assert.equal(await page.getByLabel('API-ключ').getAttribute('type'), 'password');
  assert.match(await page.locator('[data-key-note]').innerText(), /Ключ хранится в файле на этом компьютере\. Читать его может только ваша учётная запись\./, 'фраза о хранении ключа уже при вводе');
  await page.getByLabel('Адрес').fill('https://api.example.test/v1');
  await page.getByRole('button', { name: 'Проверить' }).click();
  await page.getByText('Для облачного сервиса нужен API-ключ').waitFor();
  await page.getByLabel('Адрес').fill('https://api.example.test/v1');
  await page.getByLabel('API-ключ').fill(SECRET);
  await page.getByRole('button', { name: 'Проверить' }).click();
  await page.getByText(/Отвечает за/).waitFor();
  // Ключ неизбежно есть в параметрах key/set (так он попадает в хранилище движка) и больше нигде.
  const keySets = engineCalls.filter(call => call.method === 'hoh/model/key/set');
  assert.equal(keySets.length, 1, 'key/set вызван ровно один раз');
  assert.deepEqual(keySets[0].params, { apiKey: SECRET, baseUrl: 'https://api.example.test/v1' });
  assert(!JSON.stringify(engineCalls.filter(call => call.method !== 'hoh/model/key/set')).includes(SECRET), 'в других запросах ключа нет');
  assert.equal(await page.getByLabel('API-ключ').inputValue(), '', 'после сохранения поле ключа очищено');
  assert(!(await page.content()).includes(SECRET), 'ключа нет в разметке страницы');
  const stored = await page.evaluate(() => JSON.stringify([{ ...localStorage }, { ...sessionStorage }]));
  assert(!stored.includes(SECRET), 'ключа нет в localStorage и sessionStorage');
  assert.equal(await next.isEnabled(), true);
  const keyNote = await page.locator('[data-key-note]').innerText();
  assert.match(keyNote, /Ключ хранится в файле на этом компьютере\. Читать его может только ваша учётная запись\.$/);
  assert(!keyNote.includes(SECRET), 'в пояснении ключа самого ключа нет');
  ok('облачный режим: ключ уходит движку один раз (key/set), в окне, хранилищах и других запросах его нет');

  // правка адреса снимает проверку
  await page.getByLabel('Адрес').fill('https://api.example.test/v2');
  assert(await next.isDisabled(), 'после правки адреса проверку надо повторить');
  await page.getByRole('button', { name: 'На этом компьютере' }).click();
  assert.equal(await page.getByLabel('Адрес').inputValue(), 'http://localhost:8000/v1');
  await page.getByRole('button', { name: 'Проверить' }).click();
  await page.getByText(/Отвечает за 2,1 с|Отвечает за 20 мс/).waitFor();
  await page.screenshot({ path: join(shots, 'firstrun-1440.png'), animations: 'disabled' });
  ok('правка адреса снимает проверку; локальный режим проходит проверку; снимок firstrun-1440');

  await next.click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  assert.equal(await page.evaluate(() => document.activeElement?.id), 'fr-h', 'фокус на заголовке нового шага');
  assert(engineCalls.some(c => c.method === 'hoh/settings/set' && c.params.model.model === 'local'), 'модель сохранена через settings/set');
  assert.equal(await page.locator('.fr-path').innerText(), '/home/me/Documents/HoH', 'по умолчанию показан настоящий путь, не перевод');
  await page.getByRole('radio', { name: /Читать открытые сайты самому/ }).check();
  await page.getByRole('button', { name: 'Выбрать' }).click();
  await page.locator('.fr-path', { hasText: 'Документы/Проекты' }).waitFor();
  await page.getByRole('radio', { name: /Спрашивать про каждый новый сайт/ }).check();
  await page.screenshot({ path: join(shots, 'firstrun-2-1440.png'), animations: 'disabled' });
  await page.getByRole('button', { name: 'Назад' }).click();
  await page.getByRole('heading', { name: 'Подключите модель' }).waitFor();
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByRole('heading', { name: 'Я ваш Помощник.' }).waitFor();
  assert.deepEqual(rules.at(-1), { internet: 'ask', folder: 'Документы/Проекты' });
  await page.screenshot({ path: join(shots, 'firstrun-3-1440.png'), animations: 'disabled' });
  ok('шаги 2 и 3: правила сохранены, «Назад» возвращает, снимки firstrun-2 и firstrun-3');

  await page.getByRole('button', { name: /Прочитать документ/ }).click();
  assert.equal(await page.evaluate(() => document.body.dataset.finished), 'Прочитай документ в папке и перескажи в трёх строках', 'onFinish получил текст примера');
  assert.deepEqual(errors, []);
  assert.deepEqual(outside, [], 'страница не обращалась наружу');
  await context.close();
  ok('шаг 3: пример завершает первый запуск без ошибок страницы и без внешних запросов');
}

{
  // The rules method is now in the protocol. Simulate refusal of a non-default
  // policy, while still accepting default rules; the UI must not lose the refusal.
  const { page, context } = await open('firstrun', { init: requestOnly, hook: async (kind, method, params) => {
    if (kind === 'request' && method === 'hoh/settings/rules/set' && params.internet !== 'ask') throw new Error('Нестандартные правила недоступны');
    return firstrunHook(kind, method, params);
  } });
  await page.getByRole('button', { name: 'Проверить' }).click();
  await page.getByText(/Отвечает за/).waitFor();
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByRole('radio', { name: /Читать открытые сайты самому/ }).check();
  await page.getByRole('button', { name: 'Выбрать' }).click();
  await page.getByText(/Выбор папки пока недоступен/).waitFor();
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByRole('alert').getByText(/Не удалось сохранить/).waitFor();
  assert.equal(await page.locator('[data-screen=firstrun]').getAttribute('data-step'), '2', 'остались на шаге 2');
  await page.getByRole('radio', { name: /Спрашивать про каждый новый сайт/ }).check();
  await page.getByRole('button', { name: 'Дальше' }).click();
  await page.getByRole('heading', { name: 'Я ваш Помощник.' }).waitFor();
  await context.close();
  ok('сохранение правил: отказ для особого выбора не теряется молча, обычный сохраняется');
}

{
  const { page, context } = await open('firstrun', { init: requestOnly, hook: async (kind, m) => { if (m === 'hoh/model/probe') throw new Error('Модель недоступна'); return firstrunHook(kind, m); } });
  await page.getByRole('button', { name: 'Проверить' }).click();
  await page.getByText('Не удалось проверить. Модель недоступна').waitFor();
  assert(await page.getByRole('button', { name: 'Дальше' }).isDisabled());
  await context.close();
  ok('сбой проверки: понятный текст, «Дальше» закрыта');
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
  await page.getByRole('button', { name: 'Облачный сервис' }).click();
  await page.getByLabel('Адрес').fill('https://api.example.test/v1');
  await page.getByLabel('API-ключ').fill(SECRET);
  await page.getByRole('button', { name: 'Проверить' }).click();
  await page.getByText(/Не удалось сохранить ключ/).waitFor();
  assert.equal(await page.getByLabel('API-ключ').inputValue(), SECRET);
  assert(!(await page.content()).includes(SECRET));
  fail = false; await page.getByRole('button', { name: 'Проверить' }).click();
  await page.getByText(/Отвечает за/).waitFor();
  assert.equal(await page.getByLabel('API-ключ').inputValue(), '');
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
  await page.getByRole('button', { name: 'Проверить' }).click(); await page.getByText(/Отвечает за/).waitFor();
  await page.getByRole('button', { name: 'Дальше' }).click();
  assert(await page.getByLabel('Адрес').isDisabled()); assert(await page.getByLabel('Название модели').isDisabled());
  assert(await page.getByRole('button', { name: 'Назад', includeHidden: true }).isDisabled()); assert(await page.getByRole('button', { name: 'Дальше' }).isDisabled());
  releaseModel(); await page.getByRole('heading', { name: 'Правила работы' }).waitFor();
  await page.getByRole('button', { name: 'Дальше' }).click();
  assert(await page.getByRole('radio', { name: /Читать открытые сайты самому/ }).isDisabled());
  assert(await page.getByRole('button', { name: 'Выбрать' }).isDisabled());
  assert(await page.getByRole('button', { name: 'Назад', includeHidden: true }).isDisabled()); assert(await page.getByRole('button', { name: 'Дальше' }).isDisabled());
  releaseRules(); await page.getByRole('heading', { name: 'Я ваш Помощник.' }).waitFor();
  await context.close(); ok('сохранение модели и правил блокирует поля, «Назад» и «Дальше»');
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
  assert.match(await page.getByTestId('timer').innerText(), /^[45]:\d\d$/, 'виден срок');
  await page.waitForTimeout(1200);
  assert(await allow.isEnabled(), 'через секунду «Разрешить» доступна');
  await page.screenshot({ path: join(shots, 'approval-1440.png'), animations: 'disabled' });
  assert.deepEqual(decisions, [], 'пока никто ничего не нажал — ответа нет');
  ok('подтверждение: «Отклонить» в фокусе, «Разрешить» с задержкой, слова A и C на месте, снимок approval-1440');

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
  await rm(home, { recursive: true, force: true });
}
