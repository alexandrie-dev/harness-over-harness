---
name: hoh-plugin-authoring
description: Interview the author, agree on a plan, then scaffold, implement, and test an in-tree Rust plugin in the selected HarnessOverHarness checkout.
---

# HoH plugin authoring

## Start with an interview

Use this interview in every new Plugin Creator chat, including when the initial request looks complete. Use the person's language. Read the selected checkout's `AGENTS.md` and any relevant existing code or contracts if needed to ground your questions. During this phase, use only read-only inspection.

- Ask exactly one design question per turn and wait for the answer. With each question, offer your recommended answer and a short reason. Use what the person already said and what you can verify in the checkout. Cover purpose, what the plugin may read or change, scope, failure behavior, and how success will be verified, as relevant to this plugin.
- The first reply must include a question. If the brief already answers the essential design questions, summarize the proposed plan and use the question to ask for explicit approval.
- When the decisions are clear, post a short plan in this chat covering what will change, what is out of scope, and how you will test it. Ask for explicit approval of that plan. Wait for approval before scaffolding, writing files, running implementation commands, or delegating implementation. An instruction to start immediately in the initial request does not replace this confirmation.
- If the person changes the agreed scope or a new trade-off changes the plan, ask about that decision and get approval for the revised plan before implementing it. Keep routine progress updates and implementation steps within the approved plan.

## Implement the agreed plugin

Work in the selected HoH checkout. Read its `AGENTS.md` and use `docs/plugin-creator.md` for the authoring workflow.

1. Use `hoh plugin templates --json` and `hoh plugin guide tool` to choose a starting point. If HoH is missing from PATH, use `cargo run --offline -p hoh -- plugin` as the command prefix.
2. Use `hoh plugin new <id> --template tool --json` for a callable tool or the `basic` template for a minimal plugin. Edit the reported files. For a session event or panel, use `hoh plugin guide session-panel` and read `plugins/turn-note`.
3. Run `hoh plugin test <id> --json` locally. Fix the reported diagnostics and read the full log only if the summary is insufficient. The tool template tests a successful call, bad input, and cleanup on unload.
4. Report the plugin ID, changed files, and test result. Inclusion in `plugins/index.json` is a separate release decision after source review.
