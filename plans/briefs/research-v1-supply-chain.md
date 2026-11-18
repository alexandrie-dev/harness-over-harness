# Research: a TypeScript interface without supply-chain exposure

Working directory `/home/save/Alexandrie/HarnessOverHarness`. Read `plans/briefs/v1-requirements.md` first.
Write exactly one file, `research/supply-chain-ts.md` (in Russian, plain words, no invented terms); change
nothing else, install nothing, run no builds. You may search the web; cite every source with its URL and date.

The founder allows the window of HoH to be written in TypeScript on one condition: a scheme that protects the
product from supply-chain attacks, strong enough to be shown to customers as a feature. The product is a
closed-source signed desktop binary with auto-update (Tauri 2 shell, engine in Rust), installed at companies
that run a local model and care where their data goes.

Answer, concretely enough to be turned into rules and scripts:

1. The attacks of the last two years on the npm ecosystem that such a scheme must stop (name the incidents,
   the mechanism of each, the date): malicious lifecycle scripts, maintainer account takeover, typosquatting
   and dependency confusion, compromised build tooling, self-propagating worms, poisoned updates of popular
   packages. For each: at which moment the damage happens (install, build, run).
2. A scheme in layers, each with the tool or setting that enforces it and what it costs us:
   - choice of dependencies (a small allow-list, a ceiling on the number of transitive packages, criteria);
   - fetching (pinned lockfile with integrity hashes, a minimum age of a version before it may be taken,
     lifecycle scripts off, a vendored or mirrored store so that builds run with no network);
   - review of every update (diff of the package contents, who reviews, automated checks);
   - the build (isolated, reproducible, no secrets present, provenance);
   - the shipped product (no `node_modules` and no package manager at run time: the interface is a static
     bundle inside the signed binary; the webview's content security policy; Tauri capabilities so that
     interface code cannot read files, spawn processes or reach the network except through named engine
     commands);
   - updates of the product itself (signature, rollback).
   Compare package managers and settings as they are today (npm, pnpm, bun, yarn; `ignore-scripts`,
   `minimumReleaseAge` and similar; trusted publishing and provenance attestations) from their current
   documentation.
3. What remains exposed after all layers, said plainly.
4. The same question for the Rust side (crates.io): which of these layers we already have (offline builds,
   `Cargo.lock`, `docs/porting/DEPS.md` as the allow-list) and what is missing (for example `cargo-vet` or
   `cargo-deny`), without installing anything.
5. A one-page rule set for this repository: what a writer may and may not do when a TypeScript dependency is
   needed, and the checks a lead runs before accepting it.
6. How this is told to a customer in five sentences without overstating it.

At most 2500 words. Mark what you did not verify.
