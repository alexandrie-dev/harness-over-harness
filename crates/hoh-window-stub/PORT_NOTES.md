# hoh-window-stub — заметки переноса

Пакет 3: протокольная заглушка для окна, без модели, сети, запуска инструментов
или исполнения программ из записи. Источники только читаются:
`snapshots/sdk/{text-turn,approval,plan-review}/session.v4.jsonl`,
`snapshots/sdk/sdk.snapshot.ts`, SDK server/query-activity/user-questions,
plan-mode и `crates/hoh-conformance/README.md §5.1`.

Запуск: `hoh-window-stub --scenario text-turn`. Значение по умолчанию —
text-turn; корень dsh — `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`.
Можно задать HOH_DSH_ROOT, --source-root, --recording (отдельный V4 JSONL),
--fixtures. Пути нужны только самому процессу и не определяют транспорт.
На stdout только JSON-RPC, диагностика на stderr. --help печатает справку.

## Что сохранено

- Те же initialize/serverInfo, session/prompt/query/resume/wait,
  working-directory/get|set, shutdown. Параметры проверяет hoh-protocol.
- Чтение/запись кадров через независимый адаптер hoh-protocol.
  Один ограниченный канал из потока чтения позволяет принимать cancel, wait,
  shutdown и ответы, пока запись ждёт обратный запрос.
- Полный порядок событий родительской V4-сессии. Заголовок не уведомление;
  seq — индекс события без заголовка; time и времена потока нормализованы в 0,
  как SDK snapshot normalizer. {{session:1}} и {{cwd}} подставляются рекурсивно
  из запроса/initialize. Остальные snapshot placeholders остаются.
- Первые три policy-события уже существуют до SDK-подписки. Они доступны
  через session/events, но не повторяются в live-потоке. Первое уведомление —
  inbox/spliced, затем running перед turn/start, turn/end перед idle.
- Prompt выдаёт receipt с записанным messageId после inbox. Query отличает
  prompt от command. command/done всегда раньше command-response. Команда с
  вводом отвечает после done, completed отправляется после idle; команда без
  ввода отправляет completed и ответ после done. Порядок completed-полей
  соответствует реальному QueryActivity.finish.
- approval/asked сначала виден событием, затем отправляется approval/request.
  До ответа approval/decided и продолжение не выдаются. Для старого SDK
  требуются approvalRequests и только outcome, сессия/инструмент/reason
  совпадают с SDK harness. Ответ обязан совпасть с записью.
- exit_plan_mode порождает user-question/ask после tool/call с точным plan
  detail, header, options/descriptions и intent.callId. Ответы сценария
  plan-review: Keep planning + Add rollback., Approve, cancelled:true.
  Несовместимый ответ завершает ход ошибкой, а не воспроизводит ложный результат.
  Вопросы требуют capabilities.userQuestions; поздний ответ после cancel игнорируется.

## Новое и открытые решения

HoH/* отвечает соответствующим `protocol/fixtures/<method>.response.json`,
проверяя результат типом и заменяя только JSON-RPC id. Это статические
демонстрационные данные, CRUD их не меняет. session/list/events показывают
созданные виртуальные сессии и фактически выданную историю; limit 1..1000,
afterSeq >= -1 и exclusive. Синтетическое завершение после cancel/ошибки также
сохраняется в history. Employee для session/list — employee-1.

В host-режиме initialize требует hoh.protocolVersion=1 и approvalKey (32 байта,
64 lowercase hex). Обратный approval/request дополнен approvalId/risk/high/
humanText/actionHash и approval.pending. HumanText строится из имени действия;
reason остаётся отдельным полем. actionHash — SHA-256 компактного JSON
исходных sessionId/toolName/callId?/reason? с сортировкой ключей. Ключ наружу
не выдаётся. HMAC-SHA256 по UTF-8 канонический JSON-запись подтверждения, lowercase hex.
Сравниваются все 64 байта; исходный ключ фиксированной длины, HMAC проверен
RFC 4231 case 1. Требуются approvalId и mac, downgrade до {outcome} запрещён.
Срок 300 секунд по монотонным часам; expiresAt в уведомлении — Unix ms.
Каждый ответ потребляется один раз. Истечение, cancel, shutdown закрывают
ожидание; отмена вопроса выдаёт user-question.cancelled. Обычные вопросы и
legacy approvals бессрочные, как в SDK.

Ошибочный обратный ответ создаёт turn/end(error) и idle; очередь wait получает
JSON-RPC error. Cancel выдаёт turn/end(aborted), idle и завершает уже ожидающие
wait, включая wait после отмены. Такой replay нельзя продолжать: следующий
ввод возвращает ошибку, поскольку в записи нет альтернативного будущего. Для повторения нужен
новый sessionId или процесс. Shutdown завершает ожидания и выходит.

## Границы заглушки

Один активный action на процесс, независимый курсор для каждого sessionId.
Это сознательное ограничение тестовой заглушки, не ограничение протокола.
Запись воспроизводится до следующего turn/end или command/done без модельного
ввода. Цель проверенных сценариев — parent-сессия; child logs/subagent lifecycle,
произвольные goal continuation chains и внешние side effects здесь не
симулируются. Для ask_user_question из произвольной записи возвращается
структурированный вопрос и проверяются id ответов; строгая проверка записанного
выбора реализована для plan-review и approval. Resume открывает виртуальную
сессию в начале записи, а не восстанавливает живой Agent с диска.
working-directory/set меняет виртуальное значение, не проверяя существование
папки и не меняя процесс cwd. Model/settings и артефакты — fixtures, не сервисы.

В V4 JSONL нет исходного порядка agent/status относительно внешних callbacks:
lifecycle восстанавливается по SDK и проверяется независимо golden text-turn.
В самих документах не описаны shape HoH CRUD, подпись/кодировка ключа и поведение
при несовпадении ответа с записью: решения выше и в hoh-protocol/PORT_NOTES.md.

## Проверка

Интеграционные тесты запускают настоящий бинарник с pipe stdin/stdout:
text-turn целиком сравнивается с notifications.expected.jsonl и finalResponse;
approval — оба решения и весь журнал; plan-review — все четыре хода,
три команды/вопроса, keep/approve/dismiss, command completed, 6 model steps.
Отдельно: все HoH fixture-ответы, host MAC, отсутствие downgrade, cancel вопроса
и поздний ответ. Нет npm, вызовов dsh/модели или внешних сервисов.

Итоговый `cargo test -p hoh-protocol -p hoh-window-stub`: для заглушки
1 unit + 6 интеграционных тестов, 0 ошибок. Все три сценария прошли через
бинарник. `cargo clippy -p hoh-protocol -p hoh-window-stub --all-targets -- -D warnings`
и `cargo fmt -p hoh-protocol -p hoh-window-stub -- --check` прошли.
Для всех cargo-команд: CARGO_NET_OFFLINE=true, CARGO_BUILD_JOBS=4,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui1.


## Раунд исправлений по ревью

Этот раздел уточняет первоначальные решения выше.
Итог раунда: 1 unit + 11 pipe integration = 12 passed; 0 failed.
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

