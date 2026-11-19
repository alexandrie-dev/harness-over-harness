# Матрица исходных тестов

В четырёх `*.spec.ts` — 79 объявлений `it`/`it.each`: `deepseek.spec.ts` 74,
`settings.spec.ts` 2, `redirect.spec.ts` 2, `egress.spec.ts` 1. Таблица описывает
фактическое сгруппированное покрытие, а не 79 тестов один-к-одному; отдельный live
`it.skip` из `deepseek.e2e.ts` исключён по PORTING §10.

| исходный suite | Rust coverage |
|---|---|
| citation/result mapping | `citation_snippets_maps_*`, `maps_structured_*`, `dedupes_*`, `accepts_empty_*` |
| availability и metadata | `availability_requires_*`, `provider_metadata_*` |
| request body/record ordering | `records_exact_*`, `built_request_*`, `invalid_header_*`, `one_search_uses_*`; exact method/headers/JSON |
| account/API-key order | `account_token_precedes_*`, `absent_account_*`, `resolver_failure_*`, private header sensitivity matrix |
| HTTP 401/403 state machine | active `reqwest::Response` boundary plus pure matrix: exact sign-in guidance, exact sent token, reject failure ignored, API-key 401/account 403 do not reject, aborted 401 body still rejects once |
| cancellation | `pre_aborted_*`, `cancellation_interrupts_*`, detached resolver completion after caller abort, pending send/success/error body |
| config/readiness/environment | unit tests `src/lib.rs`, null/integer/default checks, missing/unreadable/rotated credential service, invalid credential ref and invalid env-key values |
| plugin registration/lifecycle | `plugin_registers_metadata_*`, configured-provider failure after disposal, live volatile endpoint without re-registration |
| redirect status matrix и default-follow control | два настоящих fixture в `redirect.rs`, ignored только из-за sandbox bind |
| system proxy egress | `egress.rs`: proxy берётся из launch snapshot; fixtures проверяют exact POST/body/headers/success и account-only header + 401 rejection; ignored только из-за sandbox bind |

Близкие source cases объединены в Rust-матрицы. Account route и session event
проверяются через настоящий `AgentRegistry::with_initiator` и production closures;
файловый credential-store composition заменён тем же production service seam. Response
body проходит через настоящий `reqwest::Response`, а фактический socket остаётся
в ignored loopback fixtures. Четыре ignored cases не входят в число пройденных.
