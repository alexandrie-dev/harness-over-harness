# Ревью: отложенные пункты движка после вехи (x7, 20.11)

Ты — независимый ревьюер, только чтение. НЕ запускай тесты, сборки, cargo, браузеры, серверы. Читай код и TS-источник.

Задание писателю: `/mnt/toshiba/alexandrie/hoh/logs/lead1118/x7-postmilestone.txt` (пункты 1–3). Смотри незакоммиченные изменения (`git diff`) в:
- п.1/2: `crates/hoh-sdk-jsonrpc-server/src/transport.rs`, `tests/assistant_stream.rs`, `tests/transport_bounded.rs`, `PORT_NOTES.md`; `crates/hoh-protocol/src/sdk.rs`, `tests/assistant_stream.rs`, `PORT_NOTES.md`; `protocol/core.ts`, `protocol/schema/Notification*.json`, `ui/src/protocol.ts`.
- п.3: `crates/hoh-bash-sandbox`, `crates/hoh-fs-sandbox`, `crates/hoh-sandbox-local`, `crates/hoh-sandbox-policy`, `crates/hoh-sandbox`, `crates/hoh-tool-fs/tests`, `apps/hoh/tests/window_profile.rs`, `apps/hoh/PORT_NOTES.md`, `apps/hoh/assets/bundle/window/cordis.patch.yml`.
Прочие изменения рабочего дерева (новые крейты `??`, design/, scripts/, PLAN.md, docs/) — чужие, не ревьюй.

Проверь:
1. Тест снятия подписки `agent/assistant-stream` реально падает, если disposer убрать.
2. Очередь уведомлений: память ограничена (кадры и байты, включая in-flight); ответы на запросы и не-chunk уведомления не теряются; при невозможности — соединение закрывается с ясной ошибкой; цикл агента не блокируется; кадр `gap` корректен по схеме (Rust/TS/JSON совпадают); нет гонок/дедлоков; тест медленного клиента осмыслен.
3. Хранилище ключа (DSH_HOME): модель профиля window не может прочитать/перечислить/записать/удалить его ни fs-инструментами, ни bash (cat/grep/ls/cp, перенаправления, подстановки, `find /`, `tar`, относительные пути через `..`, симлинки, жёсткие ссылки, /proc/self/root, после эскалации danger-full-access); движок сам ключ читает; рабочая папка не задета; путь канонизируется устойчиво (DSH_HOME ещё не создан, внутри рабочей папки, совпадает с ней). Найди обходы.
4. Расхождения с dsh, не записанные в PORT_NOTES.

Формат: каждое замечание — сценарий, наблюдаемое отклонение, доказательство (файл:строка), что нарушено; отдельно пометь дефект / нарушение требования / рекомендацию. Последняя строка: `ВЕРДИКТ: ЧИСТО` или `ВЕРДИКТ: ЗАМЕЧАНИЯ`.
