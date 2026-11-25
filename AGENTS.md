# HarnessOverHarness

Run this project's tests, builds and browser checks locally.

Use the app's shared, themed Select or Menu for visible dropdowns. Do not ship
native HTML `select` controls in HoH screens. Their OS-rendered white lists and
blue highlights break the app theme. Verify the opened popup and keyboard
interaction in the desktop window for new or changed pickers. See
[HoH design guide](docs/design-guide.md).

For an in-tree Rust plugin, use a separate author worktree. Discover the templates with
`hoh plugin templates --json`. If HoH is not on PATH, use
`cargo run --offline -p hoh -- plugin templates --json` from the checkout.

Use `hoh plugin new <id> --template tool --json`, edit the reported files and run
`hoh plugin test <id> --json`. The report gives the relevant file and a failure log when Cargo runs.
See [Plugin Creator](docs/plugin-creator.md) for the full loop and release indexing.
