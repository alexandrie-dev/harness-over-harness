# hoh-user-approval ← packages/interaction/user-approval @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| src/index.ts | src/lib.rs, src/state.rs | ApprovalService/plugin, policy, private approvalState, runtime-context, audit and cancellation |
| src/types.ts | src/types.rs | ApprovalRequestId, closed outcomes, scoped request and audit data |
| tests/approval.spec.ts | tests/approval.rs | Все 32 исходных случая |
| tests/projection.spec.ts | tests/projection.rs | Все 9 исходных случаев; дополнительно schema, decided failure, dropped future и null config |
| Schemastery null default | src/shim.rs | Приватный SHIM(hoh-kernel), см. вопросы |
| locale/{de,en,es,fr,ja,pt,ru,zh}.json | locale/ | Скопированы побайтно |
| README.md, package.json | эти заметки, Cargo.toml | Прочитаны; дополнительный ./types outlet — публичный модуль types |

Собственного LICENSE и заимствованных лицензированных фрагментов нет. Все 41 исходных
случая сохранены; параметризованных it.each в исходнике нет. Тесты включены как unit modules
из tests/*.rs (autotests=false), чтобы управлять private registration handoff без публичного mock API.
Скелет создал неизменённый scripts/new-crates.sh с одной назначенной строкой: обычный discovery
пропускает status writing:w4. Ни MAP, ни script, ни чужие crates не менялись.

## Отличия от образца

- ApprovalService реализует Plugin с исходным class-row name ApprovalService; ключ Approval
  предоставляет сервис `approval`. ApprovalDeps требует настоящий SessionProjections. Дочерний
  inject ожидает необязательный SystemPromptKey и возобновляется при его замене. Публикация
  только после awaited registration; при позднем завершении после unload новые эффекты не создаются.
- Agent — существующий Arc<dyn hoh_agent::Agent>. request принимает impl Into<Arc<ApprovalRequest>>;
  переданная Arc и agent сохраняют тождество. ApprovalRequest — alias ApprovalRequestEvent.
  Scoped carrier строится общим agent_carrier с точным scope_key агента. Carrier не предоставляет
  JS this, поэтому соответствующие source assertions проверяют membership и subject identity.
- ApprovalRequestEvent дополнительно реэкспортирован в корне для Rust listeners; исходный
  ./types outlet остаётся types. ApprovalRequest и ApprovalRequestEvent — один runtime payload,
  как структурно одинаковые интерфейсы образца; отдельного несовместимого Event TypeId нет.
- ApprovalRequestId реэкспортирован из hoh_brand::ids: такой же бренд объявляет исходный
  cordis-host-runner, второй несовместимый newtype не вводится. Исходный конструктор строки — From.
- Waterfall `approval/request` возвращает сырой serde_json::Value: это недоверенная граница JS
  answerer, необходимая для исходного rogue-return normalization. Нормальный ответ:
  Ok(ApprovalOutcome::AllowedOnce.into()). Публичный request всегда возвращает закрытый
  ApprovalOutcome. Err и синхронная panic callback содержатся через kernel waterfall.
- request начинает turn-check и asked append при вызове; продолжение уже начатого Promise
  работает в owned Tokio task и не отменяется при Drop возвращённого future. В исходнике нет
  фонового ctx.spawn для запроса. После abort kernel waterfall продолжает pending answerer,
  поздний ответ/ошибка отбрасываются, второй decided не появляется. Перед выбором отмены
  ожидается WaterfallStarted: начальные callback prefixes предшествуют decided(cancelled),
  включая непосредственное делегирование через next(). При одновременно готовых signal и
  answer native select отдаёт приоритет отмене; остальной JS microtask runtime не моделируется.
- set_policy/override_of/set_approval_policy возвращают Result с исходной BoxError; регистрация
  читается через retained ProjectionHandle, история approval-пакетом не загружается. Config.policy
  — enum с default Ask, поэтому программный пустой Config — Config::default(). Config сервиса
  хранится в приватной общей Arc<Mutex<Config>>: `service.config().policy = ...` меняет
  конфигурацию самого сервиса, всех его clone и runtime-context provider. Mutex не держится
  через await; session override по-прежнему читается первым и имеет приоритет. Вместо прежнего
  value-поля доступен `config(&self) -> MutexGuard<'_, Config>`: `*service.config() = Config { ... }`
  заменяет конфигурацию целиком в том же shared slot, не перепривязывает только один clone.
- SHIM конфигурации допускает policy:null в native schema и перед deserialization заменяет его
  Ask, как Schema.resolve образца (vendor/schemastery/src/index.ts:533–543). Типизированный
  Config и effective_config содержат только ask/never; третьей policy нет. После исправления
  kernel defaults-on-null убрать shim и nullable schema annotation.
- Невалидная policy-строка отказывается ApprovalPolicy::try_from с исходным TypeError-текстом
  до set_approval_policy; передать такую строку в setter без преобразования нельзя по типам Rust.
  Raw approval/policy event вне закрытого vocabulary возвращает тот же admission error при fold;
  source полагается на TypeScript type и не проверяет вручную собранный невалидный event.
- ApprovalError — собственный обычный Error/TypeError/ZodError, не HarnessError. Source throws
  не наследуют HarnessError. Сохраняются исходные audit field order, optional omission,
  source=user-approval, две policy sentences и switch notices. UUID v4 через getrandom, значения
  случайны как source randomUUID и не сравниваются побайтно.
- stateSchema вручную повторяет z.object({policy:z.enum(['ask','never']).nullable(),turnOpen:z.boolean()}):
  обе пары обязательны, null разрешён только policy, лишние ключи отбрасываются. stateVersion=1,
  wire отсутствует (host-only); unchanged apply возвращает тот же Arc. Сообщения InvalidState
  (pretty JSON issues, name ZodError) написаны вручную по zod 4 в state::parse, не сняты с zod.
- DisplayReason — именованная упорядоченная карта с обязательным en: new(en), insert/get
  заменяют создание и чтение индексных полей исходного объекта; она не попадает в журнал.
  Дополнительные имена Rust: Approval (service key), ApprovalDeps, ApprovalError,
  ApprovalPolicyEvent/ApprovalPolicySource, types::ApprovalAsked/ApprovalDecided и их TYPE,
  DisplayReason, constructors/conversions/as_str/name. Они называют анонимные исходные shapes
  или заменяют языковые механизмы; новых настроек нет.
- В тестах структурный FakeAgent имеет только используемые session/scope/inject; остальные
  Agent capabilities недостижимы для пакета. Session и registry настоящие. vi.spyOn append
  заменён precommit DispatchValidation с SharedError — проверяется исходная identity и отсутствие
  роста журнала. Prototype registration spy заменён per-test Future gate той же initialize-функции.
  Нельзя перехватить inherent Session::event_at/snapshot_events: assertions о warm reads
  представлены повторным чтением той же Arc state, unchanged log и eager checkpoint/release.

Проверены production imports из packages/**/src без client/api/host/experimental. ApprovalPolicy,
APPROVAL_POLICIES и setApprovalPolicy используют headless/permission-presets/subagent; request,
overrideOf, setPolicy и event используют tools, shell/tool-bash/tool-pwsh, ACP и SDK server.
Новых runtime exports сверх перечисленных нет. ApprovalRequestId, ApprovalOutcome и
ApprovalRequestEvent сохраняются как source public types, даже когда импорт type-only.
Прямых named imports ApprovalService, ApprovalRequest, ApprovalRequestEvent, ApprovalOutcome,
ApprovalRequestId и Config в переносимом production-коде не найдено; default service загружается
составом, а события и типы выводятся из declaration merging. Все эти экспорты сохранены.

## Не перенесено

Исходных runtime-файлов и тестовых случаев не исключено. Declaration merging заменён открытыми
event payloads и MessageSource::new("user-approval"); отдельный общий enum чужих событий не введён.
TypeScript runtime mutation/casts и prototype mocks представлены описанными Rust-границами.

## Не проверено

Node/zod для дифференциального исполнения не запускались. Ручные zod issue messages и точное
JS Promise scheduling не проверены относительно Node. Буквальные method-call spies warm-reader
недоступны для inherent Session API; текущий registry внутри чтения берёт cached snapshot cut.
Платформенных веток и файлового/сетевого I/O в пакете нет. Настоящее окно, провайдер модели и
полный профиль этой проверкой не запускаются.

## Вопросы ведущему

- SHIM(hoh-kernel) в src/shim.rs: default_null_policy и nullable annotation Config.policy.
  Текущий kernel validator не применяет .default('ask') к explicit property null, source
  Schemastery применяет. После kernel round 7 заменить на обычное enum-поле с serde default.
  Регрессия проверяет успешный load, canonical effective_config=ask и отказ неверной строки.
- Других недостающих API и правок зависимостей нет. Точный microtask tie-break и method-call spy
  различия указаны выше; данные snapshot, аудита и policy vocabulary сохранены.

## Проверки

Все cargo-команды: CARGO_BUILD_JOBS=2, CARGO_NET_OFFLINE=true,
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w4. Приостановленные регулятором процессы
самостоятельно не возобновляются. Результаты проверок:

- `cargo check -p hoh-user-approval --tests`: exit 0; Finished dev profile, 37m 19s.
  Ошибок компиляции и правок зависимостей не потребовалось. Ненужный test-util feature
  удалён: тесты используют gates/каналы, не зависят от настоящего времени и не создают таймеров.
- `cargo test -p hoh-user-approval`: exit 0; `Finished test profile ... in 16m 36s`.
  `test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s`.
  Все 41 исходных случая + 4 регрессии; doc-tests: 0 passed, 0 failed. Тесты запущены
  один раз после завершения кода и успешного check; повторный прогон не требовался.
- Первый Clippy сообщил только bind_instead_of_map в тесте недопустимой policy. and_then с
  Ok(()) заменён на map без изменения проверок или production-кода; тесты не повторялись.
- `cargo clippy -p hoh-user-approval --all-targets -- -D warnings`: финальный exit 0;
  `Finished dev profile [unoptimized + debuginfo] target(s) in 1.27s`, предупреждений нет.
- `cargo fmt -p hoh-user-approval -- --check`: exit 0, без вывода.
- Все 8 locale-файлов побайтно сравнены с образцом. В production нет todo!(),
  unimplemented!(), unsafe или незавершённых заглушек. Другие crates не редактировались.

## Круг rv-approval.full.md

Ревью прочитано целиком. Результаты 45/0 выше относятся к первой сдаче, не к этому кругу.

- П.2: configuration slot общий для сервиса и prompt provider. Новый config() accessor
  возвращает короткий MutexGuard; присваивание `*service.config() = Config { policy: Never }`
  меняет текущую конфигурацию и для сохранённого closure. Регрессия проверяет Never и возврат
  к Ask через тот же session без override, в request и context_text, а также через исходный
  зарегистрированный экземпляр и его clone.
- П.1: убрано ready.await до abort; порядок asked/handler/decided проверяется общей
  последовательностью после немедленного request(); abort(). Регрессия включает 1 и 8
  делегирующих next() перед terminal answerer; проверка late rejection сохранена.
  request использует новый Context::waterfall_with_start и ожидает только started перед
  выбором cancelled, а не ответ пользователя. Исходные pre-abort и never gates остаются
  перед dispatch. Поздний answer/error остаётся содержимым, второй decided не пишется.
- По явному разрешению ведущего добавлен kernel API waterfall_with_start: started-future
  отдельно от answer-future. Начальный проход включает непосредственные next() до первого
  Pending; пустая цепь, tail и ошибка не оставляют started зависшим. Обычный waterfall,
  его chain, поведение и сигнатуры не менялись; в approval нет копии диспетчера, таймера
  или фиксированного числа yield для синхронизации. Детали и тесты — в разрешённом разделе
  hoh-kernel/PORT_NOTES.md «Добавлено w4 для hoh-user-approval, 19.11».
- `cargo check -p hoh-kernel -p hoh-user-approval --tests`: exit 0, 11.14s.
- `cargo test -p hoh-kernel -p hoh-user-approval`: exit 0, Finished test profile in 31m 47s.
  kernel — 294 passed, 0 failed, 1 прежний ignored; approval —
  `test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s`.
  Новые kernel startup cases: 6/0. В approval сохранены все 41 исходных случая,
  исходная отмена теперь проверяется без предварительного ready.await, включая 0/1/8 делегатов.
  Добавлен один тест согласованности текущей конфигурации, всего 5 дополнительных регрессий.
  Полный тестовый прогон этого круга выполнен один раз; doc-tests обоих пакетов 0/0.
- `cargo clippy -p hoh-kernel -p hoh-user-approval --all-targets -- -D warnings`: exit 0,
  Finished dev profile in 22m 58s; предупреждений нет, дополнительных правок не потребовалось.
- `cargo fmt -p hoh-kernel -p hoh-user-approval -- --check`: exit 0, без вывода.
- Оба замечания ревью закрыты и проверены. Из зависимостей изменено только явно разрешённое
  добавление kernel startup API; старый waterfall сохранён побайтно. SDK server не начинался.

## Режим host (склейка окна, часть 4, sg4, 19.11)

- Не часть TS-пакета: добавление HoH для окна. `src/host.rs` + `ApprovalService::install_host_mode(key_hex, peer)`.
  Один раз на сервис (повтор → `HostAlreadyConfigured`); без вызова поведение прежнее (цепочка answerer'ов).
- В режиме host запрос обходит цепочку answerer'ов: `approval.pending` → обратный запрос
  `approval/request {approvalId,risk,humanText,actionHash,reason,expiresAt,…}` через `HostApprovalPeer` →
  `hoh_protocol::approval::verify_decision` (HMAC-SHA256, срок, совпадение id) → `approval.resolved`.
  Любая ошибка/неверный mac/ответ после срока → `unavailable` (fail-closed); 5 минут → `cancelled`
  (сигнал уходит транспорту); abort хода → `cancelled`. Новый `approvalId` на каждый запрос, поэтому
  повтор прежнего ответа новое подтверждение не авторизует. `humanText` строится из `displayReason.en`
  или из имени инструмента; текст модели (`reason`) в него не попадает.
- GLUE-SHIM(2b): трейт `HostApprovalPeer` — сервер JSON-RPC реализует его и вызывает `install_host_mode`
  из успешного `initialize` с `capabilities.approvals:"host"`.
- Привилегированные методы (`PRIVILEGED_METHODS`): `ApprovalService::approve_privileged(method, params)` →
  тот же путь `approval/request` с mac (`sessionId` из params, иначе `host`; `toolName` = метод; `action` =
  `{method, params}`; для `hoh/model/key/set` — `{keyHash, baseUrl}`, ключ не уходит). Текст (`src/method.rs`,
  `method_human_text`) перенесён из `hoh-window-stub::engine::approval_text` и строится из типизированных
  параметров; невалидные параметры/текст → отказ без запроса. Не `allowed-once` → ошибка.
  Монтаж `HohPrivilegedApprover` невозможен внутри этого крейта: трейт и ключ сервиса живут в
  `hoh-sdk-jsonrpc-server`, который зависит от `hoh-user-approval` (цикл). Стык — тонкий адаптер
  `impl PrivilegedApprover` над `approve_privileged` в `apps/hoh` или в сервере (см. отчёт).
- Тесты `tests/host.rs`: 8/0 (всего крейт 55/0).

## Круг исправлений по ревью (sg4-fix1, 19.11)
- Host-режим переживает пересборку сервиса: слот режима лежит в сервисе `approvalHostSlot`, который
  провайдится из корня ядра (`ctx.root()`), а не в экземпляре `ApprovalService`. Публичный API
  (`install_host_mode`, `approve_privileged`, `host_mode_enabled`) не менялся; повторная установка
  по-прежнему ошибка и на новом экземпляре. Слот живёт до остановки ядра.
- Тесты усилены как в образце: ровно один вклад `approval:policy` до и после перезапуска промпта
  (`context_text` теперь падает на дубле), `OutsideTurn` проверяется по типу и тексту, у `approval/policy`
  проверяется тип события.
