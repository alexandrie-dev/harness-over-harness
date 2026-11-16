# Настольное приложение HoH на Rust: на чём строить окно

Дата среза: 2026-11-16. Версии и даты взяты из API crates.io и GitHub в этот день, остальное — из
первоисточников, прочитанных в этой сессии (репозитории, release notes, документация). Ничего не
собиралось и не запускалось: все утверждения о поведении — из документации и исходников, а не из опыта.

Пометки: **[проверено]** — прочитан первоисточник; **[вторично]** — пересказ в блоге или поисковой выдаче;
**[не проверено]** — моё знание или вывод без источника в этой сессии.

Клоны для чтения лежат в
`/tmp/claude-1000/-home-save-Alexandrie-HarnessOverHarness/a3276e7a-5aae-4a80-860b-e32609c6b177/scratchpad/refs/`:
`longbridge_gpui-kit` (f1d1f5a), `longbridge_gpui-fast` (454c29e), `gpui-ce_gpui-ce` (f388375),
`iced-rs_iced` (edb068e), `emilk_egui` (c61b45f), `slint-ui_slint` (2f450d6), `marc2332_freya` (d878d1a),
`lapce_floem` (1351ffb), `linebender_xilem` (dc49687), `vizia_vizia` (8a0c60d), `DioxusLabs_blitz` (e8b370a),
`AccessKit_accesskit` (432d066). Каталог временный.

Как читать ссылки. Путь без адреса (например, `website/docs/test.md`) — это файл в репозитории
инструмента, о котором идёт речь, на названном выше коммите; адрес в сети получается как
`https://github.com/<владелец>/<репозиторий>/blob/main/<путь>`. Репозитории: `longbridge/gpui-kit`,
`longbridge/gpui-fast`, `zed-industries/zed`, `tauri-apps/tauri`, `tauri-apps/tauri-docs`,
`DioxusLabs/dioxus`, `DioxusLabs/blitz`, `slint-ui/slint`, `emilk/egui`, `iced-rs/iced`,
`marc2332/freya`, `linebender/xilem`, `lapce/floem`, `makepad/makepad`, `vizia/vizia`. Версии и число
загрузок — `https://crates.io/api/v1/crates/<имя>`; звёзды, даты выпусков и коммитов —
`https://api.github.com/repos/<владелец>/<репозиторий>`.

## 0. Главное

**Рекомендация.** Два стека, между которыми выбирает один ответ основателя («допустима ли вёрстка
HTML/CSS внутри окна»):

1. **GPUI + GPUI Kit** (`gpui-kit` 0.10.0, Apache-2.0) — полностью нативное окно на GPU, тот же движок,
   что у Zed. Это единственный нативный вариант, где уже есть готовые части именно нашего окна: лента
   сообщений с виртуализацией, потоковый Markdown с таблицами и выделением, вложения, формы, диалоги,
   таблица данных, настройки, пружинная анимация, AccessKit и тестовый стенд без дисплея.
2. **Tauri 2 + интерфейс на Rust в WebAssembly (Leptos)** — окно на системном webview. Самый надёжный
   текст, доступность, упаковка и обновление; на Windows внутри Chromium (WebView2). Цена: это HTML/CSS
   в webview, а не нативная отрисовка.

Вариант по умолчанию — первый, с обязательной пробой на Windows до начала основной работы. Условия смены
описаны в разделе 8.

**Что меняет наш дизайн.**

- Интерфейс делать тонким: транспорт, сопряжение, состояние и модели экранов — в отдельном crate без
  зависимости от инструмента окна. Тогда большая часть логики проверяется без отрисовки, а смена
  инструмента окна не трогает остальное.
- Предпросмотр документов и таблиц готовит движок (Markdown, табличные данные, PNG страниц). Тогда окну
  нужны три вида содержимого вместо просмотрщиков PDF, DOCX и XLSX.
- Обновления приложения раздаёт сам движок на Spark по локальной сети: сеть заказчика может быть закрытой,
  а готовый механизм обновления есть только у Tauri.
- Сокет к движку живёт в нативной части на Rust, а не в странице: webview не умеет доверять
  самоподписанному сертификату по отпечатку.
- Длинные анимации ожидания (а очередь к локальной модели длинная) в стандартном GPUI стоят дорого по CPU;
  индикатор очереди проектировать как редкие обновления состояния, а не вечный спиннер.
- Приёмка на Windows не закрывается проверками без дисплея на Linux ни в одном стеке: нужна Windows-машина
  или виртуальная машина.

**Неожиданное.**

- `gpui-component` за 2026 год превратился в **GPUI Kit**: три слоя, AccessKit, тесты без дисплея,
  компоненты чата. При этом официальный crate `gpui` на crates.io заморожен на 0.2.2 с 2025-10-22, а
  экосистема живёт на еженедельных снимках `gpui-pre`, которые публикует не Zed, а Longbridge.
- В основной GPUI в 2026 году вошли AccessKit (PR от 2026-05-27) и отрисовка без дисплея на Linux
  (сентябрь–октябрь 2026).
- Tauri 3 в альфе и делает Chromium (CEF) равноправной средой наряду с системным webview. OpenCode и
  Paseo в 2026 году ушли с Tauri на Electron; в числе причин оба называют расхождения системных webview.
- Slint и egui поставляют встроенные MCP-серверы, через которые агент видит дерево интерфейса и нажимает
  кнопки; у Slint есть снимок экрана из командной строки без дисплея.
- В Iced по-прежнему нет доступности для экранных дикторов.
- Компоненты Zed (`markdown`, `ui`, `editor`) под GPL-3.0; под Apache-2.0 только сам GPUI и его
  служебные crates.

## 1. Требования и как я их взвешивал

Окно: чат с потоковым Markdown (заголовки, списки, таблицы, код, ссылки), карточки задач, мастер «новая
задача» с формами и вложениями (перетаскивание файлов), предпросмотр документов и таблиц, диалоги
одобрения, индикатор очереди, настройки, русский интерфейс с вводом, выделением и копированием,
желательно доступность.

Среда: пользователи без технической подготовки, в основном Windows, также macOS и Linux; движок на DGX
Spark (Linux aarch64) в той же сети; то же приложение работает с движком на своей машине.

Ограничения процесса: код пишут агенты, проверяет другая модель; проверки только без дисплея; в продукте
нет TypeScript.

Вес критериев: (1) готовность частей окна и качество текста; (2) проверяемость без дисплея; (3) надёжность
на Windows; (4) насколько предсказуемо пишут агенты; (5) упаковка и обновление; (6) доступность;
(7) устойчивость API.

## 2. Сводная таблица

Версии и даты — API crates.io; звёзды — API GitHub; оба на 2026-11-16.

| Инструмент | Последняя версия | Ритм выпусков | Отрисовка | Доступность | Проверка без дисплея | Лицензия | Итог |
|---|---|---|---|---|---|---|---|
| GPUI (Zed) | crates.io `gpui` 0.2.2 (2025-10-22); `gpui-pre` 0.3.13 (2026-11-16) | Zed выходит еженедельно; crate не публикуется; снимки `gpui-pre` раз в неделю | Metal / DirectX 11 / wgpu | AccessKit с 2026-05 | `TestAppContext`, `render_to_image` | Apache-2.0 | основа варианта 1 |
| GPUI Kit (бывш. gpui-component) | 0.10.0 (2026-11-16) | раз в неделю, в каждом минорном выпуске ломающие изменения | GPUI | AccessKit, сами оценивают как «частично» | стенд событий на трёх ОС, пиксели на Linux и macOS | Apache-2.0 | **вариант 1** |
| Tauri 2 | 2.12.6 (2026-11-16); 3.0.0-alpha.6 (2026-11-06) | минорные раз в 1–3 месяца | WebView2 / WKWebView / WebKitGTK; CEF в 3.0-alpha | средствами webview | WebDriver, режим браузера, `tauri::test` | Apache-2.0 OR MIT | **вариант 2** (оболочка) |
| Leptos | 0.8.23 (2026-10-31); 0.9.0-rc1 | частые патчи; 0.9 на подходе | DOM (WASM) | DOM | headless Chromium | MIT | вариант 2 (интерфейс) |
| Dioxus | 0.7.10 (2026-07-30); 0.8.0-alpha.3 (2026-10-16) | минорный раз в год, патчи между | webview (wry) или Blitz (wgpu) | webview: DOM; Blitz: AccessKit | SSR-снимки, Playwright для web | MIT OR Apache-2.0 | запасной webview-вариант |
| Yew | 0.23.0 (2026-03-10) | редко | DOM (WASM) | DOM | headless Chromium | MIT OR Apache-2.0 | не брать: замедлился |
| Slint | 1.19.0 (2026-11-16) | раз в 2–3 месяца, API 1.x стабилен | FemtoVG / Skia / Vello / программный | AccessKit, Qt | тестовый backend, снимок из CLI, MCP | GPL-3.0 / бесплатная для настольных / коммерческая | третий по силе |
| egui | 0.37.0 (2026-10-15) | раз в 1–2 месяца, ломающие | glow / wgpu | AccessKit | `egui_kittest`, `egui_mcp` | MIT OR Apache-2.0 | лучший для инструментов, не для премиального вида |
| Iced | 0.14.0 (2025-12-07); master 0.15.0-dev | раз в 12–15 месяцев | wgpu / tiny-skia | **нет** | `iced_test` | MIT | не брать |
| Freya | 0.5.2 (2026-11-11); 0.6.0-alpha.1 | активно, переписан в 0.4 | Skia | AccessKit | `freya-testing` | MIT | рано: один автор, свежая переделка |
| Xilem / Masonry | 0.4.0 (2025-10-29) | выпусков не было год, репозиторий живой | Vello (wgpu) | AccessKit | `masonry_testing` | Apache-2.0 | «experimental» по словам авторов |
| Floem | 0.2.0 (2024-11-14) | crate не выходил два года | wgpu (vger / vello) / Skia / tiny-skia | нет | не найдено | MIT | не брать |
| Makepad | `makepad-widgets` 1.0.0 (2025-05-13) | crate не обновлялся 18 месяцев, репозиторий живой | свой GPU-рендер | не найдено | не найдено | MIT OR Apache-2.0 | не брать |
| Vizia | 0.4.0 (2026-04-23) | раз в 6–12 месяцев | Skia | AccessKit | не найдено | MIT | не брать: мало кода и пользователей |

## 3. Кандидаты по отдельности

### 3.1. GPUI и GPUI Kit

**Что это сейчас.** GPUI — UI-фреймворк редактора Zed, живёт в его репозитории, лицензия Apache-2.0
**[проверено]** (`crates/gpui/Cargo.toml`, <https://github.com/zed-industries/zed>). README прямо говорит:
«GPUI is still in active development as we work on the Zed code editor, and is still pre-1.0. There will
often be breaking changes between versions»
(<https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md>). Версия в `Cargo.toml` на main —
всё те же 0.2.2; последний выпуск на crates.io — 2025-10-22 (<https://crates.io/crates/gpui>). На main
фреймворк разбит на `gpui`, `gpui_platform`, `gpui_macos`, `gpui_windows`, `gpui_linux`, `gpui_wgpu`,
`gpui_web`, `gpui_macros` **[проверено по снимку в `longbridge_gpui-fast/crates`]**.

**GPUI Kit.** Репозиторий `longbridge/gpui-component` переименован в `longbridge/gpui-kit` (20,8 тыс.
звёзд). Описание из README **[проверено]**: «GPUI Kit ships 75+ documented components and primitives,
WebAssembly support, AccessKit accessibility, UI integration testing, and an optional JavaScript extension
runtime» (<https://github.com/longbridge/gpui-kit>). Слои: `gpui-base` (поведение без стиля),
`gpui-component` (готовая визуальная система, оформление по мотивам shadcn/ui), `gpui-kit` (одна
зависимость для приложения), отдельно `gpui-shell` (расширения на JavaScript; нам не нужен). Сделан
компанией Longbridge для собственного торгового терминала Longbridge Pro.

**Откуда берётся GPUI.** GPUI Kit зависит не от `gpui`, а от `gpui-pre` с точной версией
(`gpui = { package = "gpui-pre", version = "=0.3.12" }` в корневом `Cargo.toml`). FAQ **[проверено]**:
«They are unmodified snapshots of Zed's GPUI crates, published to crates.io under the `gpui-pre` name
because Zed does not publish GPUI there regularly», снимок публикуется раз в неделю
(`website/docs/faq.md`, <https://gpui-kit.com>). Следствие: цепочка поставки проходит через одну стороннюю
компанию. Параллельно существуют `gpui-fast` (форк Longbridge с инкрементальной отрисовкой, включается
feature-флагом) и `gpui-ce` (форк сообщества, API уже расходится с основным:
<https://github.com/gpui-ce/gpui-ce>); поиск показывает ещё несколько форков с собственными именами crate
**[вторично]**.

**Платформы и отрисовка [проверено по исходникам и README GPUI].**

| ОС | Окна | Графика | Текст |
|---|---|---|---|
| macOS | AppKit | Metal | CoreText через `font-kit` |
| Windows 10/11, x64 и Arm64 (Arm64 — **[вторично]**) | Win32 | DirectX 11 (`directx_renderer.rs`) | DirectWrite |
| Linux X11 и Wayland, x86_64 и aarch64 | свои клиенты X11 и Wayland | wgpu (Vulkan) | cosmic-text и swash (`gpui_wgpu/src/cosmic_text_system.rs`) |

Требования Zed на Windows: «Zed requires a DirectX 11 compatible GPU to run»; в виртуальной машине «Zed
will work in this environment, performance may be degraded» (<https://zed.dev/docs/windows>). На Linux:
«Zed requires a GPU to run effectively», нужен Vulkan; готовые сборки для aarch64 требуют glibc ≥ 2.35
(<https://zed.dev/docs/linux>). Документация GPUI Kit для разработки называет macOS 15+, Windows 10+,
Rust 1.92+ (`website/docs/installation.md`). Поведение на офисных ПК без видеокарты и по RDP
(программный DirectX) **[не проверено]**.

**Текст и содержимое [проверено по `website/component/*.md`].**

- `TextView` рисует Markdown и простой HTML: заголовки, списки, таблицы, блоки кода с подсветкой
  (tree-sitter или syntect), ссылки с собственным обработчиком, картинки. Выделение включено по
  умолчанию («uses the shared window selection engine from `gpui-base`»). `stream_fade(true)` —
  плавное проявление приходящих кусков ответа, включая текст в таблицах и блоках кода.
- Готовые части чата: `MessageScroller` («A virtualized message list with tail following, history
  insertion, unread navigation»), `Message`, `Bubble`, `Attachment` (состояния загрузки, удалить,
  повторить), `Questionnaire` (многошаговый опрос с проверкой), `ShimmerText`, `Diff`.
- Остальное: `Form`, `Dialog`, `AlertDialog`, `Sheet`, `Notification`, `DataTable` (виртуализация строк
  и столбцов, сортировка, изменение ширины), `VirtualList`, `Settings`, `Progress`, `Spinner`, `Sidebar`,
  `Dock`, `Editor`, графики.
- Перетаскивание файлов из системы: `ExternalPaths` реализован в `gpui_windows`, `gpui_macos`, X11 и
  Wayland **[проверено по исходникам]**.
- Ввод: IME на Windows обрабатывается (`WM_IME_COMPOSITION` в `gpui_windows/src/events.rs`). Кириллица
  IME не требует.
- Локализация через `rust-i18n`; встроенные строки компонентов есть для `en`, `zh-CN`, `zh-HK`, `zh-TW`,
  `it` (`website/docs/i18n.md`). **Русского нет** — строки компонентов переводим сами.
- Просмотрщика PDF и офисных документов нет. Есть экспериментальный `gpui-webview` на wry.

**Сочетания клавиш в русской раскладке.** Документация Zed: «Zed's support for non-QWERTY keyboards is
still a work in progress». На macOS для кириллицы сопоставление идёт и по латинской раскладке; на Linux
«we use the QWERTY-layout equivalent key for keyboard shortcuts». Про Windows раздел молчит
(<https://zed.dev/docs/key-bindings>). Ctrl+C, V, X, A, Z в русской раскладке на Windows — обязательная
проверка в пробе **[не проверено]**.

**Анимация.** Три уровня (`website/docs/animation.md`): `Animation`/`with_animation` самого GPUI;
`gpui-base` motion — `transition`, `spring` с перенацеливанием на лету, `Presence` для анимации ухода;
motion-токены темы. Учитывается системная настройка «уменьшить движение».

Цена анимации в стандартном GPUI: по измерениям авторов GPUI Fast на их галерее компонентов (Linux, до
144 кадров в секунду) «A spinner animating» стоит 5,91 мс главного потока на кадр и 85 % одного ядра
против 0,73 мс и 11 % у GPUI Fast (<https://github.com/longbridge/gpui-fast>). Это тяжёлая страница, но
причина общая: стандартный GPUI при анимации перестраивает окно целиком. Для нас важно, потому что
ожидание локальной модели длится минуты.

**Доступность.** В GPUI: PR #56065 «gpui: Accesskit support», слит 2026-05-27
(<https://github.com/zed-industries/zed/pull/56065>); адаптеры `accesskit_macos`, `accesskit_windows`,
`accesskit_unix` подключены в соответствующих crate **[проверено]**. GPUI Kit: роли, имена, состояния и
действия встроены в компоненты, но в собственной сравнительной таблице строка Accessibility у GPUI Kit —
«Partial» (у egui, Qt и Slint — «Yes»), с оговоркой, что нужна проверка с экранным диктором на каждой ОС
(`website/docs/comparison.md`).

**Упаковка и обновление.** Встроенного нет. Документация GPUI Kit описывает ручной путь: `.app` и DMG с
подписью и нотаризацией, установщик Inno Setup для Windows, `dpkg-deb` для Linux; обновление — «Your
application and its distribution system own the update policy, release metadata, verification,
installation, and recovery», для одиночного исполняемого файла предлагается `self_update`
(`website/docs/packaging.md`, `auto-update.md`). Сторонние инструменты — в разделе 6.

**Размер.** Hello World в release около 12 МБ по таблице GPUI Kit, с оговоркой авторов «approximate
references from different build conditions». Установщики Zed 68–119 МБ **[вторично]**, но это целый
редактор. Память не измерялась.

**Кто выпустил приложения.** Zed (95 тыс. звёзд, v1.28.1 от 2026-11-11; панель агентов), Longbridge Pro.
Агентные приложения на GPUI по поиску GitHub: `sheprd-labs/sheprd` (2,3 тыс.), `feigeCode/navop`
(2,2 тыс.), `maddada/Ghostex` (1,7 тыс.), `iAmCorey/Wake` (1,6 тыс.), `bytesbay/pi-gpui`,
`stippi/code-assistant` **[проверено только описание и активность репозиториев]**.

**Больные места.**

- Устойчивость API: GPUI Kit выходит раз в неделю (0.7.3 — 2026-10-12, 0.8.0 — 10-19, 0.9.0 — 11-02,
  0.10.0 — 11-16), в заметках к 0.8.0 раздел Breaking Changes занимает полтора десятка пунктов
  (`release-notes.md`). Нужно закрепить версию и обновляться по своему расписанию.
- Windows — самый молодой backend GPUI. Автор Paneflow откладывал сборку для Windows Arm64 «pending GPUI
  DirectX reliability» **[вторично]**
  (<https://baeseokjae.github.io/posts/native-app-for-coding-agents-rust-gpui/>).
- Первая сборка 5–15 минут **[вторично]**.
- Смешение линий `gpui` и `gpui-pre` даёт ошибку компиляции `Render is not satisfied` **[вторично]**.
- Чужие GPL-компоненты Zed копировать нельзя: `crates/markdown`, `crates/ui`, `crates/editor` — GPL-3.0
  **[проверено]**.

### 3.2. Tauri 2 с интерфейсом на Rust в WebAssembly

**Версии.** Tauri 2.12.6 (2026-11-16), 113 тыс. звёзд, 45 млн загрузок crate. Идёт 3.0: alpha.0 —
2026-09-13, alpha.6 — 2026-11-06 (<https://github.com/tauri-apps/tauri/releases>).

**Отрисовка и требования к машине пользователя [проверено].**

| ОС | Webview | Что нужно на машине |
|---|---|---|
| Windows | WebView2 (Chromium) | предустановлен в «Windows 10 (from version 1803 onward) and later» (<https://v2.tauri.app/start/prerequisites/>) |
| macOS | WKWebView | macOS 10.15+; версия движка зависит от версии ОС |
| Linux | WebKitGTK 4.1 | системный пакет `libwebkit2gtk-4.1` |

Установщик может нести WebView2 с собой: `offlineInstaller` добавляет около 127 МБ и «Recommended for
offline environments», `fixedVersion` — около 180 МБ; варианты с загрузчиком требуют интернет
(<https://v2.tauri.app/distribute/windows-installer/>). Обновления WebView2 для Windows 10 22H2 обещаны
как минимум до октября 2028 года **[вторично]**
(<https://www.windowscentral.com/microsoft/windows-10/microsoft-edge-support-windows-10-until-2028-for-free>).

**Tauri 3 и CEF.** В 3.0-alpha среда выбирается в коде:
`.runtime(tauri_runtime_wry::Wry::default()) // or tauri_runtime_cef::Cef::default()` **[проверено]**
(<https://github.com/tauri-apps/tauri/releases/tag/tauri-v3.0.0-alpha.0>). CEF даёт один и тот же Chromium
на всех ОС ценой размера: сторонний отчёт называет около 170 МБ на macOS **[вторично]**. Срок стабильной
3.0 не объявлен.

**Интерфейс на Rust.** Leptos 0.8.23 (0.9.0-rc1 от 2026-10-31), Dioxus 0.7.10, Yew 0.23.0. Интерфейс
компилируется в WASM и живёт в webview; системные вещи (сокет, mDNS, файлы, хранилище ключей) — в нативной
части через команды и каналы Tauri. Поддерживаемой Rust-обёртки над IPC Tauri для WASM нет: `tauri-wasm`
0.2.0 от 2025-04-07 с 235 загрузками за 90 дней. Обёртку в несколько десятков строк на `wasm-bindgen`
пишем сами. JavaScript-склейка самого Tauri остаётся внутри зависимостей; своего TypeScript в продукте
нет.

**Текст.** Всё делает браузерный движок: шейпинг, кириллица, выделение, копирование, IME, таблицы,
анимации и переходы CSS. Markdown — `pulldown-cmark` 0.13.6 или `comrak` в HTML. Библиотека компонентов
для Leptos `thaw` не обновлялась с 2025-08-03 (0.4.8); дизайн-систему придётся писать на CSS.

**Перетаскивание.** Свой обработчик Tauri даёт пути файлов. Из схемы конфигурации **[проверено]**:
«Disabling it is required to use HTML5 drag and drop on the frontend on Windows since we replace the drag
drop handler of WebView2». То есть на Windows — либо пути файлов от Tauri, либо HTML5-перетаскивание
внутри страницы.

**Упаковка.** Цели сборщика по схеме конфигурации: `deb`, `rpm`, `appimage`, `msi`, `nsis`, `app`, `dmg`
**[проверено]**. MSIX нет. MSI собирается только на Windows («WiX can only run on Windows systems»),
кросс-сборка NSIS возможна, но «should only be used as a last resort». Обновление — `tauri-plugin-updater`
2.14.1 с проверкой подписи; источник обновлений задаётся адресом **[не проверено в этой сессии, по
памяти]**.

**Больные места.**

- Linux: «on some setups, most often NVIDIA GPUs» WebKitGTK и драйвер расходятся — пустое окно, мерцание,
  падение при изменении размера. Обходы — переменные `__NV_DISABLE_EXPLICIT_SYNC=1`,
  `WEBKIT_DISABLE_DMABUF_RENDERER=1`, `WEBKIT_DISABLE_COMPOSITING_MODE=1`; Tauri их сам не выставляет
  (<https://v2.tauri.app/develop/debug/linux-graphics/>). DGX Spark — это как раз Linux с NVIDIA, так что
  запуск окна на самом Spark попадает в зону риска.
- Расхождение webview между ОС. OpenCode Desktop перешёл на Electron (пост от 19 апреля; год 2026 — по
  сопутствующим источникам):
  «Tauri uses WebKit on macOS and Linux, which not only has worse performance than Chromium», плюс
  расхождения стилей (<https://dev.to/brendonovich/moving-opencode-desktop-to-electron-4hip>). Paseo
  (2026-05-28): «The WebKitGTK bindings Tauri was using were too old», «Wayland had problems», «Some
  screens had real layout differences» (<https://paseo.sh/blog/i-was-wrong-about-electron>). У обоих
  интерфейс и сервер на TypeScript, часть причин к нам не относится.
- На Windows этих проблем нет: там Chromium. Риск webview-стека сосредоточен на наименее важной для нас
  платформе.

**Кто выпустил приложения.** Jan (45,7 тыс. звёзд, каталог `src-tauri` в репозитории); Spacedrive, Cap,
anarlog (бывш. Hyprnote) — стек этих трёх называю по памяти **[не проверено]**. У всех интерфейс на
JavaScript. Крупных приложений на Tauri с интерфейсом на
Leptos я не нашёл **[не проверено]**. Goose Desktop — Electron 45 (`ui/desktop/package.json`).

### 3.3. Dioxus: desktop через webview и нативный рендер

**Версии.** 0.7.0 вышел 2025-10-31, текущая стабильная 0.7.10 (2026-07-30); 0.8.0-alpha.3 — 2026-10-16
(<https://github.com/DioxusLabs/dioxus/releases>). 39,8 тыс. звёзд.

**Desktop через webview [проверено].** Отрисовка в системном webview через `wry` (в 0.8-alpha — wry 0.57,
tao 0.37), при этом код приложения исполняется нативно, а не в WASM, и ему доступны файловая система и
сокеты (<https://dioxuslabs.com/learn/0.7/guides/platforms/desktop>). Размер «typically under 5MB».
Требования к webview те же, что у Tauri, и те же риски на Linux. Сборка установщиков — `dx bundle`.
Готового механизма обновления нет **[не проверено]**. Компоненты: 28 примитивов по образцу Radix и их
оформленные версии в стиле shadcn (заметки к 0.7.0); на crates.io `dioxus-primitives` — заглушка 0.0.0,
ставится через `dx components add` или из git. В 0.8.0-alpha.3 «`dx components` doesn't work in this
release».

**Dioxus Native (Blitz).** Свой движок HTML/CSS на wgpu: Stylo, Taffy, Parley, Vello, AccessKit. README
Blitz **[проверено]**: «Blitz is currently in a **beta** state ... usable for making apps if you are an
early adopter and willing to live on the bleeding edge. But there are also still many bugs and missing
features» (<https://github.com/DioxusLabs/blitz>, таблица поддержки CSS: <https://blitz.is/status/css>).
Версии: `blitz` 0.3.0-beta.4, `dioxus-native` 0.8.0-alpha.3. Для продукта рано. Ценность — запасной путь:
тот же RSX и CSS со временем сможет работать без системного webview.

**Проверка.** Документация предлагает сравнение SSR-строк (`dioxus-ssr` + `pretty_assertions`), ручной
прогон `VirtualDom` и Playwright для web, liveview и fullstack; про desktop раздела нет
(<https://dioxuslabs.com/learn/0.7/guides/testing/web>). В 0.8.0-alpha.1 для Native появились
синтетические клики для тестов (#5658).

**Вывод.** Проще Tauri по устройству (один процесс, без WASM и IPC), но слабее по обновлению и по
сквозной проверке настольной сборки, и сейчас на пороге ломающей версии 0.8.

### 3.4. Slint

Версия 1.19.0 вышла 2026-11-16; линия 1.x идёт с 2023-04-03 без смены мажорной версии; выпуски раз в
2–3 месяца (1.15 — февраль, 1.16 — апрель, 1.17 — июнь, 1.18 — сентябрь 2026). Интерфейс
описывается на собственном языке `.slint`, который компилируется в Rust; логика на Rust. Это не
TypeScript, но и не «только Rust» — вопрос к основателю, если это для него принципиально.

- Отрисовка: FemtoVG (OpenGL), Skia, программный рендер, экспериментальный Vello (с 1.18); окна через
  winit или Qt.
- Текст: `StyledText` с макросом `@markdown`; в 1.19 добавлены `selectable` и функции выделения и
  копирования; встроенный код рисуется моноширинным шрифтом (CHANGELOG). Таблицы внутри `StyledText` не
  проверял. Есть `StandardTableView`, `ListView` с виртуализацией, `DragArea`/`DropArea`, `backdrop-blur`
  (1.19), переводы.
- Доступность: свойства `accessible-*`, AccessKit; много исправлений в 1.18–1.19 (CHANGELOG).
- Проверка: тестовый backend (без пикселей), `Window::take_snapshot()`, `slint-viewer --screenshot`
  без оконной системы (1.17+), встроенный MCP-сервер (1.17+): «walk the UI tree, read accessibility
  properties, screenshot, and simulate clicks, drags, and typing in the running app»
  (`ai-plugins/skills/slint/reference/debugging-and-mcp.md`). Тестовый API назван «preliminary» и лежит во
  внутреннем crate с точным закреплением версии (`internal/backends/testing/README.md`).
- Для агентов: официальный плагин и skill (<https://github.com/slint-ui/ai-plugins>), `llms.txt`.
- Лицензия: GPL-3.0, либо бесплатная Royalty-free 2.0 для настольных, мобильных и веб-приложений с
  условием показать виджет `AboutSlint` или значок на странице загрузки, либо коммерческая
  (`LICENSES/LicenseRef-Slint-Royalty-free-2.0.md`). Запрет: «The License does not permit the distribution
  of Application that exposes the APIs ... of the Software».
- Размер Hello World около 21 МБ по измерению авторов GPUI Kit (Slint 1.18.1, Linux x86-64).

Слабое место для нас — нет готовых частей чата и богатого Markdown: ленту, таблицы в ответах и вложения
придётся собирать самим на более узком наборе примитивов.

### 3.5. egui

0.37.0 (2026-10-15), 32,3 тыс. звёзд, 31,7 млн загрузок. Немедленный режим. Отрисовка glow или wgpu.

- Текст: шейпинг через `harfrust` (с 0.35), рендер через `skrifa` (CHANGELOG); с 0.37 «egui can now render _any_
  glyph that is supported on your system, including colored emojis and non-Latin scripts» через системные
  шрифты. Markdown — сторонний `egui_commonmark` 0.26.0; таблицы — `egui_extras`. IME улучшен в 0.35.
- Доступность: AccessKit, заметные улучшения в 0.37.
- Проверка: `egui_kittest` (запросы по дереву AccessKit, снимки через wgpu, запись видео теста);
  протокол инспекции и `egui_mcp`: «It allows reading the accesskit tree of a running app, as well as
  sending events to control it» (0.35, 2026-06-25).
- Оформление: система стилей в стиле CSS в работе (классы в 0.35, экспериментальные темы в 0.37).

Вывод: агенты пишут egui лучше всего и проверять его проще всего, но до уровня Linear и Zed по
типографике, анимации и раскладке его придётся тянуть вручную против устройства библиотеки. Годится для
внутренних инструментов и стенда отладки движка, не для окна заказчика.

### 3.6. Iced

0.14.0 вышла 2025-12-07, предыдущая 0.13 — 2024-09; master — 0.15.0-dev. В 0.14: реактивная отрисовка,
API анимации, IME (#2777), виджет `table`, инкрементальный разбор Markdown, тестирование без дисплея
(#2698) и сквозные тесты (#3059), исправление «Hotkey combinations not working on non-latin keyboard
layouts» (#3134) **[проверено по CHANGELOG]**. В master у Markdown есть таблицы и `selectable`; попали ли
они в 0.14 — не проверял.

Доступности нет: в репозитории нет ни одного упоминания `accesskit` **[проверено поиском по клону]**,
задача открыта с 2020 года (<https://github.com/iced-rs/iced/issues/552>). Документация — неполная книга;
дорожная карта — внешняя картинка. Выпущенные приложения: рабочий стол COSMIC (на форке `libcosmic`),
Halloy.

Вывод: нет доступности, редкие выпуски с большими ломающими изменениями, мало документации для агентов.

### 3.7. Freya

0.5.2 (2026-11-11), уже 0.6.0-alpha.1 (2026-11-15). В 0.4 (июль 2026) переписан и больше не зависит от
Dioxus **[вторично]**. Skia, winit, AccessKit 0.25. Есть `MarkdownViewer` с таблицами, редактор кода,
анимации, `freya-testing` («test your Freya components in a no-window (headless) environment», умеет
сохранять кадр в файл), skills для агентов, `llms.txt`. 3,4 тыс. звёзд, 50 тыс. загрузок, по сути один
автор. Технически интересен, но риск сопровождения слишком велик для продукта заказчика.

### 3.8. Xilem и Masonry

0.4.0 (2025-10-29) на crates.io, репозиторий активен (последний коммит 2026-11-03). README: «An
experimental Rust architecture for reactive UI». Стек Linebender: Vello, Parley, AccessKit — лучший
открытый текстовый стек на Rust, на нём же стоит Blitz. `masonry_testing`: стенд без дисплея, управление
временем, снимки экрана с макросом `assert_render_snapshot`. Набор виджетов мал, готового Markdown и
таблиц не нашёл **[не проверено]**.

### 3.9. Floem

0.2.0 от 2024-11-14, новых выпусков на crates.io нет два года; последний коммит 2026-09-24. Доступности
нет (в манифестах нет `accesskit`). Сделан для редактора Lapce. Не брать.

### 3.10. Makepad

`makepad-widgets` 1.0.0 от 2025-05-13; репозиторий активен. README описывает проект как «AI-accelerated
application and game development environment for Rust» со скриптовым языком Splash. По сторонним
источникам, «Makepad 2.0» с заменой `live_design!` на `script_mod!` вышел в феврале 2026 года
**[вторично]**; на crates.io этого нет. IME в 1.0 авторы называли недоделанным **[вторично]**.
Доступность и стенд без дисплея не нашёл. Приложения: Robrix (клиент Matrix, активен), Moly AI (клиент
LLM, последний коммит 2026-03-27). Не брать: свой язык, нестабильная поставка, слабый ввод текста.

### 3.11. Vizia

0.4.0 (2026-04-23), 2,4 тыс. звёзд, 7 тыс. загрузок. AccessKit и Markdown включены по умолчанию
(`Cargo.toml`). Основное применение — интерфейсы аудиоплагинов. Слишком мало пользователей и кода.

### 3.12. Прочее

- **Qt Bridges for Rust** — открытая бета от Qt, доступ к Qt Quick из Rust **[вторично]**
  (<https://www.qt.io/blog/qt-bridges-public-beta-for-rust>). Зрелый тулкит с настоящей доступностью, но
  это C++ под капотом, QML и лицензии Qt.
- **Native Shell** (`longbridge/nsh`, 2,9 тыс. звёзд) — родственный GPUI Kit проект «AI generate UI»;
  backend для GPUI ещё в разработке (README GPUI Kit).
- **Rune**, Tessera UI, Ribir, Cushy, Kas — новые или малые проекты без выпущенных приложений
  **[вторично / не проверено]**.
- **relm4 / gtk4-rs** — хороши на Linux, чужеродны на Windows и macOS.

## 4. Проверка без дисплея

| Инструмент | Кадр в картинку без дисплея | Управление через дерево или стенд | Снимки в CI на Linux | Для агента |
|---|---|---|---|---|
| GPUI Kit | да: `HeadlessAppContext`, `Window::render_to_image`; Linux — wgpu, годится программный Vulkan (lavapipe); macOS — Metal; Windows — нет в закреплённом снимке | да: `gpui_kit::test` — поиск по `ElementId`, имени и роли AccessKit; клик, ввод, клавиши, перетаскивание, прокрутка; `wait_for` | события — на трёх ОС; пиксели — Linux и macOS; готового сравнения с эталоном нет | skills, `llms-full.txt` |
| Tauri + WASM | да: интерфейс в headless Chromium; целое приложение — WebDriver под виртуальным дисплеем (xvfb) | да: WebDriver и дерево доступности Chromium | да | `llms.txt`; тестовые средства на JS |
| Dioxus desktop | только web-сборка в headless Chromium | SSR-строки, `VirtualDom`; для настольной сборки готового нет | для web — да | — |
| Slint | да: программный рендер, `slint-viewer --screenshot`, `take_snapshot()` | да: `ElementHandle`, запросы по accessible-label; MCP | да | плагин, skill, MCP |
| egui | да: `egui_kittest` со снимками через wgpu | да: запросы по AccessKit | да | `egui_mcp` |
| Iced | да: `iced_test` (`Simulator::snapshot`, `Emulator::screenshot`) | да: селекторы по тексту и id; файлы `.ice` | да | — |
| Freya | да: `render_to_file` | да: `TestingRunner` | да **[не проверено в CI]** | skills |
| Masonry | да: `assert_render_snapshot` | да: `TestHarness`, включая события доступности | да | — |
| Floem, Makepad, Vizia | не найдено | не найдено | — | у Makepad «AI automation inside Studio» |

Подробности по двум основным вариантам.

**GPUI Kit [проверено, `website/docs/test.md`].** Три уровня: обычные `#[test]`; `#[gpui_kit::test]` с
`TestAppContext`; интеграционные тесты интерфейса: «renders real components or an application view in a
headless window, simulates clicks, keyboard input and scrolling, then verifies state, focus, layout and
application callbacks». Включается feature `test-support`. Снимок элемента отдаёт роль, имя, значение,
состояние, границы и видимость. Ограничения, названные самими авторами: произвольный отрисованный текст
не находится («Kit therefore has no general rendered-text `HasText` assertion») — проверяемым элементам
нужны id или имена доступности; полноценный IME не моделируется; пиксельные проверки — «sensitivity
checks, not a complete golden-image suite»; на Windows «pixel verification is skipped because the pinned
platform crate has no Windows headless renderer». В основной GPUI поддержка уже шире: PR #64718
«gpui_wgpu: Add Linux headless rendering» (2026-09-25), #64969 «Cross-platform headless windowing API and
linux impl» (2026-10-02), #63012 «gpui_windows: Implement render_to_image for headless window capture»
(2026-08-26).

**Tauri [проверено, tauri-docs].** Рекомендуемый путь — WebdriverIO с `@wdio/tauri-service`: «works on
**Windows, Linux, and macOS**», по умолчанию встроенный сервер WebDriver внутри приложения через
`tauri-plugin-wdio-webdriver`. На Linux в CI «WebDriver tests are executed on Linux by creating a fake
display» (xvfb, `webkit2gtk-driver`). Есть «browser mode»: интерфейс в обычном Chrome без сборки Tauri.
Этот инструментарий на JavaScript; для Rust есть клиенты WebDriver `thirtyfour` 0.38.3 и `fantoccini`
0.22.2. Нужен ответ основателя: считается ли виртуальный дисплей xvfb допустимой «проверкой без дисплея».

Довод в пользу webview-стека: проверка в headless Chromium на Linux идёт на том же семействе движка, что
WebView2 на Windows, то есть хорошо предсказывает поведение у большинства пользователей. У GPUI проверка
на Linux идёт через wgpu, а у пользователей на Windows — через другой backend (DirectX 11).

Общее для обоих: проверки без дисплея не покрывают установщик, подпись, системный IME, экранные дикторы,
драйверы видеокарт. Это прямо сказано и в документации GPUI Kit: «a headless snapshot ... cannot prove ...
the platform adapter exposes the node, or a screen reader announces».

## 5. Насколько хорошо агенты пишут под каждый инструмент

Измерений качества генерации по фреймворкам я не нашёл. Ниже — косвенные признаки и моя оценка.

| Инструмент | Звёзды | Загрузки всего / за 90 дней | Изменчивость API | Что дано агенту | Оценка |
|---|---|---|---|---|---|
| Tauri 2 | 113 тыс. | 45,1 млн / 18,8 млн | низкая в 2.x; впереди 3.0 | `llms.txt`, большая документация | высокая для оболочки; пара «Tauri + Rust-интерфейс» встречается реже, агент будет сползать к JS |
| egui | 32,3 тыс. | 31,7 млн / 10,0 млн | средняя | `egui_mcp`, kittest | высокая |
| Leptos | 21,5 тыс. | 6,2 млн / 1,9 млн | заметная (0.6 → 0.7 → 0.8, скоро 0.9) | книга | средняя: путает версии |
| Dioxus | 39,8 тыс. | 3,75 млн / 1,40 млн | заметная (0.5 → 0.6 → 0.7, альфа 0.8) | документация | средняя: путает версии |
| Iced | 32,0 тыс. | 3,37 млн / 0,83 млн | высокая между редкими выпусками | неполная книга | ниже средней |
| Slint | 24,6 тыс. | 2,57 млн / 1,07 млн | низкая | плагин, skill, MCP, LSP | средняя: свой язык, но быстрая проверка компилятором `.slint` |
| GPUI + GPUI Kit | 95 тыс. (Zed), 20,8 тыс. (Kit) | `gpui` 0,51 млн; `gpui-component` 0,62 млн, из них 0,53 млн за 90 дней | очень высокая | два skills, `llms.txt` 48 КБ и `llms-full.txt` 4,7 МБ, проверяемые рецепты | ниже средней без подсказок; приемлемая со skills и закреплённой версией |
| Freya | 3,4 тыс. | 50 тыс. / 12 тыс. | очень высокая | skills, `llms.txt` | низкая |

Наблюдения.

- У GPUI публичного кода мало, а API менялся; модель с устаревшими знаниями будет писать под версии
  2024–2025 годов. Противовес у GPUI Kit сильнее, чем у остальных: skills ставятся командой
  `npx skills add longbridge/gpui-kit`, есть каталог `examples/ai_recipes` с проверяемыми рецептами и
  командами приёмки. Сами авторы оговаривают: «These gates make compiler, runtime, and example failures
  reproducible for human and AI developers. They do not establish an AI model success rate».
- Rust сам по себе помогает: ошибка в API — это ошибка компиляции. Цена — длительность сборки.
- В webview-стеке агенты сильнее всего в CSS и разметке, слабее — в связке WASM и нативной части.
- Опубликованный опыт: разбор Paneflow (терминал для агентов на GPUI) описывает уход с Tauri («The
  webview API surface is too narrow for the kind of low-level keyboard, IME, and input grabbing a
  multiplexer needs») и ошибки с отрисовкой глифов, но не то, как агенты писали код
  (<https://dev.to/arthurj-dev/building-a-native-terminal-for-ai-coding-agents-in-rust-gpui-2bg4>).
  В заметках к egui 0.35 показано, как Claude управляет демо через `egui_mcp`. Строгих сравнений не
  найдено.

Практический вывод для любого стека: закрепить версии вместе с `Cargo.lock`, положить в репозиторий
документацию именно этой версии, каждому исполнителю давать рабочий образец экрана и команду проверки.

## 6. Упаковка, обновление, подпись

| Средство | Версия | Что делает | Замечание |
|---|---|---|---|
| Сборщик Tauri (`tauri-bundler` 2.10.5) | 2026-11-16 | deb, rpm, AppImage, MSI, NSIS, .app, DMG | MSI только на Windows |
| `tauri-plugin-updater` | 2.14.1 | обновление с подписью | только в приложениях Tauri |
| `cargo-packager` | 0.11.8 (2025-11-27) | NSIS, MSI (WiX), DMG, .app, deb, AppImage, pacman; свой crate обновления | для любого бинарника; выпусков год не было, репозиторий активен |
| Velopack | 1.2.161 (2026-09-29) | установщик и обновление, статический хостинг файлов | Rust помечен «Ready» (<https://docs.velopack.io/>); форматы по ОС не проверял |
| `cargo-dist` | 0.34.0 | архивы и установщики, свой механизм обновления | рассчитан на GitHub Releases **[не проверено]** |
| `self_update` | 1.3.3 | замена одного исполняемого файла | не для MSI, `.app` и deb |
| `dx bundle` | 0.7.10 | установщики для Dioxus | обновления нет |

- MSIX готово не делает ни одно из средств, которые я смотрел.
- Подпись: Windows — сертификат Authenticode, без него предупреждение SmartScreen; macOS — Developer ID
  и нотаризация. Кто и чем подписывает сборку и доступны ли нам эти сертификаты — вопрос основателю
  **[не проверено]**.
- У заказчика одна организация и закрытая сеть. Разумная схема: администратор ставит MSI один раз,
  дальше приложение сверяет версию с движком при подключении и берёт обновление с самого Spark, проверяя
  подпись нашим ключом. Тогда не нужны ни интернет, ни внешний сервер обновлений.

## 7. Связь приложения с движком

### 7.1. Транспорт

| Вариант | За | Против | Crates |
|---|---|---|---|
| WebSocket + JSON-RPC 2.0 | двусторонний: движок сам спрашивает одобрение и шлёт события; тот же контракт, что по stdio; читается глазами; работает и из webview | типы и версии схемы ведём сами | `jsonrpsee` 0.26.1; `tokio-tungstenite` 0.30.0; `axum` 0.8.9 |
| gRPC | строгая схема, генерация кода, потоки | HTTP/2 и protobuf; из webview только через gRPC-web; вторая схема рядом с JSON-RPC | `tonic` 0.14.9 |
| HTTP + SSE | самый простой поток событий, легко отлаживать | поток в одну сторону: одобрения и отмена идут отдельными запросами, сопоставление вручную | `axum`, `reqwest` |

Предложение: **один порт, WebSocket с JSON-RPC 2.0 для управления и событий, обычный HTTP на том же
порту для файлов** (вложения вверх, предпросмотр и установщик вниз).

- Ведущий уже решил: «внешним контрактом сделать JSON-RPC уровня SDK» (`plans/session-state.md`). По
  WebSocket тот же контракт идёт без изменений, меняется только кадрирование.
- Диалог одобрения — это запрос от сервера к клиенту; так устроено `session/request_permission` в ACP и
  `item/commandExecution/requestApproval` в Codex app-server (`research/harness-adapters.md`, разделы 2.1
  и 3).
- Модель медленная, сеть в офисе рвётся, ноутбук засыпает. Нужны рукопожатие `initialize` с версией
  протокола и возможностями, порядковые номера событий и довосстановление по курсору после переподключения.
  Журнал сессии — единственный источник истины, окно — его проекция.
- Очередь — подписка на состояние: место в очереди, кто впереди (если основатель разрешит показывать),
  оценка ожидания.
- Движок на той же машине: тот же WebSocket на `127.0.0.1` с токеном. Запуск движка дочерним процессом
  со stdio возможен, но это второй путь исполнения; без нужды не заводить.

Особенность GPUI: у него свой исполнитель задач, не tokio. В Zed для этого есть мост `gpui_tokio`
(«Initializes the Tokio wrapper using a new Tokio runtime with 2 worker threads»; Apache-2.0, но на
crates.io его нет), идея простая: сеть — в отдельном рантайме tokio, в окно — сообщения по каналу. Альтернатива — `async-tungstenite` 0.35.0. В Tauri нативная часть уже на tokio.

### 7.2. Как находят сервер и сопрягаются сопоставимые продукты

| Продукт | Поиск сервера | Сопряжение и защита | Источник |
|---|---|---|---|
| T3 Code | LAN, Tailscale, SSH, свой relay; список маршрутов | pairing выдаёт набор прав; токены cookie/bearer/DPoP; короткоживущие тикеты для WebSocket | `research/harness-adapters.md`, 1.10 |
| Codex app-server | нет | stdio по умолчанию; `--listen ws://IP:PORT` помечен «experimental and unsupported» | там же, раздел 2 |
| OpenCode | `--mdns`, домен `opencode.local` | HTTP basic auth через `OPENCODE_SERVER_PASSWORD`; события — SSE `GET /event` | <https://opencode.ai/docs/server/> |
| LM Studio (LM Link) | своя сеть устройств совместно с Tailscale | «end-to-end encrypted, thanks to Tailscale» | <https://lmstudio.ai/docs/lmlink> |
| Sunshine / Moonlight | часто адрес вручную | PIN с клиента вводится в веб-интерфейсе хоста; HTTPS с самоподписанным сертификатом и предупреждением браузера | <https://docs.lizardbyte.dev/projects/sunshine/latest/md_docs_2getting__started.html> |
| LocalSend | multicast UDP `224.0.0.167:53317`; запасной путь — HTTP-запрос на все адреса подсети | HTTPS; отпечаток устройства — SHA-256 сертификата; необязательный PIN | <https://github.com/localsend/protocol> |
| Syncthing, KDE Connect | широковещательный поиск в подсети | идентификатор устройства из сертификата, подтверждение с двух сторон | **[не проверено, по памяти]** |

Общее: автоматический поиск всегда дополняется ручным адресом, а доверие строится на отпечатке
самоподписанного сертификата и коротком коде.

### 7.3. TLS в локальной сети

Публичный сертификат на адрес в частной сети получить нельзя. Let's Encrypt с 2026-01-15 выдаёт
сертификаты на IP-адреса сроком 160 часов
(<https://letsencrypt.org/2026/01/15/6day-and-ip-general-availability.html>), но выпуск требует
подтверждения владения адресом извне, то есть для адресов вида 192.168.x.x не годится **[вывод, не
проверено]**. Остаётся самоподписанный сертификат движка и закрепление его отпечатка в клиенте.

В нативном клиенте это решается своим проверяющим в `rustls` 0.23. В webview — нет: страница не может
принять самоподписанный сертификат без предупреждения (см. Sunshine). Поэтому в варианте Tauri соединение
держит нативная часть.

### 7.4. Предложение для HoH

1. При первом запуске движок создаёт ключ и самоподписанный сертификат (`rcgen` 0.14.10), идентификатор
   экземпляра и имя («Spark в бухгалтерии»).
2. Движок объявляет себя в mDNS/DNS-SD как `_hoh._tcp.local` с именем, версией протокола и началом
   отпечатка. Crate `mdns-sd` 0.21.7: Windows, macOS, Linux, без привязки к асинхронному рантайму;
   авторы пишут «This is still beta software» (<https://github.com/keepsimple1/mdns-sd>). Совместная
   работа с Avahi на Spark не проверена.
3. Приложение показывает найденные движки. Поиск ненадёжен там, где сеть разбита на сегменты, включена
   изоляция клиентов или мешает брандмауэр Windows **[не проверено]**, поэтому рядом два запасных пути:
   адрес вручную и **строка-приглашение** (адрес, отпечаток, одноразовый код), которую администратор
   пересылает сотруднику. Для нетехнического пользователя вставить строку надёжнее всего.
4. Сопряжение: администратор получает одноразовый код с коротким сроком (командой на Spark или из уже
   сопряжённого приложения с правами администратора). Пользователь вводит код. Код должен подтверждать
   отпечаток сертификата, иначе его можно перехватить посередине: либо обмен ключами по паролю
   (`spake2` 0.4.0), либо отпечаток внутри строки-приглашения. Какой из двух — решить при проектировании
   с проверкой безопасности.
5. Итог сопряжения — токен устройства, привязанный к пользователю; хранится в системном хранилище
   ключей (`keyring` 4.2.0); на движке есть список устройств с отзывом.
6. При каждом подключении — сверка версий протокола и приложения; при расхождении приложение предлагает
   обновиться с движка (раздел 6).

Чего не хватает во входных данных: кто у заказчика играет роль администратора и есть ли у него доступ
к консоли Spark.

## 8. Рекомендация

### Вариант 1: GPUI + GPUI Kit

За:

- Нативное окно того же класса, что Zed; ни HTML, ни webview. Буквально соответствует словам основателя.
- Готовы именно наши части: лента сообщений, потоковый Markdown с таблицами и выделением, вложения,
  опрос, формы, диалоги, таблица данных, настройки, анимация с пружинами.
- Стенд без дисплея с запросами по ролям и именам доступности и отрисовка в картинку на Linux без
  видеокарты (программный Vulkan), то есть без очереди GPU.
- Apache-2.0, без условий.
- Один язык и один процесс.

Против:

- Всё до версии 1.0; еженедельные ломающие выпуски; GPUI приходит снимками от одной сторонней компании.
- Windows — самая важная для нас и самая молодая платформа GPUI; нужен DirectX 11; поведение на слабых
  офисных машинах и по RDP неизвестно.
- Доступность частичная.
- Упаковку, обновление и подпись собираем сами.
- Нет русских строк компонентов и просмотра документов.
- Агентам нужна подпорка: skills, документация закреплённой версии, образцы.

### Вариант 2: Tauri 2 + Leptos (WASM)

За:

- Лучшие текст, выделение, IME, доступность и анимация — от браузерного движка.
- На Windows внутри Chromium; проверка в headless Chromium на Linux это поведение предсказывает.
- Готовые установщики и обновление с подписью; линия 2.x идёт с 2024-10-02 без смены мажорной версии.
- Наш уже существующий способ проверки вида (headless-браузер, снимки, CSS-токены) переносится без
  изменений.
- Больше всего публичного кода и документации.

Против:

- Это страница в webview. Если основатель под «не страница в браузере» имел в виду и это, вариант отпадает.
- Linux с NVIDIA (включая сам Spark) и различия WKWebView на macOS.
- Две цели сборки и граница IPC; обёртку над IPC пишем сами; пара «Tauri + интерфейс на Rust» менее
  обкатана, чем с JavaScript.
- Дизайн-система целиком своя на CSS.
- Tauri 3 сломает часть API; Leptos 0.9 на подходе.
- Больше памяти: процессы webview **[не измерено]**.

Разновидность варианта 2 — Dioxus desktop: один нативный процесс без WASM и запасной путь на Blitz.
Выбирать его вместо Tauri, если важнее простота устройства, чем готовое обновление и сквозные тесты.

### Что заставит сменить вариант

С первого на второй:

- основатель допускает HTML/CSS в окне и ставит срок первой версии выше нативности;
- проба на Windows провалена: окно не запускается или тормозит на типовой машине заказчика, не работают
  сочетания клавиш в русской раскладке, ломается ввод или перетаскивание;
- доступность для экранного диктора — обязательное условие первой версии;
- два обновления GPUI Kit подряд стоят больше дня работы каждое;
- нужен встроенный просмотр PDF или HTML прямо в окне, а не картинками от движка.

Со второго на первый:

- основатель считает webview «страницей в браузере»;
- у заметной доли пользователей Linux или старые macOS;
- запрещён любой JavaScript в поставке, включая склейку внутри зависимостей.

### Как снизить цену ошибки

- Слои: `hoh-protocol` (типы и версии), `hoh-client` (транспорт, сопряжение, хранилище состояния, модели
  экранов; без UI), `hoh-ui` (окно). Первые два одинаковы для обоих вариантов и проверяются без
  отрисовки против настоящего движка.
- Проба до основной работы, на закреплённых версиях: один экран чата с потоковым Markdown и таблицей,
  форма с вложением перетаскиванием, диалог одобрения, индикатор очереди; сборка под Windows; прогон
  тестов без дисплея на Linux. Условия приёмки пробы записать заранее.
- Закрепить `gpui-kit` и `gpui-pre` точными версиями, обновлять по расписанию отдельной задачей.

## 9. Вопросы, на которые может ответить только основатель

1. Допустима ли вёрстка HTML/CSS внутри окна, если приложение при этом отдельное, со своим установщиком
   и без адресной строки? Или окно должно рисоваться нативно? (Уже стоит в очереди вопросов под номером 6.)
2. Распространяется ли запрет TypeScript на JavaScript-склейку внутри зависимостей (она есть у Tauri и
   Dioxus) и на инструменты тестирования, не входящие в продукт?
3. Допустим ли свой декларативный язык интерфейса, компилируемый в Rust (Slint), или интерфейс должен
   быть написан только на Rust?
4. Какие машины у сотрудников заказчика: версии Windows, есть ли машины без видеокарты, работают ли через
   RDP или тонкие клиенты? Сколько пользователей на macOS и Linux на самом деле?
5. Есть ли у нас Windows-машина или разрешение на виртуальную машину для сборки и приёмки? Проверки без
   дисплея на Linux не покрывают установщик, DirectX, WebView2 и системный ввод.
6. Считается ли виртуальный дисплей (xvfb) допустимой проверкой «без дисплея»?
7. Есть ли в сети заказчика интернет? От этого зависят установка WebView2 и источник обновлений.
8. Кто у заказчика администратор: кто ставит приложение, выдаёт коды сопряжения, имеет доступ к Spark?
9. Нужны ли учётные записи пользователей на движке; видит ли сотрудник чужие задачи и чужое место в
   очереди?
10. Обязательна ли доступность для экранного диктора в первой версии или это пожелание?
11. Какие документы нужно показывать в предпросмотре (PDF, DOCX, XLSX, сканы) и достаточно ли вида
   «как отрисовал движок»?
12. Чем подписываем сборки для Windows и macOS; готовы ли к предупреждению SmartScreen на первой поставке?
13. Открытый ли код и какая лицензия у продукта (влияет на Slint и на любое заимствование из Zed).
14. Разрешение на установки для разработки: системные библиотеки (Vulkan и Wayland для GPUI либо
   WebKitGTK для Tauri), цели Rust, средства упаковки.

## 10. Что не проверено

- Ни один инструмент не собирался и не запускался; оценки качества текста и анимации — по документации,
  исходникам и чужим приложениям.
- Память и размер: только чужие приблизительные цифры для Hello World; своих измерений нет.
- GPUI на Windows без видеокарты и по RDP; кросс-сборка GPUI под Windows с Linux.
- Сочетания клавиш в русской раскладке на Windows в GPUI.
- Как экранные дикторы (NVDA, Narrator, VoiceOver, Orca) читают приложения на GPUI Kit.
- Таблицы в `StyledText` у Slint; состав Markdown в выпущенной Iced 0.14; набор виджетов Xilem.
- Источник обновлений в `tauri-plugin-updater`, форматы Velopack по ОС, привязка `cargo-dist` к GitHub.
- Совместная работа `mdns-sd` с Avahi на DGX OS; поведение mDNS за брандмауэром Windows и в сегментированной
  сети.
- Выдача сертификатов Let's Encrypt на частные адреса (вывод из условий проверки, не из документа).
- Дата и состав «Makepad 2.0»; состояние доступности в Makepad и `libcosmic`.
- Сроки поддержки WebView2 на Windows 10 — по пересказам, документ Microsoft не читал.
- Приложения на Tauri с интерфейсом на Leptos в продуктовой эксплуатации не найдены.
- Сравнений качества генерации кода агентами по фреймворкам не найдено.
