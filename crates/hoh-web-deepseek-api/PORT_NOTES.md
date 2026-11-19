# hoh-web-deepseek-api ← packages/web/web-deepseek-api

## Файлы и поведение

- `src/lib.rs` переносит конфигурацию, plugin-регистрацию двух провайдеров,
  выбор billing/route и live-снимок настроек на каждую операцию.
- `src/transport.rs` переносит порядок credential sources, readiness, отмену,
  redirect-free POST, HTTP/JSON диагностику, account-token rejection и endpoint guidance.
- `src/search.rs` переносит request/result mapping, `ori_question` (Unicode scalar,
  не UTF-16 half-surrogate), локальный sliding-window 1s rate limit и все 8 описаний.
- `src/fetch.rs` переносит Fetch API request и Markdown → `WebFetchBody::Text`.
- README, package manifest, оба README-языка и все восемь locale-файлов прочитаны;
  пользовательские строки provider picker сохранены в `LocalizedText`.

Transport использует общий `reqwest 0.13.5`. Один клиент строится при `apply` из
замороженного `LaunchEnvironment` через policy/options `hoh-http-proxy`, поэтому
launch-снимок proxy и штатные connect/read timeouts применяются ко всем операциям,
а пул соединений переиспользуется. Builder вызывается отдельно от
`ProxyInstallation::client`, потому что web API обязан запрещать redirects; proxy
policy и dispatcher options при этом те же. Офлайн-тест доказывает инъекцию клиента,
а ignored loopback-тест проверяет реальный proxy request-target.

Новых внешних зависимостей нет. Внешний API в тестах не вызывается. Wire-проверки
используют одноразовый loopback TCP server; в writer-песочнице bind запрещён,
поэтому четыре сетевых теста явно помечены `ignored`, а не засчитываются как
успешные. Transport state machine, точные приоритеты и sensitive-флаги заголовков
проверяются отдельно без сокета.

## Отличия и границы

- `DeepSeekWebApiKey` и auth callbacks выражены `Arc<dyn Fn -> BoxFuture>`;
  это объектно-безопасный эквивалент Promise-callbacks TS. Credential не хранится
  в provider: reference перечитывается на каждую операцию.
- Rust `u64` не представляет программно переданные дробные `count`/rate значения;
  JSON schema и typed API не допускают их. Ноль и верхняя граница count проверены.
- Причина ошибки сохраняется через `HarnessError::with_cause`; строковое JS throw
  в типизированном Rust callback не представимо.
- Отмена ожидания credential resolver немедленно возвращается вызывающему коду,
  но сама неотменяемая операция остаётся наблюдаемой в `tokio::spawn` и может
  безопасно завершить запись/refresh, как Promise в TS.
- Account-token `reject` после HTTP 401, напротив, намеренно ожидается даже после
  отмены чтения body: TS также завершает invalidation до возврата исходного 401
  или `WEB_ABORTED`.
- JSON success/error bodies принимают UTF-8 BOM. Невалидные credential references
  и URL origin теперь возвращают ошибку конфигурации вместо молчаливой подстановки.
- Reqwest сообщает network/redirect failure своим Rust Display-текстом, поэтому
  текст причины не дословно равен браузерному `TypeError: fetch failed`; код
  `WEB_PROVIDER_ERROR`, запрет redirect и endpoint guidance совпадают.
- Реальные Search/Fetch e2e из `web-deepseek-api.e2e.ts` не запускаются: им нужны
  `DEEPSEEK_API_KEY` и внешний endpoint, что запрещено условиями волны.
- Полная 12-status wire-матрица и abort во время response-body/reject оставлены
  для разрешённого loopback-контура и явно перечислены в `SOURCE_TEST_CASES.md`;
  они не засчитываются в локальные успешные тесты.
- Review 235043: HTTP diagnostic теперь точно повторяет TS ternary — `error`
  строкой имеет приоритет, object читает только собственный string `message`, а
  `null`/число/массив/bool откатываются к верхнеуровневому `message`. Регрессия
  закреплена полной shape-матрицей. Безсетевой injected-client тест дополнительно
  проверяет `reqwest::Error::is_builder()`: `https_only` отвергает HTTP URL до
  DNS, поэтому тест действительно доказывает использование переданного клиента.
  Дополнительные readiness-регрессии фиксируют переход от пустого referenced key
  к следующему source, остановку после ошибки чтения account token и передачу
  model route/диагностики из ready target resolver без чтения operation options.

## Вопросы ведущему

- `SHIM(hoh-llm-deepseek-account)` в `src/shim.rs`: пока owning-крейта нет
  (`MAP.tsv: later`), protocol subset account-route разбирается через публичный
  `hoh-llm-deepseek::Options` и его resolver. После появления крейта заменить на
  `plain_account_options`/`resolve_account_options`; поведение endpoint и ошибок
  зафиксировано тестами. Чужие крейты не менялись.
- Account service уже предоставлен точным временным seam
  `hoh_llm_deepseek::DeepSeekAccount`; его замена owning-крейтом также остаётся
  интеграционной работой ведущего.

## Проверки

Команды выполнены offline, jobs=2, target `/mnt/toshiba/alexandrie/hoh/target-x8`;
тесты всегда под внешним `timeout`.

- `timeout 180s cargo test -p hoh-web-deepseek-api --offline --no-fail-fast`: 55 passed, 0 failed,
  4 ignored (loopback bind запрещён песочницей); doc-tests 0/0.
- `cargo clippy -p hoh-web-deepseek-api --all-targets --no-deps
  --offline -- -D warnings`: exit 0.
- `cargo fmt -p hoh-web-deepseek-api -- --check`: exit 0.
