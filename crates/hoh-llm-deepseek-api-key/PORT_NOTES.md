# hoh-llm-deepseek-api-key ← packages/llm/llm-deepseek-api-key @ 39163dd3

## Файлы

| образец | перенос | состояние |
| --- | --- | --- |
| src/config.ts | src/config.rs | Полная конфигурация route, defaults, plain snapshot, protocol/listing/credential resolution |
| src/index.ts | src/provider.rs | Регистрация каталога и adapter route, credential-gated discovery, listing cache, admission, volatile switching |
| зависимости от llm-deepseek | src/listing.rs, src/shim.rs | Временный приватный shim до появления hoh-llm-deepseek |
| tests/config.spec.ts | tests/config.rs | Исходные проверки конфигурации, точного listing timing, порядка ошибок и schema metadata |
| tests/discovery.spec.ts | tests/provider.rs | Все исходные сценарии, включая отсутствующий и пустой ключ с default/custom catalog |
| tests/listing.spec.ts | src/listing.rs tests, src/provider.rs tests | Все 12 сценариев: порядок/header/diagnostics, badges, отсутствие запросов, fallback, refusals, смена ключа, abort, reload и idle refresh |
| locale/*.json | locale/*.json | Все 8 файлов побайтно |
| README.md, README.i18n.yaml, package.json | PORT_NOTES.md, Cargo.toml | Прочитаны; отдельной LICENSE нет |

## Поведение и адаптации

- `Config` использует `hoh_kernel::Volatile`, сохраняет wire-имена (включая точное `baseURL`), defaults и роль
  `credential-ref`. `resolve_adapter_options` заново проверяет динамические значения, доверенный
  `DEEPSEEK_BASE_URL`, HTTP(S)-origin, built-in catalog и оба listing timer. Как в TS, listing timing
  проверяется раньше protocol options, поэтому при нескольких ошибках выбирается то же сообщение.
- Credential service авторитетен даже при отсутствии значения; fallback к launch environment бывает
  только без сервиса. Значение проверяется через `hoh_llm::assert_usable_api_key`, секрет не попадает
  в диагностику или cache scope (scope содержит SHA-256).
- Listing отправляет `x-api-key`, запрещает redirects, ограничен timeout/caller abort, сохраняет
  last-known-good каталог и одну warning на серию отказов. Lifecycle-owned задача обновляет listing
  в простое и публикует `llm/models-updated`; `maxAgeMs=0` отключает автоматический refresh. Первый
  caller может вернуть собственную abort reason, не отменяя shared single-flight; stale view не ждёт refresh.
  Фоновый driver только ставит first-load в свою очередь и не ждёт сам себя, поэтому смена credential
  перед idle refresh не создаёт взаимную блокировку.
- Изменение volatile `retryPolicy` вызывает атомарный `registration.replace`, поэтому следующий prepared
  call получает policy той же конфигурационной генерации. Начало/прекращение listing invalidates cache;
  отвергнутая конфигурация сохраняет прежнее состояние.
- Diagnostics идут через kernel logger вместо `console.error/console.warn`. `settingsNs` равен имени
  плагина: у текущего Context API нет публичного эквивалента `ctx.fiber.entry?.options.id`.
- `resolve_model` всегда описывает модель; refusal применяется только при admission в `prepare_call`
  и непосредственном `stream`, как в исходном `DeepSeekAdapter`. Ошибка credential при смене ключа
  не использует refusal от старого listing.
- Prepared dispatch захватывает тот же connection, на котором прошёл admission, и не перечитывает
  новую конфигурацию перед transport dispatch; прямой `stream` по-прежнему проверяет текущий listing.
- Listing shim повторяет JS для safe integer capacities, нетипизированных update modes, optional/null
  badge, проверки непрочитанных символов до trim и классификации redirect как `TRANSPORT`.
- Rust enum/newtype заменяют строковые TypeScript unions; поля и строки ошибок сохранены. Typed integer
  fields исключают часть невозможных для Rust programmatic значений до resolver boundary.

## Не перенесено

- Собственный исходный runtime-код пакета перенесён. Полноценная отправка Messages-запроса относится к
  ещё отсутствующему `hoh-llm-deepseek`; временный adapter возвращает `TRANSPORT` только при фактическом
  stream dispatch. Discovery, exact-model resolution, admission и prepared-call выполняются.
- Hooks `image_request_pricing` и `image_request_target` остаются на defaults `hoh-llm` до появления
  `hoh-llm-deepseek`; в частности, временный shim пока не воспроизводит исходный `UNKNOWN_MODEL` для
  image target. Это часть той же transport-зависимости, а не локальная имитация.
- `settings.configure({auto:false})` из общего TS `registerDeepSeekProvider` не перенесён: в текущем
  workspace нет Settings service/API, который можно вызвать из этого крейта.
- `reqwest::Client` пока создаётся на каждую загрузку listing. Это не меняет наблюдаемый HTTP-контракт,
  но повторно создаёт transport state; общий client должен прийти из будущего `hoh-llm-deepseek` shim.
- Проверка длины badge в shim считает Unicode scalar, а зависимость использует `Intl.Segmenter` graphemes.
  В `Cargo.lock` нет подходящего Unicode-segmentation crate, а добавлять новый внешний crate запрещено.

## Вопросы ведущему

- `SHIM(hoh-llm-deepseek)`: заменить credential helpers, listing/parser/cache/catalog/model-info и
  adapter transport на API принятого `hoh-llm-deepseek`. После этого убрать `semver`, `sha2`, `reqwest`
  и временные модули, если они станут транзитивной ответственностью зависимости.
- До интеграции с `hoh-llm-deepseek` живая отправка Messages намеренно не имитируется несовместимым
  OpenAI Chat Completions transport.

## Проверки

Команды выполняются с `CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-x1`,
`CARGO_BUILD_JOBS=2`, `CARGO_NET_OFFLINE=true`; тесты — только под `timeout`.

- `timeout 240s cargo test -p hoh-llm-deepseek-api-key --offline`: 44 passed, 0 failed
  (unit 27, config 12, provider 5, doc-tests 0).
- `timeout 240s cargo clippy -p hoh-llm-deepseek-api-key --all-targets --no-deps --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-llm-deepseek-api-key -- --check`: exit 0.
- Независимое read-only ревью после исправлений: High/Medium замечаний нет; найденный prepared-generation
  defect и последний Low о регрессионной проверке direct-stream admission исправлены до финального gate.
