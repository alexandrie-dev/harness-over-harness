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
