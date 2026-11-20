# Review: hoh-tool-fs, hoh-working-directory, hoh-tool-working-directory against their TypeScript source

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files. The lead runs the tests
(writer s8 reports 269/0, 21/0, 1/0). Read `docs/porting/PORTING.md` (§14). Sources under
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/` (tools/tool-fs or wherever MAP.tsv points: see docs/porting/MAP.tsv
rows for these crates; session/working-directory; session/tool-working-directory). Ports: `crates/hoh-tool-fs`,
`crates/hoh-working-directory`, `crates/hoh-tool-working-directory` (read PORT_NOTES first: deferred cases, SHIMs).

These tools are what the agent uses to read and write the user's files in the desktop milestone. Priority:
1. Path safety: resolution against the working directory, `..`, symlinks, absolute paths, paths outside the allowed roots;
   what is refused and with what exact text; no write outside the allowed area.
2. Writes and patches (`apply_patch`, write/edit): atomicity, partial failure, exact result texts and the content returned
   to the model, line endings, encodings, large files, binary files.
3. read / read_image: limits, truncation markers, offsets, image handling, error texts.
4. Approval: which operations require approval and that a rejected approval leaves the file untouched.
5. working-directory: change, persistence, notifications (the directory_agent SHIM — what is lost without the real Agent).
6. Deferred cases: is any of them needed for the milestone path (read/write/patch in the chat)?
Triage: unobservable type differences → note; bug = accepted input with different result/text/side effect, data loss.
Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, test to add. ≤800 words, Russian.
