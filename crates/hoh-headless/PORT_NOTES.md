# Port notes

- `cordis.patch.yml` is copied unchanged and exposed through `CORDIS_PATCH`; it remains profile
  data instead of being re-expressed as Rust configuration code.
- Node process globals from `runner-internals.ts` and `startup-internals.ts` are represented by
  the optional `HeadlessRuntimeKey` host service. Its default uses process stdio, while tests and
  embedders can provide isolated sinks, terminal detection, and stdin reading without mutating
  process-global state.
- Session event projection intentionally reads the open `SessionEvent` JSON vocabulary. This
  preserves plugin-owned event extensions (notably `agent-preset/selected` and structured tool
  errors) without claiming ownership of their schemas.
- Rust listeners are lifecycle-owned by the mounting context. Projection and reasoning handles
  additionally carry an active flag so their observable behavior stops synchronously, matching
  the source disposer even though kernel listener teardown itself is asynchronous.
- Empty `reasoning-delta` frames are explicitly inert, as in `src/index.ts`: they neither print
  nor close an already open reasoning line. A regression covers `"abc"`, `""`, `"def"`.
- Tool arguments and every emitted JSON line use `hoh-util-values`' established V8-compatible
  `JSON.parse`/`JSON.stringify` implementation. This preserves JavaScript rounding and spelling
  for `1.0`, exponent input, integers beyond 2^53, and negative zero without a new dependency.
- `HeadlessStartupKey::view` omits absent optional properties instead of turning JavaScript
  `undefined` into JSON `null`. The profile's truthiness checks therefore see the same shape.
- `SESSION_QUERY_SESSION_NOT_FOUND` is translated to the public `--session-id` diagnostic both
  during observation and during the later `agents.resume` race, matching the single TS `try`.

## Test inventory

The source has 118 `it` declarations: `headless.spec.ts` 61, `json-stream.spec.ts` 31,
`startup.spec.ts` 22, and `terminal-failures.spec.ts` 4 (several are parameterized). Rust uses
table-driven and seam-level tests rather than copying every fixture declaration: 49 test
functions cover the package-owned contracts after this fix round. In particular they now cover
the full fresh-run order (initial idle → `followup` → settled idle → flush → stdout → exit),
terminal-failure recovery and foreign-agent isolation, final-turn summary reset, every
`resolve_agent` stage and resume-race translation, all adoption rejection facts, reasoning
continuity, JSON numeric semantics, JSON run output, permission/approval/add-dir guards, terminal
identity, disposal, usage aggregation, event boundaries, startup JSON error channels, option
operands, and `--` handling.

The following source fixture mechanics are intentionally not duplicated one-for-one:

- Node module-namespace/default-export and mutable `process.stdin/stdout/stderr` assertions have
  no Rust runtime counterpart; native service composition and injected `HeadlessRuntimeKey`
  sinks are tested instead.
- Permission preset mutation, approval-policy persistence, subagent tree scheduling, and
  persistence lease acquisition have their state machines in their accepted owner crates. This
  crate tests its ordering, missing-service and sandbox-policy guards, arguments, observation and
  adoption gates, resume setup, and error translation at those seams rather than recreating the
  owners' fixtures.
- JavaScript non-`Error` Promise rejection is represented by mandatory `BoxError`; the observable
  string diagnostic path is shared for every Rust error value.

## Fix-round regressions

- The UTF-8 truncator drops a real terminal U+FFFD only at an exact cut boundary. When the cut
  splits the following scalar, Rust's boundary rollback preserves the preceding real U+FFFD,
  matching the value left after Node removes only its decoder-created replacement character.
- The last-resort bounded record omits `type` when the source object has no such property instead
  of emitting `"type": null`.
- The native run test proves the complete fresh-agent sequence, including both idle barriers,
  `followup`, session flush, output, and exit notification.
