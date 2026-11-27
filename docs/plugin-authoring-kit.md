# In-repository Rust plugin kit

## Start from the plugin APIs

Use this guide as the capability contract for the interview. In the authoring
workflow below, scope any code inspection to the plugin being authored and the
named example. Start with the bundled docs and scaffolds. During the interview,
those are sufficient to confirm the capabilities listed here. If one is missing,
say which extension point is needed and ask the author whether to change the
plan or request a separate HoH feature. Explore or change HoH core only in an
explicitly approved core task.

| Need | Available authoring path |
| --- | --- |
| Lifecycle and kernel services | `hoh plugin new <id> --template basic --json`. Declare typed service dependencies and use the mount test. |
| Callable tool | `hoh plugin new <id> --template tool --json`. The template includes argument validation, a real invocation and an unload test. |
| Session lifecycle and results in a panel | `hoh plugin guide session-panel --json`. The `plugins/turn-note` sample shows a turn-end listener, a plugin record and a manifest-defined panel. |
| Dictation in the chat composer | The published kit has no extension point for microphone capture, per-chat recording controls or inserting text into the draft. This requires a separate HoH API. |

After approval, use the scaffold report's `editFiles` and verify with
`hoh plugin test <id> --json`.

## Checkout and integration

External plugins will live in the HoH user profile's separate
`external_plugins` directory. The external loader is planned. Authors or CI
will build compatible artifacts ahead of time; users will install the ready
snapshot without rebuilding HoH. The commands below author Rust components
linked into the engine today.

From the repository root, run `hoh plugin new my-plugin`. The command creates `plugins/my-plugin` with a manifest, a `Plugin` implementation and a smoke test; it refuses to overwrite an existing directory. `plugins/turn-note` is an example that records the outcome of each completed session turn and registers `last_turn` in the tool registry. Calling that tool from an external model is a separate integration.

Use the [Plugin Creator workflow](plugin-creator.md) for scaffolding, metadata checks and offline package tests. For an engine build in the author worktree, run `scripts/plugin-dev.sh my-plugin` with `CARGO_HOME` and `CARGO_TARGET_DIR` set. It links the explicit release list in `plugins/index.json` plus the selected candidate and prints `HOH_ENGINE_PATH` for an existing desktop window. After review, add the ID to the release list and run `python3 scripts/generate-plugin-index.py` to update the checked-in dependency and Rust indexes.

`hoh-plugin.json` supplies `id`, `entry` (a public Rust type implementing `hoh_kernel::Plugin`), version, name, and optional `panel`. A panel specifies a record suffix and labeled fields; the window reads the latest matching `plugin:<id>/<record>` from the selected loaded session and renders those values without plugin JavaScript. A plugin uses `append_plugin_record` to append such records; the session layer marks them ignorable.

An optional `repository` string in `hoh-plugin.json` links the plugin’s details to its source. Use an HTTPS URL for the repository or the plugin directory in a monorepo.

The Plugins page lists the linked manifests and loader status. It reports when the selected conversation is not loaded. Activation and configuration diagnostics are deliberately generic and do not expose arbitrary error text. In-process Rust code has the privileges of the engine; this kit does not implement a process sandbox, signatures, snapshot approval, or a session-permission broker.
