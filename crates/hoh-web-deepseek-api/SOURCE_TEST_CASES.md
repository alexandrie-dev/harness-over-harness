# Матрица исходных тестов

Источник содержит 129 объявлений `it`/`it.each`. Ниже указано фактическое покрытие,
а не соответствие один-к-одному: параметризованные ветви объединены в Rust-матрицы.
Два live e2e требуют внешнего ключа и намеренно не запускаются.

| TypeScript suite | Rust coverage |
|---|---|
| `transport.spec.ts` | `transport::tests::*`: source order, readiness errors/endpoints, missing/misconfigured sources, exact resolver error/cause, cancellation before/during resolution, detached completion after abort, header priority/sensitivity, exact guidance, error-detail shapes and BOM. Loopback wire/401/redirect tests are explicitly ignored because sandbox bind is forbidden. |
| `search.spec.ts` | `search::tests::*`: response/malformed matrices, optional fields, Unicode question bound, availability/count/rate/readiness, request body with `min`/`ori_question`, 1001ms pacing and cancellation, pre-dispatch abort. Session extraction remains integration-covered through plugin wiring rather than a synthetic Agent session. |
| `fetch.spec.ts` | `fetch::tests::*` plus transport boundary: availability, readiness, metadata, Markdown/malformed decode and pre-dispatch abort. The full 12-status wire matrix is deferred; the shared HTTP error-detail branches are unit-tested and the account-401 wire case is ignored. |
| `credential-resolution.spec.ts` | `lib::tests::*`, `transport::tests::*`, `shim`: all billing source orders and five diagnostics, official/account defaults and misconfiguration, route first-match semantics, empty values and readiness. |
| `credentials.spec.ts` | referenced-key callback is invoked per operation; empty launch value and independent configured/resolve callbacks are tested. |
| `composition.spec.ts` | plugin registration/disposal and configured-provider failure are tested. Current initiator/session and live `Cfg` lookup use the same production closures but are not reconstructed with test-only Agent internals. |
| `redirect.spec.ts` | redirect-none client policy; `transport::tests::redirect_response_is_rejected_without_following` is explicitly ignored because sandbox loopback bind is forbidden |
| `egress.spec.ts` | launch-environment proxy snapshot is used to build the shared client; offline client-injection test is active, loopback proxy wire assertion is ignored |
| `web-deepseek-api.e2e.ts` | deferred: live API key/endpoint required; local protocol and mapping covered |

Компактные Rust-тесты объединяют табличные TS cases, но каждая ветвь runtime
реализована непосредственно по `src/{index,transport,search,fetch}.ts`.

Явно отложены до разрешённого loopback/e2e контура: полная HTTP status matrix,
abort во время чтения response body и account-token `reject`, а также два live
Search/Fetch e2e. Они не входят в число успешно прошедших тестов этого крейта.
