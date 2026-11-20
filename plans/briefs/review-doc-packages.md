# Review: docs/packages.md (HoH package contract, HOH-27) — document review

You only read. Do not run anything that writes; do not edit files. Repository /home/save/Alexandrie/HarnessOverHarness.
The document was written by Sonnet from `research/polyglot-packages.md` and `research/polyglot-node.md` under these CEO
decisions: packages in four kinds (Markdown skill, Rust, Go, TypeScript via Node with the supply-chain scheme), common
contract = separate process, JSON-RPC over stdio, manifest with hash and permissions, cancel, permission intersection;
only the document now, host code after the window-chat milestone; founder: «не прям пароноидально».

Check:
1. Consistency with the code it cites: `crates/hoh-protocol` (approval MAC record, PRIVILEGED_METHODS,
   requires_host_approval), `crates/hoh-tools` (tool definition, output schema), `crates/hoh-skill` (names), `docs/window.md`
   §4–5, `docs/kernel.md`. Any claim about existing code that is false is a `bug`.
2. Security holes in the contract: can a package escalate (choose another identity, widen rights on update without
   consent, reach network/files outside grants, bypass the approval window, read the MAC key, survive cancel/disable)?
   Is the digest definition unambiguous (what is hashed, ordering, normalization)?
3. Protocol gaps that would force a redesign later (versioning, cancel races, concurrent calls, crash/restart, frame limits).
4. Faithfulness to the two research files (no new decisions passed off as theirs); the writer's own proposals are marked —
   judge whether each is sound.
5. Proportionality: anything clearly over-engineered for the founder's stance.

Output: findings, most severe first: kind (`bug`, `security`, `gap`, `over`, `note`), section of docs/packages.md, the
problem, a concrete scenario, the smallest fix. At most 800 words. Say what you did not check.
