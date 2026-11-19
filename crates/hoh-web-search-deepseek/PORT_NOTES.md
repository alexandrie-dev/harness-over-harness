# hoh-web-search-deepseek ← packages/web/web-search-deepseek @ 39163dd3

## Файлы

| образец | перенос | состояние |
|---|---|---|
| `src/index.ts` | `src/lib.rs` | plugin `WebSearchDeepSeek`, точные config keys/defaults, live `Cfg`, launch-environment fallback, выбор account route, readiness и session event |
| `src/provider.ts` | `src/provider.rs` | provider metadata, credential order, cancellation, secret-free request record, Anthropic Messages POST, redirect policy, diagnostics и result mapping |
| `src/types.ts` | `src/types.rs` | все семь экспортированных wire types; открытый union сохранён как `serde_json::Value` |
| `tests/deepseek.spec.ts` | `tests/deepseek.rs`, unit tests в `src/lib.rs` и `src/provider.rs` | сгруппированные матрицы по mapping/auth/cancellation/snapshot/config; integration-замены перечислены в «Не проверено» |
| `tests/settings.spec.ts` | `tests/settings.rs` | defaults/validation, registration metadata и lifecycle disposal |
| `tests/redirect.spec.ts` | `tests/redirect.rs` | все пять provider redirect status и контрольное default-follow поведение; оба явно ignored в writer sandbox |
| `tests/egress.spec.ts` | `tests/egress.rs` | plugin получает proxy из `LaunchEnvironment` snapshot; настоящий loopback proxy проверяет absolute URI и credential header; ignored в writer sandbox |
| `tests/deepseek.e2e.ts` | — | live API probe исключён правилом PORTING §10 |
| `locale/{de,en,es,fr,ja,pt,ru,zh}.json` | те же пути | скопированы без изменений; `diff -u` чист |
| `README.md`, `README.zh.md`, `README.i18n.yaml`, `package.json`, `tsconfig.json` | rustdoc, Cargo manifest и эти заметки | прочитаны; npm/build/i18n metadata не создаёт runtime API крейта |

Новых внешних зависимостей нет. HTTP использует уже закреплённый workspace
`reqwest 0.13.5`, JSON — `serde_json`, порядок citation/result сохраняет
`indexmap`. Plugin при `apply` строит один redirect-free pooled client из
замороженного `LaunchEnvironment` через policy/options `hoh-http-proxy`, включая
штатные connect/read timeouts. Standalone `DeepSeekSearchProvider::new` хранит
один direct `no_proxy` client: в source proxy появляется только после явной
установки, native fetch сам process env не читает. Каждый поиск получает один
снимок настроек до первого `await`; ключ разрешается заново на каждую операцию. Запись
`web/deepseek-search-llm-request` происходит до создания HTTP-запроса и не
содержит credential. Построенный `reqwest::Request` отдельно закреплён тестом:
POST, все шесть заголовков и JSON body совпадают с записанной структурой.

Прямых импортов экспортов пакета из переносимых production-пакетов не найдено:
bundle подключает plugin по manifest row. Поэтому сохраняемые для внешнего API
`DeepSeekSearchProvider`, constants, options/request types, wire types,
`citation_snippets`, `map_anthropic_response` и settings namespace сейчас не
имеют Rust-потребителя за пределами тестов.

## Отличия от образца

- JavaScript object/callback types выражены именованными Rust-структурами и
  `Arc<dyn Fn -> BoxFuture>` aliases. Вложенные части публичного
  `DeepSeekSearchLlmRequest` получили публичные имена
  `DeepSeekSearchRequestBody`, `DeepSeekSearchMessage`, `DeepSeekSearchText`,
  `DeepSeekSearchTool`, `DeepSeekThinking`; это требование публичной типизации
  Rust, новых wire-полей нет.
- `ContentBlock` хранится raw JSON: source union открыт вариантом
  `{ type: string }`, а ответ приходит от внешнего endpoint. Известные wire
  structs также экспортированы отдельно, как в `types.ts`.
- Положительные целые config limits представлены `u64`; custom-deserializer
  принимает целые JSON-формы `4096`, `4096.0` и `1e3`, но отвергает ноль,
  отрицательные и дробные значения. Программно передать `NaN`/бесконечность и
  целое вне диапазона `u64`, возможные у нетипизированного JS caller, нельзя.
- Native `fetch(..., redirect: 'error')` заменён reqwest client с
  `Policy::none()`: Location не посещается, но 3xx классифицируется как HTTP
  provider error, а не transport exception. Проверяемые code, endpoint guidance
  и security boundary совпадают; текст первой строки для redirect отличается.
- Ошибки Rust callbacks имеют форму `Error: <Display>`; произвольный JS throw,
  не являющийся Error, в типизированном Rust callback не представим. Cause
  сохраняется через `HarnessError::with_cause`.
- Credential future под сигналом исполняется в `hoh_agent::spawn_inherited`: caller получает
  `WEB_ABORTED` сразу, а неотменяемая операция остаётся наблюдаемой и может
  завершить refresh/чтение, как уже созданный Promise source, сохраняя task-local
  initiating Agent. Account-token
  invalidation после 401 намеренно не detach-ится и ожидается до возврата.
- JSON success/error bodies принимают UTF-8 BOM, как `Response.json()`; значения
  `x-dsh-auth-token`, `x-api-key` и `authorization` помечены sensitive.
- `Config` имеет ручной `Debug`: literal `apiKey` выводится только как
  `<redacted>`. Explicit JSON `null` для `apiKey`/`baseURL` отвергается, как
  `z.string()` source; отсутствующее поле по-прежнему означает `undefined`.
- Некорректные внешние response shapes повторяют наблюдаемое JS-поведение:
  строковый `content` и `content:null` дают пустой список, неитерируемый object и
  `null` block/result/citation дают неименованный `TypeError`; `search()` добавляет
  префикс `DeepSeek returned an unprocessable response body` и endpoint guidance.
- Rust plugin type/`Plugin::NAME` заменяют JS namespace `name/inject/apply` и
  Loader `unwrapExports`; отдельного default-export понятия у крейта нет.

## Не перенесено

- Единственный `it.skip` из `deepseek.e2e.ts`: требует настоящий
  `DEEPSEEK_API_KEY`, внешний endpoint и сеть, прямо исключённые PORTING §10 и
  условиями волны.
- Npm artifact/default-export assertions заменены compile-time namespace/plugin
  формой Rust; npm loader в workspace Rust отсутствует.

## Не проверено

- Четыре полноценных loopback integration tests в `redirect.rs` и `egress.rs`
  компилируются, но помечены `ignored`: writer sandbox запрещает bind. Они не
  засчитываются как успешные и должны быть запущены conformance-средой с
  разрешённым loopback. Egress fixtures передают proxy только через отдельный
  `LaunchEnvironment` snapshot и не читают/меняют process env; они проверяют
  полный POST/body/header contract, успешный ответ и account-token 401.
- Настоящий DeepSeek endpoint, реальные proxy credentials и TLS не проверялись.
- Файловый credential store не поднимается в этом крейте; тот же production
  `Credentials` service seam проверяет missing/unreadable store и ротацию ключа
  между поисками. Порядок account/API-key, настоящий
  `AgentRegistry::with_initiator`, отсутствие initiator, session event и
  readiness проверены production closures. HTTP response boundary проверяется
  настоящими `reqwest::Response`, включая success, 400/401/429/500/503,
  malformed body и cancellation; только фактический socket остаётся частью
  разрешённого loopback-контура.

## Вопросы ведущему

- `hoh-llm-deepseek` сейчас публикует временный `DeepSeekAccount` service seam,
  которым этот plugin пользуется без копирования. После появления отдельного
  owning-crate `hoh-llm-deepseek-account` ведущему нужно подтвердить, остаётся ли
  service key/API прежним; чужой крейт здесь не менялся.
- `Session::append` принимает строковое имя события, поэтому source module
  augmentation для `web/deepseek-search-llm-request` не имеет отдельного
  registry API в текущем `hoh-session`. Exact payload закреплён публичной
  структурой и тестом; если появится typed event registry, событие следует туда
  зарегистрировать без изменения wire.

## Проверки

Все cargo-команды выполнены offline, jobs=2, target
`/mnt/toshiba/alexandrie/hoh/target-x8`; tests только под внешним `timeout`.

- `timeout 180s cargo test -p hoh-web-search-deepseek --offline
  --no-fail-fast`: 50 passed, 0 failed, 4 ignored (loopback bind); doc-tests 0/0.
- `timeout 180s cargo clippy -p hoh-web-search-deepseek --all-targets
  --no-deps --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-web-search-deepseek -- --check`: exit 0.

Все 79 исходных `*.spec.ts` объявлений прочитаны. Активное покрытие сгруппировано
в 50 Rust-тестов и не заявляется как арифметическое соответствие 79 отдельным тестам;
ограничения composition/wire перечислены выше. Четыре loopback cases
скомпилированы, но ignored. Отдельный live `it.skip` не входит в 79 и исключён
PORTING §10.
