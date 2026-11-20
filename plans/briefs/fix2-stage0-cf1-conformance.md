# cf1: what the closure check left open

A read-only closure check of your fix round closed items 1–3. Item 4 is partly closed: the drop-in fake through
`execute` works, but the sensitivity tests do not yet prove "each mutation fails in its phase for its reason".
Fix these in `crates/hoh-conformance` only; same environment (`target-cf1`).

1. The four log mutations (`tool`, `missing`, `reorder`, `usage`) are all accepted as phase `log` with reason
   prefix `session role ` (`tests/runner_process.rs:277-289`). Make each expected reason specific enough that one
   mutation cannot pass for another's reason.
2. ACP no-model scenarios (`acp/handshake`, `reject-extra-dirs`): five of six mutations corrupt `agentInfo.name`.
   Give each mutation its own target where the scenario has one; where it cannot, list the scenario as
   not sensitive to that mutation, with the reason.
3. Restore the hit counts the old test had (`HEAD:tests/corpus.rs:320-321`: `tool_count > 100`, `usage_count > 200`)
   or an equivalent: how many scenarios mutate a real tool or usage field rather than the synthetic fallback.
4. `reorder` is a no-op on a two-row log (`src/bin/fake/peer.rs:255-257`); restore the `last > 1` guard or choose
   rows that change the order, and count the scenarios where it applies.
5. Headless exit gating: a test through `execute` where a headless run writes correct logs but exits non-zero
   must fail in phase `exit`.
6. Regression of the round: `migration_gate` now runs after the comparisons (`src/runner.rs:953-960`), so a fixture
   older than v4 can report a comparison diff instead of "migration vX->vY not ported". Restore the gate before
   the comparisons, with a test.
7. `DSH_MCP_DEMO_URL`: decide from dsh's harness whether the runner must set it for the two scenarios that need it;
   do it, or record why not.

Report: per item what changed and the test, result lines. At most 250 words.
