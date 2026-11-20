# Triage of a review: confirm each finding against the source

You check a code review for the lead of a porting project (DeepSeek Harness, TypeScript → HoH, Rust). You only
read; no builds, tests, servers, edits or new files. Source clone (read-only):
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`. Rules: `docs/porting/PORTING.md` (Russian), triage rule in
`.claude/rules/acceptance.md` of `/home/save/Alexandrie` (an error, a violation of a direct requirement, or an
optional recommendation are different things).

The review to triage and the crate are named in your task. For **every** finding:
1. Open the Rust and TypeScript lines it cites; read enough around them to judge.
2. Verdict: `confirmed` (the input it gives really produces the different result it claims — trace it), `rejected`
   (say exactly why, with lines), `inherited` (real, but the cause is in another crate — name it and the line),
   or `optional` (a recommendation, not a divergence).
3. For `confirmed` and `inherited`: the smallest fix instruction a writer can act on, and the test that proves it.

Also: did the reviewer miss something obvious in the same area while you were there? Report it only if you traced it.

Output: a numbered list in the order of the review, one block per finding, then a short "to the writer" section
that contains only confirmed items, ready to paste into a fix brief. At most 600 words.
