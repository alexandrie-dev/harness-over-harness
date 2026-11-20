# Closure check 2: hoh-llm-openai fix2

You only read. Do not run builds, tests, servers or anything that writes; do not edit files. The lead runs the tests.
Previous check: `/mnt/toshiba/alexandrie/hoh/logs/rc-openai.report.md` (item 2 partly, item 5 new defect). Writer s5c reports
97/0 (8 new in `crates/hoh-llm-openai/tests/fix_round2.rs`); README updated.
Claims: model IDs containing the key are dropped (`model_ids` in src/http.rs) in list_models and probe; `redact` walks
Display/Debug of every cause and `source()`, replacing a tainted chain with one clean error; a key shorter than 8 chars is
replaced only as a whole token; the max_tokens retry fires only on status 400 with the field named next to an
"unsupported/unknown/unrecognized/not supported/extra inputs" phrase.
For item 2 and item 5: closed / not closed, file:line, test, fails on old code? Can the key still reach any output (Debug,
Display, ProbeResult, LlmModelInfo, warnings) — including a short key inside a URL, a key split across two error causes, or
a provider that echoes the key with different case? Is the whole-token rule for short keys safe (a 6-char key next to `/`
or `=` in a URL query)? Any retry false negative for real OpenAI/compatible error texts you know of? New defects. ≤400 words, Russian.
