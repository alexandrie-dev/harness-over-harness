# ui — чат A против записей SDK

## Файлы

- src/lib/store.ts: Zustand-адаптер production-контроллера из screens/chat/controller.mjs (.d.mts — интерфейс TS).
- screens/chat/projection.mjs + .d.mts: V4 → пользователь/Помощник/понятные read/write/command строки, скрытие служебных данных, короткие превью и итоги словами.
- src/lib/markdown.tsx: Lexer marked → React; HTML/картинки/ссылки инертны.
- screens/chat/ChatScreen.tsx, icons.tsx, chat.css, chat-tokens.css: вид A с семантическими классами/data; все визуальные значения через переменные. Общие токены s14 сохранены, удалён только отмеченный старый блок ui1.
- src/main.tsx: FirstRun → ChatScreen, onFinish переносит пример без автозапуска, сохраняется только UI-маркер завершения.
- tests/chat/: 44 core/guard/token теста и 6 headless сценариев с настоящим hoh-window-stub. Проверка screens-a обновлена только в подмене rules/set и очистке ресурсов; исходники s14, HTML отдельного первого запуска и подтверждения не менялись.
- scripts/acquire.sh, scripts/tools/pnpm, scripts/build-env.allow, package.json: уже установленный pnpm-native 12.21.0; поддерживаемые acquisition-флаги; явные project-local tsc/vite после очистки PATH.
- vite.config.ts, scripts/local-diagnostics.ts, scripts/check-dist.mjs: локальные диагностические строки вместо внешних ссылок, точный allow-list инертных URI и бюджеты всего окна.
- scripts/verify.sh: read-only проверка dist с явным пропуском нативного бинарника, если он не собирался; полная проверка требует бинарник.

## Результат и отличия

Подробности, числа, ограничения и команды — tests/chat/README.md. Сеть использовалась только разрешённым acquire для npm. 65 пакетов, 29 прямых, lock не менялся, новых пакетов нет. Уязвимость отозвана и исчезла из свежего audit; исключение удалено. Rust/Cargo не запускались.

Скрин A: design/build-a/chat-1440.png. Данные persistent-tools, поэтому текст/число шагов отличаются от макета; не добавлялись неготовые разделы, кнопки задач, панель результата или макетные вкладки. Старый scripts/snapshot.mjs описывает пустую оболочку пакета 1; текущая приёмка и снимок — tests/chat/behavior.mjs.

Два SHIM(hoh-desktop) остаются из interface-first:

- transport.ts: ожидаются engine_request({method,params}) и engine-notification → Notification. Мост Rust — пакет 4, текущий main.rs его не регистрирует. Headless-адаптер к настоящему stdio stub не используется при isTauri().
- controller.mjs: fixture employee-1 задаёт исходный разговор до принятого host-owned выбора. UI не рисует список сотрудников.

## Вопросы ведущему

Перенести 19 переменных из screens/chat/chat-tokens.css в общие токены (полный список в tests/chat/README.md). Подключить принятый native bridge пакета 4, после чего собрать новое Tauri-окно в очереди и проверить с настоящей моделью. Статические проверки первого запуска/подтверждения не доказывают реализацию хранилища ключей и разрешений в движке.


## Новый вид (20.11.2026, u1)

Система `design/DESIGN.md` перенесена в окно: токены, боковая колонка со списком чатов, чат, мастер, окно защиты, «Подтверждения». Подробности и проверки — `tests/chat/README.md`. Уведомление T3 (MIT) — `ui/NOTICE.md`.

- Список чатов: `session/list`; новый чат — случайный id `chat-<uuid>`, который движок создаёт при первом `session/working-directory/get`; переключение — `session/resume` + `session/events` с чистой проекцией. Метод создания чата отдельным вызовом не нужен.
- Открытый чат после обрыва связи тот же, что выбрал человек; первый запуск открывает чат по умолчанию (`hoh/employee/open`, иначе `window-main`).
- Открытый чат определяет и вопрос Помощника: показывается только вопрос с `sessionId` этого чата.
- Шрифт: Open Sans из системы, иначе Inter. Файла Open Sans в сборке нет: решение о лицензии и добавлении ~530 КБ — за ведущим.
- Корневой `NOTICE` не менялся (вне ui/); при желании строка о T3 переносится туда из `ui/NOTICE.md`.

- Код T3 Code (дополнение 20.11): `ui/src/t3/` + `ui/T3-MAP.md`. Переключатель рендера — `ui/src/t3/render.tsx`; включение отложенных файлов описано в его шапке (флаг `deferredEnabled` в `manifest.json`, `t3-sync.py`, строка `@source not "../t3"` в tokens.css, замена тела `ChatMarkdown`). В tsconfig lib поднят до ES2023 (T3 использует `toSorted`).

## HOH-40, этапы 1–2 (22.11.2026, u2): весь веб-клиент T3 как основа окна

План — `plans/hoh40-t3-base.md`. Старая оболочка не тронута и собирается как раньше; основа в сборку пока не входит (э3).

- Пакеты: `ui/T3-DEPS.md`, раздел «HOH-40» — замыкание импортов от `apps/web/src/main.tsx` (`ui/scripts/t3graph.py`, `ui/scripts/t3-deps.py`), версии и патчи из lock T3, отдельно — что заглушено вместо установки.
- Перенос: `ui/src/t3/base/` (`web`, `contracts`, `shared`, `client-runtime`) — раздел `base` в `ui/src/t3/manifest.json`, `ui/scripts/t3-sync.py`. Файлы T3 без правок, кроме двух замен знака и трёх правок-выключателей (`ui/src/t3/base-patches/`); заглушки пакетов скрытых частей — `ui/src/t3/base/stubs` (создаются), рукописные прослойки — `shims/`, `types/`. Подробно — `ui/T3-MAP.md`, раздел «Основа HOH-40».
- Проверки основы: `python3 -I scripts/t3-check.py` (импорты обоих наборов), `tsc --noEmit -p src/t3/base/tsconfig.json` (неотложенные файлы). Отложенные пересчитывает `t3-sync.py` по `ui/package.json` и `ui/node_modules`: после установки пакетов из T3-DEPS — запустить его снова.

## HOH-40, веха 1 (22.11.2026, u2): основа собирается и открывается

- Пакеты поставил ведущий; отложенных 0. lucide-react окна 1.x (у T3 0.564) допущен решением ведущего (`WINDOW_VERSION` в `t3graph.py`): все 276 иконок T3 в 1.x есть.
- Пакеты T3 `packages/*` берутся файлами (только клиентские части, до которых дотягивается `apps/web/src`): заглушки `@t3tools/*` падали на уровне модуля в видимом коде (source-control, провайдеры).
- i18n: `gt-react`/`gt-i18n` не ставятся (решение CEO) — прослойки `src/t3/base/shims/gt-*.ts(x)`: английский как есть, ICU-подстановка, словарь языка, без сети.
- Не переносятся 3D-модели устройств и переводы кроме en/ru (`exclude` манифеста); выключены терминал и импорт тем, имя продукта — HoH (правки, `ui/T3-MAP.md`).
- tsc основы — с `noImplicitAny`, как у T3; файлы скрытых частей на заглушках проверяются по объявлениям (`*.hoh.d.ts`), их импортёры — полностью.
- Точка подключения для хоста и входа — `ui/src/t3/base/README-HOH.md`; отдельная сборка — `ui/src/t3/base/vite.config.ts`, снимки — `ui/scripts/t3-snapshot.mjs`.

## HOH-41, эмблема в окне на T3 (22.11.2026, u2)

- Растры эмблемы — прозрачные, из исходника `ui/src/assets/emblem-a.webp` (копия `site/assets/emblem-a.webp`, 696×750 sRGBA): `sh ui/scripts/emblem-assets.sh` (ImageMagick `convert`, Lanczos, альфа сохраняется) пишет `emblem-{splash,hero,chat,brand,fav}-{1x,15x,2x,3x}.png` и значки окна `apps/hoh-desktop/icons/*` (квадрат с прозрачными углами). Эмблема на экранах — `image-set` / `srcset` под плотность экрана.
- Пустой разговор: эмблема вне потока (absolute над заголовком), показывается только если над заголовком хватает места между шапкой и им (`src/t3-entry/emblem-fit.ts`, атрибут `data-hoh-emblem`, учёт масштаба окна) — не под шапкой и не двигает заголовок. Шапка боковой панели: знак 32×34 px (`T3Wordmark.tsx`, srcset 1×/1.5×/2×/3×). Welcome: знак по левому краю текста. Заставка: слой `fixed; inset: 0` + flex-центр.
- Снимки: `node scripts/t3-emblem-shots.mjs <папка> [dist]` (Chromium, хост HoH с поддельным движком; матрица 1440×900 / 800×550 × масштаб 100 %, 150 %, 1.2, 1.44, 1.73).
