# Codex app-server port notes

The adapter follows T3 Code's `CodexAdapterV2` and `CodexProvider`: `initialize`, `model/list`,
`account/read`, `thread/start` or `thread/resume`, `turn/start`, streaming `item/*` notifications,
server-initiated approval requests, and `turn/interrupt`.

The four access rows intentionally match T3 exactly. Authentication is never loaded by HoH;
the installed `codex` child inherits and owns its normal authentication environment.
