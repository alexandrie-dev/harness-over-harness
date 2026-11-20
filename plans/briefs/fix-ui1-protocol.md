# ui1: fix round for hoh-protocol and hoh-window-stub after review

A Sonnet read-only review found security gaps in the approval path; I changed `docs/window.md` §5 items 4 and 6
accordingly — re-read them. Fix the following in your crates (same ownership and environment as before). Each
with a test that fails before the fix.

1. **Privileged methods.** Add `PRIVILEGED_METHODS` to `hoh-protocol` (methods that grant or widen rights:
   `hoh/employee/create|update` carrying `permissions`, `hoh/task/draft/decide` with consent, `hoh/task/rollback`
   — and any other you find in `hoh.rs` that carries `Permissions`). The stub must answer each only after a host
   `approval/request` with a valid `mac`; tests per method.
2. **Initialize.** `InitializeResult` gains optional `hoh: {protocolVersion, approvals: "host"}` (dsh clients ignore
   it). A second `initialize` is rejected and the first key still verifies — test.
3. **What the MAC covers.** HMAC-SHA256 over a canonical record `{approvalId, outcome, actionHash, risk, humanText,
   expiresAt}` (state the canonical form: key order, encoding, separators — not ad-hoc `|` joining); `actionHash`
   defined over a canonical action record that the core can recompute; schema patterns and length limits for
   `approvalId`, `actionHash`, `mac`.
4. **Verification in one place.** Move MAC, hash and expiry verification into `hoh-protocol` (a module the Tauri core
   will reuse). Tests with independently pinned vectors (computed once by a different implementation — e.g. `node -e`
   with `crypto.createHmac`, record the command in the test comment), plus: mismatched `approvalId`, expiry,
   replay of an old valid answer on a new request. `validate_params`/`validate_result` use the host approval types
   in host mode.
5. **Byte-for-byte for real.** Serialize each typed fixture with the typed structs (not the stored `Value`) and
   compare the string with the file; swapping two fields in a type must fail a test. Rename tests whose names
   overstate what they check.
6. **dsh client frames.** `{"jsonrpc":"2.0","id":1,"method":"shutdown","params":{}}` and a frame without `params` for
   a no-argument method must be accepted by the typed `Request` path as dsh's server accepts them (`transport.ts:129`,
   `server.ts`). Test both.
7. **Generated TypeScript.** Split core-only types (`HostApprovalDecision`, `HostApprovalRequest`, `approvalKey`) into
   a separate generated module the window does not import; window-side `Request` contains only methods the window
   may send. Type-check is not available (no `tsc` installed yet) — keep it pure type declarations.
8. **Notes from the review:** integral fields (`time`, `bytes`, `width`, `height`, `maxTokens`) as integer types so
   they print `0`, not `0.0`; `maxTokens` positive safe integer as dsh (`server.ts:219`); `session/query` rejects extra
   keys as dsh; `ApprovalOutcome` includes `unavailable` as dsh's log; stub expiry yields `cancelled` for that action,
   not a failed turn; the "waits for the answer" test proves waiting (the answer arrives after a delay and the turn
   resumes) rather than a 30 ms absence; remove the hard-coded `.skip(3)`; a line-size limit for non-stdio transports
   (configurable, default off for stdio as dsh).

Report: per item what changed, result lines. At most 350 words.
