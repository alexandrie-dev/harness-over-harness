# hoh-llm-deepseek ← packages/llm/llm-deepseek

## Файлы

Все 28 файлов `src/*.ts` перенесены в одноимённые Rust-модули: adapter/config,
credential/defaults, Files API/store/index, изображения и pricing, listing/cache/catalog,
Messages serialization/SSE/translation/replay/transport, host и wire types. `src/index.ts`
соответствует публичным реэкспортам `src/lib.rs`.

## Реализованное поведение

- Messages transport запрещает redirects, формирует точные attribution/session/compaction/beta
  headers, читает SSE с idle deadline, классифицирует HTTP и in-band ошибки, сохраняет request id
  и оба вида `Retry-After`.
- Подготовка изображений соблюдает raw/base64/count budgets, устойчиво проецирует logged offload,
  использует точный опубликованный V4.1 token grid и либо выдаёт Files ids, либо целиком откатывает
  запрос на bounded inline base64.
- Files store имеет durable cross-process index через `hoh-atomic-write`, singleflight upload с
  независимой отменой waiters, backoff 1/2/4/8, exact-generation invalidation, один quota cleanup
  retry, полную пагинацию owned-файлов и `release`/`release_all`.
- Listing cache разделён по endpoint+SHA-256 credential, делает first-load singleflight, позволяет
  caller отменить только ожидание, обновляет stale listing в фоне, удерживает last-known-good,
  автоматически обновляется, отменяется при rescope/invalidate/dispose и подавляет поздние результаты.
- `resolve_adapter_options_with_environment` воспроизводит приоритет explicit `baseURL` над
  доверенным `DEEPSEEK_BASE_URL`; исходный `resolve_adapter_options` остаётся удобной границей без environment.

## Отличия и SHIM

1. `DeepSeekAdapter` принимает operation-local callbacks исходника: live `options`, async `connect`,
   provider-owned `discoverModels`/`admit`, request-local `resolveAuth` с безопасным `onRequestError`,
   live anonymous-user/attachment resolvers, а также replay/extension reporters. `prepare_call` замораживает один connection snapshot для model
   info, endpoint, defaults, Files и auth. Текущий `LlmAdapter` workspace не позволяет вернуть coded
   error из sync `image_request_target`, поэтому для отсутствующей model declaration остаётся identity
   projection вместо исходного `UNKNOWN_MODEL`; это ограничение чужого trait API.
2. `hoh-deepseek-account` ещё отсутствует. Его service boundary временно представлен публичным
   `DeepSeekAccount`/`DeepSeekAccountApi` с точным именем `deepseekAccount`; helpers разрешают токен
   только через сервис и содержат ошибку удаления отвергнутого токена. Это interface-first SHIM,
   который надо заменить типом будущего owning-крейта без изменения поведения callers.
3. `Intl.Segmenter` отсутствует в разрешённых зависимостях. Badge parser локально покрывает combining
   marks, variation selectors, emoji modifiers и ZWJ families, а также все safety rejects исходника.
4. Типизированные Rust integer/enum boundaries не могут представить часть JS programmatic значений
   (дробные integer fields, arbitrary non-string unions); доступные значения проверяются теми же bounds.
5. Files transport инъецируется внутренней trait-границей в unit-тестах вместо source `fetch` seam;
   production использует один redirect-free `reqwest::Client` на объект клиента.
6. Удалённое поле `imageDetail` сохраняется только как скрытый presence-флаг модели, включая JSON `null`.
   Это SHIM для точного `Object.hasOwn`-отказа исходника: значение никогда не сериализуется и всегда отвергается
   `resolve_adapter_options` с указанием `imagePixelBudget`.

## Тесты и остаток

Сейчас 87 Rust-тестов (unit + integration; табличные проверки разворачивают дополнительные значения).
Покрыты конфигурация/defaults/environment и все числовые границы, image grid/dimensions/pricing/budgets,
listing decode и catalog gating, побайтовый SSE и полная матрица translation/provider errors,
serialization system/tool/replay cases, Files object/limit/quota validation, singleflight/cancellation/
backoff/quota/pagination, durable index races/corruption/exact-generation removal и listing-cache lifecycle.

Дополнительная сверка с `tests/listing-cache.spec.ts` нашла и закрыла два дефекта: автоматический цикл
теперь продолжает credential discovery после sign-out и ошибок хранилища, а `dispose()` отменяет дальний
таймер и ждёт уже допущенный credential lookup. Регрессионные тесты падают на прежней реализации.

Внешний review-run выявил scheduling-зависимость тестового барьера Files singleflight: 1000 быстрых
`yield_now()` могли закончиться раньше фонового чтения durable index. Как и source `vi.waitFor`, mock
теперь ждёт явное событие начала upload с bounded timeout; сохранённый `Notify` permit закрывает окно
между проверкой счётчика и регистрацией ожидания. Production lifecycle не изменён.

Adapter lifecycle дополнительно покрыт локальными регрессиями: discovery omission, volatile retry policy,
async connection с signal, provider admission до I/O, prepared-call snapshot, request-local auth и
`onRequestError` с сохранением исходной ошибки при отказе mapper. Host-composition сценарии dynamic
settings/provider re-registration и Loader YAML/restart принадлежат принятому
`hoh-llm-deepseek-api-key`: Rust-порт разделил общий protocol adapter и route provider на два крейта.
Полные HTTP wire/e2e проверки Messages и Files требуют
разрешённого loopback test server: sandbox этого писателя возвращает `PermissionDenied` на
`bind(127.0.0.1:0)`; protocol/transport seams проверены без сети. Account helpers и destination/rejection
контракт покрыты через SHIM, а полный signed-in lifecycle останется за owning account-крейтом.

Повторное ревью закрыло ещё восемь расхождений с TypeScript:

- чтение успешного Files body теперь участвует в том же abort race после получения заголовков; ошибка body
  остаётся `TRANSPORT` с исходной `reqwest::Error` в cause, а только JSON syntax становится `INVALID_RESPONSE`;
- оба пути Messages payload (base и API extensions) используют `js_json_stringify`, поэтому `1.0` уходит как `1`;
- stale-id classifier поддерживает исходные `invalid.{0,20}file id/api` в обоих порядках и действительно удаляет
  exact mapping перед единственным retry;
- legacy `imageDetail` отвергается после десериализации, включая `null`;
- catalog semver игнорирует build metadata и отвергает numeric prerelease с ведущим нулём;
- badge trim использует полный JavaScript whitespace set, включая BOM;
- завершённые listing load/timer tasks удаляются из pending set и не накапливаются между refresh;
- delta-seconds `Retry-After` принимает только исходную decimal grammar, без exponent, знака и пробелов.

Регрессии body boundary построены на настоящих streaming `reqwest::Response` (`pending` и transport-error body),
поэтому доказывают `Response::text()`/abort поведение без запрещённого в sandbox loopback bind. Остальные пункты
проверены на публичном config/catalog/request API либо на owning module seam; каждый тест падает на старом коде.

Следующий review-круг уточнил stale file-id grammar. Классификатор теперь воспроизводит исходный
`\bfile(?:[_ -]?...)?`: `profile` и `my_file` не считаются упоминанием Files API, а формы без разделителя
`fileid`, `fileapi` и `notfound` принимаются. Табличный regression-тест фиксирует обе стороны границы;
прежняя реализация ошибалась на четырёх из пяти строк.

Отложенный review-круг уточнил normalized-image rejection grammar. Классификатор теперь сохраняет два
направленных набора причин из исходных regex, предел `.{0,40}` по Unicode-символам и запрет перехода через
`\n`, `\r`, U+2028 или U+2029. Табличный regression фиксирует совпадающие и несовпадающие порядок,
дистанцию и line-boundary случаи; прежняя реализация ошибочно принимала шесть отрицательных строк.

Известные допустимые отличия от JavaScript:

- HTTP-date ветка `Retry-After` использует `jiff` RFC 2822 parser. Исходный `Date.parse` дополнительно
  принимает ISO, asctime и RFC 850; стандартный серверный IMF-fixdate входит в поддержанную Rust-грамматику.
- Ошибка чтения response body в TypeScript пробрасывается напрямую, а Rust представляет её как
  `LlmError(TRANSPORT)` с исходным `reqwest::Error` в cause. Класс transport failure и причина сохраняются.

## Вопросы ведущему

- `SHIM(hoh-deepseek-account)`: заменить `DeepSeekAccount`/`DeepSeekAccountApi` импортом owning-крейта,
  когда пакет выйдет из MAP milestone `later`; service name и helper semantics уже зафиксированы тестами.
- После принятия этого крейта `hoh-llm-deepseek-api-key` должен убрать свой приватный transport/listing
  SHIM и подключить публичные модули и новые operation-local callbacks; чужой крейт в этой работе не
  менялся.

## Текущие проверки

- `timeout 300s cargo test -p hoh-llm-deepseek --offline`: 87 passed, 0 failed
  (unit 33, integration 54, doc-tests 0).
- `timeout 240s cargo clippy -p hoh-llm-deepseek --all-targets --no-deps --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-llm-deepseek -- --check`: exit 0.
- `timeout 180s cargo check --workspace --all-targets --offline`: exit 0; в чужом
  `hoh-web-search-yandex` остался warning об устаревшем quick-xml API, этот крейт не изменялся.
