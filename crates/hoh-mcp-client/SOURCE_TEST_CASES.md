# Матрица исходных тестов

`it.each` считается одним исходным кейсом по правилу brief. Всего в `*.spec.ts` — 184 объявления; 23 e2e-кейса исключены `PORTING.md` §10.

Матрица ниже связывает исходные suites с перенесёнными контрактами, но не означает case-for-case порт: после review-fix исполняются 66 Rust-тестов (35 unit + 31 integration/contracts), а близкие TS boundaries объединены. В частности, review-regressions отдельно проверяют lifecycle supervisor и terminal unconfirmed close, close-timeout classification, retry/give-up/logging/unload, same/missing credential refresh и снятие tools, strict startup и reservation; output-schema execution/JS leading lookahead/local `$ref`/isError/redaction; late optional-service mount; probe pagination/caps/timeout/abort; transport scrub/merge env; config bounds и diagnostics.

| исходник | кейсов | перенос/проверяемый контракт |
|---|---:|---|
| `apply.spec.ts` | 24 | Config/defaults/serverName, plugin startup, reservation, strict/non-strict failure, transport choice, instruction byte limit: `contracts.rs`, `runtime.rs` |
| `credentials.spec.ts` | 6 | provider requirement, one resolution/pass, unchanged raw config, redaction and probe expansion: `credentials.rs`, template/redaction tests |
| `diagnostic.spec.ts` | 14 | cause chain, deadline/local codes, request classification, longest-first redaction: `diagnostic.rs`, `deadline.rs`, contract tests |
| `egress.spec.ts` | 1 | Streamable HTTP uses workspace `reqwest` transport; network/proxy e2e remains conformance responsibility |
| `generation.spec.ts` | 4 | negotiation, independent probe/startup budgets, bounded cleanup and closure result: duplex generation unit-test plus `generation.rs` |
| `load-path.spec.ts` | 1 | namespace-style named Rust exports and `NAME`; Rust has no default export |
| `mcp-client.spec.ts` | 53 | naming, discovery/registration rollback, raw-name calls, canonical result/schema checks, images, projections, transports/env and primitive args: `tools.rs`, `transport.rs`, contract tests |
| `negotiation-lifecycle.spec.ts` | 5 | one generation per attempt, close-before-retry and unconfirmed cleanup stop: `runtime.rs`, `generation.rs` |
| `probe.spec.ts` | 12 | identity, pagination/caps, stages, timeout, validation and close result: `probe.rs`, duplex generation test |
| `protocol.spec.ts` | 6 | no-tools server, resources/cursors/read, schema error and call cancellation: `generation.rs`, `server_context.rs`, `tools.rs` |
| `reconnect.spec.ts` | 37 | retry cap/backoff/stability, no late publication, close gating, disposal races, credential redaction: supervisor state machine and reconnect contract tests |
| `server-context.spec.ts` | 2 | scoped provider plus literal attributed instructions and joint lifecycle: `runtime.rs`, instruction tests |
| `status.spec.ts` | 17 | initial/current/closure, transitions, observer containment, stable Fiber lookup and terminal statuses: `status.rs`, `runtime.rs`, contract tests |
| `tool-definition.spec.ts` | 2 | exact canonical result, invalid result and task/isError codes: `tools.rs` |
| `mcp-client.e2e.ts` | 23 | не переносится: внешний node/process/localhost e2e, `PORTING.md` §10; wire negotiation покрывает in-memory `rmcp` server |
