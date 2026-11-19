# hoh-window-stub — заметки переноса

## Круг fix2e, 19.11

humanText для rules/set называет конкретную папку и словами описывает режим
интернета: с запросом разрешения либо к общедоступным сайтам. Подтверждение
model/key/set называет адрес текущего провайдера без API-ключа; адрес берётся
из baseUrl самого запроса и входит в подписанное действие рядом с keyHash.
Первый запуск передаёт введённый адрес вместе с ключом. Model settings
применяются только после MAC и отражаются в ответе set и последующем get.
Текст probe называет baseUrl и модель из запроса. Значения заключены в кавычки
с экранированием; общая проверка длины/невидимых символов остаётся обязательной.
Отдельный тест сверяет тексты в approval/request и approval.pending.

Подписанный rejected публикуется как rejected; отложенный эффект отбрасывается.
Для replay отказ, отличающийся от записанного разрешения, отменяет действие;
записанный отказ воспроизводится по-прежнему. Неверный MAC не позволяет выдать
подтверждённый rejected; истечение продолжает публиковать cancelled.
Тесты проверяют уведомления, отсутствие хранения отвергнутого ключа и остановку
replay без ложного выполнения разрешённой ветки записи.

ModelProbeResult.models возвращается из fixture, без GET /models и сети.
Промышленный движок заполняет его из data[].id GET /models, либо [] при отсутствии
списка. Проверка бинарника закрепляет тестовый список.
cargo check -p hoh-protocol -p hoh-window-stub -p hoh-credentials --tests:
exit 0 (31m 37s вместе с очередью). Jobs=2, offline, target-ui1.
Экспортированы 109 схем, перепорождены protocol/core.ts и ui/src/protocol.ts;
Node-генератор --check прошёл. cargo test: hoh-protocol 19/0,
hoh-window-stub 23/0, hoh-credentials 20/0. Общий clippy --all-targets
с -D warnings и fmt --check для трёх крейтов прошли. Независимый read-only
review подтвердил закрытие runtime/security findings. T/Tl вручную не
возобновлялись.

## Сдача fix2, 19.11

Каталог target-ui1 восстановлен ведущим. Общий cargo check -p hoh-protocol
-p hoh-window-stub --tests завершился exit 0 (22m 53s, включая очередь).
Собран и выполнен hoh-protocol-schema, экспортированы 109 схем; Node-генератор
перепородил protocol/core.ts и ui/src/protocol.ts. Проверка генерации прошла.
hoh-protocol сдан отдельно: 18 тестов, 0 ошибок, clippy и fmt-check чисты.
Для заглушки fmt-check прошёл; cargo test: 3 unit + 18 integration = 21 passed,
0 failed (18m 48s вместе с очередью). text-turn, approval и plan-review прошли
через бинарник. cargo clippy -p hoh-window-stub --all-targets -- -D warnings:
exit 0 (25m 56s вместе с очередью). cargo fmt -p hoh-window-stub -- --check:
exit 0. node scripts/gen-protocol-ts.mjs --check: exit 0.
Итоги предыдущих кругов ниже не заменяют результат fix2.

Cargo-команды этого круга: CARGO_BUILD_JOBS=2, CARGO_NET_OFFLINE=true,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui1. Прежний блокер создания
удалённого каталога target устранён. Остановленные сборки T/Tl не возобновлялись.


## Круг fix2, 19.11 — первый запуск и новые gate

Initialize принимает только ключ длиной 32 байта, отличный от нулевого;
не согласованный host-mode ключ не принимается. Тесты заглушки используют
[0x42;32], а протокол имеет независимые Node-векторы с другим случайным ключом.

Расширен gate: смена cwd, назначение taskIds сотруднику, model/baseUrl и probe.
Добавлены hoh/model/key/set и hoh/settings/rules/set. Реестр действий единственный,
PRIVILEGED_METHODS порождается из той же таблицы, что predicate.
Все эффекты (ключ, правила, cwd и разрешение ключа для probe) происходят после MAC.
Произвольного исходящего HTTP в replay-бинарнике нет: probe — mock с fixture
latency/toolCalls, ok отражает наличие разрешённого сохранённого ключа.
Подтверждение хранения ключа содержит keyHash=action_hash({apiKey}) и baseUrl,
но не сам ключ.
Секрет не возвращается, не журналируется и не включается в Debug.
Приватный Deferred хранит SecretString, его Debug отсутствует.

Новые методы и signed native fixtures проверяются типизированным кругом.
Тесты подтверждают новый gate, отказ без MAC/replay, хранение ключа и использование
при следующем probe, отсутствие секрета в исходящих кадрах, сохранение правил.
Адаптер firstrun теперь вызывает методы протокола; pick_folder — команда Tauri.
В docs/window.md §4 зафиксированы engine_request, approval_current, approval_decide,
pick_folder с формами адаптеров. Реализация моста остаётся пакету 4.

## Вопросы ведущему

SHIM(hoh-credentials-local): credentials.rs — process-local CredentialProvider
через настоящий seam hoh-credentials (он done). credentials-local имеет writing:w1,
по interface-first его не меняли и не использовали нестабильный durable backend.
Для replay-бинарника хранение живёт до конца процесса; производственный движок
должен заменить shim на принятый durable provider. Ведущий разрешил memory-provider
для текущей заглушки; durable credentials-local — отдельный последующий пакет.
Пароль через JSON-RPC передаётся
из поля ввода один раз; ядро обязано не журналировать args этого метода.


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

