# HarnessOverHarness

Run this project's tests, builds and browser checks locally.

For an in-tree Rust plugin, use a separate author worktree. Discover the templates with
`hoh plugin templates --json`. If HoH is not on PATH, use
`cargo run --offline -p hoh -- plugin templates --json` from the checkout.

Use `hoh plugin new <id> --template tool --json`, edit the reported files and run
`hoh plugin test <id> --json`. The report gives the relevant file and a failure log when Cargo runs.
See [Plugin Creator](docs/plugin-creator.md) for the full loop and release indexing.
