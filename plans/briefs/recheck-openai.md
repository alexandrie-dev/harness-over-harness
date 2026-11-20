# Closure check: hoh-llm-openai fix round

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
Writer s5c reports: cargo test 89/0 (15 new in tests/fix_round.rs), clippy -D warnings and fmt --check clean.

Original review: `/mnt/toshiba/alexandrie/hoh/logs/rv-openai.full.md` (items 1–6; item 7 `models` in the protocol is out of scope).
Port: `crates/hoh-llm-openai` (src/, tests/, README). This crate carries the founder's API key to any OpenAI-protocol
provider in the desktop milestone.

For each item 1–6: closed / partly / not closed, with Rust file:line and the test that proves it. Check in particular:
- the key cannot reach Debug, Display, errors, ProbeResult, warnings or logs on ANY path (including retry with
  max_completion_tokens, list_models, SSE mid-stream errors, HTTP client errors that embed the URL or headers);
  `redact_text` must not be bypassed by an error constructed before redaction or by a short/empty key matching everything;
- the max_tokens → max_completion_tokens retry fires once, only on "unsupported parameter", and the remembered choice
  cannot leak between adapters with different configs;
- tests in tests/fix_round.rs would fail on the old behaviour (judge by reading).
Also report any new defect the fix introduced. Output ≤500 words, Russian, most severe first.
