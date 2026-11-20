# Review: hoh-protocol and hoh-window-stub (the window ↔ engine protocol)

You are an independent reviewer. The code was written by Codex. You only read: no builds, tests, servers, edits
or new files.

## Read
- `docs/window.md` (Russian), sections 4 (protocol), 5 (security) and 6 (packages 2 and 3), and section 7.
- The brief the writer had: `plans/briefs/window-ui1-protocol-stub.md`.
- dsh's SDK protocol (read-only clone `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`):
  `packages/sdk/protocol/src/types.ts`, `transport.ts`, the SDK client and `snapshots/sdk/`.
- Code: `crates/hoh-protocol`, `crates/hoh-window-stub`, `protocol/` (fixtures, schemas),
  `scripts/gen-protocol-ts.mjs`, `ui/src/protocol.ts`, both `PORT_NOTES.md`.

## What to check
1. Every SDK method, notification and reverse request of dsh is present with the same names, field names, key
   order and optionality; dsh's own SDK clients would work against an engine that speaks these types unchanged.
2. Framing: what `transport.ts` does with a blank line, an oversize line, invalid JSON, a response without a
   pending request — the port does the same. Framing is separate from the transport (a later network transport).
3. The approval path: HMAC-SHA256 over what exactly (canonical form, key order, encoding), the key never reaching
   the window side, replay (`approvalId` reuse, expiry 300 s), what happens without `mac` in host mode. Look for a
   way the window could forge or replay an approval. This is the security-critical part.
4. The new `hoh/*` methods against section 4 of `docs/window.md`: anything missing or contradicting it; the writer's
   choices where the design was open — are they reasonable, and are they recorded.
5. Generated TypeScript: does it express the same optionality and unions as the schemas (e.g. `null` vs absent)?
   Is the "output matches the committed file" test real?
6. The stub: does it play the three scenarios as a client of dsh would see them, including reverse requests and
   waiting for answers? Are the tests comparing against the recordings, or against the stub's own output?

## Output
Findings, most severe first: kind (`bug`, `security`, `design gap`, `missing test`, `note`), `file:line`, the
source or design line, a concrete input with both outcomes, a confirming test. At most 800 words. Say what you
did not check.
