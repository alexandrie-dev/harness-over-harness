---
name: hoh-plugin-authoring
description: Scaffold, implement, and test an in-tree Rust plugin in the selected HarnessOverHarness checkout.
---

# HoH plugin authoring

Work in the selected HoH checkout. Read its `AGENTS.md` and use `docs/plugin-creator.md` for the authoring workflow.

1. Use `hoh plugin templates --json` and `hoh plugin guide tool` to choose a starting point. If HoH is missing from PATH, use `cargo run --offline -p hoh -- plugin` as the command prefix.
2. Use `hoh plugin new <id> --template tool --json` for a callable tool or the `basic` template for a minimal plugin. Edit the reported files. For a session event or panel, use `hoh plugin guide session-panel` and read `plugins/turn-note`.
3. Run `hoh plugin test <id> --json` locally. Fix the reported diagnostics and read the full log only if the summary is insufficient. The tool template tests a successful call, bad input, and cleanup on unload.
4. Report the plugin ID, changed files, and test result. Inclusion in `plugins/index.json` is a separate release decision after source review.
