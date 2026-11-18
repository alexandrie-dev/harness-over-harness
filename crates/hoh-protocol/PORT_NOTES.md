# hoh-protocol — заметки переноса

Источник: DeepSeek Harness, `packages/sdk/protocol/src/{types,transport}.ts`,
`packages/{llm/llm,interaction/user-questions,interaction/commands,attachment/attachment}/src/types.ts`.
Проект окна: `docs/window.md`, разделы 4 и 6. Пакет 2 выполнен перед пакетом 3.

Сохранены все 8 SDK-методов, 2 обратных запроса и 6 уведомлений, имена,
camelCase, варианты команд, inline/durable images, файлы, вопросы с intent,
опциональность. Идентичность сервера: `deepseek-harness-sdk-runtime`.
Расширяемые SessionEvent.data и stopReason остаются упорядоченным JSON: каталог
плагинов определяет их содержимое, транспорт его не сужает. Для известных
верхних payload есть отдельные serde-типы и schemars-схемы.

Кадры не зависят от процесса/stdio: `framing` содержит конверты и `Wire<T>`,
`transport` — первый адаптер над любыми BufRead/Write. Wire сначала проверяет
тип, но пишет исходный упорядоченный JSON, включая неизвестные поля вложенных
событий. Доступ к исходному JSON только для чтения. Для конструирования новых
кадров порядок полей соответствует types.ts. Исключение в самом dsh:
SessionQueryCompleted перечисляет finalResponse перед finishReason, но сервер
QueryActivity.finish отправляет наоборот. Wire сохраняет оба порядка.

Поведение transport.ts сохранено: trim и пропуск пустых строк, пропуск
невалидного JSON/некадров, отсутствие потолка длины строки, отбрасывание
незаконченного хвоста на EOF, UTF-8 с replacement, нормализация не-object params
в {}, числовые и строковые id, отсутствие проверки jsonrpc при приёме.
Ответ без pending (включая поздний после abandon) игнорируется.
Ошибка сохраняет необязательные code/data и fallback message; close навсегда
закрывает регистрацию, возвращая ожидающие запросы владельцу для отклонения.
Сами callback/executor/waiter и выбор защищённого сетевого канала — задача хоста.
Писатель выдаёт компактный JSON и LF, flush после каждого кадра.

## Решения там, где проект не задаёт поля

- hoh.protocolVersion = 1; ключ: hoh.approvalKey, 32 байта в 64 lowercase hex,
  только native host. capabilities.approvals = "host". Все добавления optional
  для старого SDK. В согласованном host-режиме обязательны HostApprovalRequest
  и HostApprovalDecision; нет понижения до простого outcome.
- Risk = low/medium/high; outcome ровно allowed-once/rejected/cancelled.
  actionHash и mac — lowercase hex SHA-256. MAC: HMAC-SHA256 по UTF-8
  канонический JSON-запись подтверждения. Срок 300 секунд, решение одноразовое.
  ApprovalRequestResult сохраняет исходный outcome первым; обязательный
  HostApprovalDecision имеет порядок approvalId, outcome, mac.
- Сотрудник: id/rev, name/role/rules/taskIds/permissions, статус
  working/waiting/sleeping, optional nextRunAt. Create возвращает id/rev,
  update — проверку rev, delete — id/новую rev (tombstone).
  Конфликт rev: JSON-RPC code -32009 с data {id,expectedRev,actualRev}.
- Permissions: internet ask/public/deny, domains/folders, writeFiles/programs,
  programNetwork и actAsUser. Секретов в Settings нет: model, security, view.
  Set — частичный объект этих трёх секций; результат — полные настройки.
  Ограничение вызова security из главного окна осуществляет пакет 4.
- Memory: itemId/text/updatedAt; list — массив, put — item, delete — {}.
  Open — sessionId. Все временные поля — Unix milliseconds.
- Task: id/title/description/version/permissions/active; list и versions —
  массивы; get/rollback — Task, delete — {}. input — открытый объект JSON,
  определяемый задачей. Knowledge — rules/sources(title,uri)/artifactIds;
  edit начинает повторную проверку и возвращает draftId/sessionId.
  Состояние отменённого черновика: failed (отдельного cancelled в проекте нет).
- Button create: {draft}, update: {id,patch}, delete: {id}; create/update —
  Button, list — массив, delete — {}. target — tagged task{id}/prompt{text};
  inputs — name/label/required. Patch не использует null для удаления:
  inputs можно очистить [], optional scalar сбрасывает delete/recreate.
- Schedule: те же CRUD-обёртки; when = once{at}, interval{everyMs},
  cron{expression,timezone}; target = taskId+employeeId либо employeeId+prompt.
  run-now — runId/sessionId; history — массив runId/scheduledAt/firedAt?/status
  (ok/error/missed/running). Только последний пропущенный доставляет движок.
- Artifact list: одна из областей runId/sessionId/employeeId; результат —
  массив. Read: байтовые offset/length, default length и верхняя граница 1 MiB.
  encoding utf8/base64; данные бинарных форматов только base64.
  Render: нумерация страниц с 0; tagged result pages{pages,totalPages},
  table{columns,rows,truncated}, text{text,truncated}; PNG pages — mime/encoding/data.
- session/events: afterSeq exclusive, по умолчанию -1; limit 1..1000;
  nextSeq — последняя доставленная seq, либо прежний курсор, если событий нет.
  list.employeeId — фильтр, sessions[].employeeId обязателен.
- Changed: id и change created/updated/deleted; employee дополнен rev,
  task — version. События example/presented используют artifactId, без пути.
  Draft errors — optional error string; egress — domain/employeeId/size/at.
  ApprovalPending содержит deadline expiresAt, без ключа или mac.

Отдельные схемы результатов помечены x-method для корреляции. Экспорт:
`hoh-protocol-schema [каталог]`, по умолчанию `protocol/schema`.
Затем `node scripts/gen-protocol-ts.mjs`. Генератор использует только встроенные
модули Node; `--check` не пишет файл. Тест сравнивает и схемы с Rust, и TS
со схемами; каждый fixture проверяется типом и побайтным кругом с LF.
Новые варианты событий/поля требуют обновлять схемы и fixtures.
Общие поля конверта рядом с oneOf/anyOf/allOf сохраняются как пересечение
типов; отдельный регрессионный тест проверяет jsonrpc/id вместе с методом.
В наборе 49 методов (включая два обратных запроса), 16 уведомлений,
122 примера и 105 экспортируемых схем. Опциональные поля не превращаются
в nullable; локализованный текст approval display обязательно содержит en.

## Неточности проекта

SDK уже имеет working-directory/get|set и user-question.cancelled: они также
включены. В разделе 3 имена методов сокращены без hoh/, на проводе действует
раздел 4. Employee/list не может возвращать один Employee: выбран массив.
Поля Button target id|text требуют tagged union. Путь MAC требует алгоритма,
кодировки, передачи ключа и запрета downgrade — выше они зафиксированы.
Общий session.event SDK содержит durable данные с путями: ограничение окна
«не получает пути» обеспечивает фильтр/проекция ядра, один artifactId не
очищает произвольные SDK-события.

## Итоговая проверка

`cargo test -p hoh-protocol -p hoh-window-stub`: для hoh-protocol
3 unit + 4 contract, 0 ошибок. Все 122 fixtures прошли побайтный круг;
схемы совпали с Rust, TypeScript — с выводом Node-генератора.
`cargo clippy -p hoh-protocol -p hoh-window-stub --all-targets -- -D warnings`
и `cargo fmt -p hoh-protocol -p hoh-window-stub -- --check` прошли.
Для всех cargo-команд: CARGO_NET_OFFLINE=true, CARGO_BUILD_JOBS=4,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui1.


## Раунд исправлений по ревью

Этот раздел уточняет первоначальные решения выше.
Итог раунда: 3 unit + 4 contract + 5 review_regressions = 12 passed; 0 failed.
Общие clippy --all-targets -- -D warnings, fmt -- --check и
node scripts/gen-protocol-ts.mjs --check прошли. Проверка tsc не запускалась:
генерируются только объявления, компилятор TypeScript не установлен.
PRIVILEGED_METHODS: employee/create, employee/update (patch.permissions),
task/draft/decide (yes), task/rollback, settings/set (security).
Движок удерживает ответ до host approval с MAC; legacy-режим эти операции
не разрешает. Повторный initialize не меняет ключ; host acknowledgement —
hoh:{protocolVersion:1,approvals:"host"}.

Каноническое действие — объект {method,params}; ключи объектов рекурсивно
сортируются по UTF-8, массивы сохраняют порядок. Компактный JSON в UTF-8,
без BOM и завершающего LF, разделители ',' и ':', JSON escaping serde_json.
actionHash — lowercase SHA-256 этих байтов. MAC — HMAC-SHA256 компактного
JSON с фиксированным порядком approvalId,outcome,actionHash,risk,humanText,expiresAt.
Разделитель '|' больше не используется. ID: ASCII [A-Za-z0-9_.:-], 1..128;
hash/mac — ровно 64 lowercase hex. HostApprovalRequest содержит action и expiresAt.
Проверка вынесена в hoh_protocol::approval; владелец pending потребляет запрос
один раз, включая ошибочные ответы. Независимые Node crypto-векторы и команда
их вычисления закреплены в tests/review_regressions.rs.
Истечение даёт cancelled/aborted без turn/end(error).

Числовые метаданные целые; maxTokens — положительное безопасное целое.
session/query запрещает дополнительные ключи. unavailable сохранён для журнала,
но не принимается как host consent. Typed Request принимает shutdown с params:{}
и отсутствие params для безаргументных методов. FrameReader::with_line_limit —
ограниченный сетевой режим; stdio по умолчанию без лимита.

Примеры теперь сериализуются типизированными структурами, а не исходным Value.
Wire<T> остаётся отдельным API сохранения чужого порядка ключей.
Генерация разделена: protocol/core.ts — native API с ключом и host requests,
ui/src/protocol.ts — только разрешённые окну методы и достижимые типы.
Никакого импорта core-модуля из window-модуля нет.

