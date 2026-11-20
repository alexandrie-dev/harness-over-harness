# Research addendum (HOH-27): Node as the runner for TypeScript packages — only with a supply-chain scheme

You only read. Do not run builds, tests, servers, installs or anything that writes; do not edit files except the one
output file `research/polyglot-node.md` (Russian, plain words). You may search the web for current facts (cite URL and
date). Repository: /home/save/Alexandrie/HarnessOverHarness. A parallel researcher writes `research/polyglot-packages.md`
(general mechanism, Wasm, embedded JS engines) — do not duplicate it; you cover the Node variant and compare.

## Founder's words (19.11)
«ну да, то есть агент может добавлять какие-то тулзы, да» — packages are tools the agent can add (product extensions).
«Node надо решить что делать, если ты придумаешь как продукт сделать защищенным от supply-chain то можно и node».
«мы на самом деле не прям пароноидально думаем» — keep the measures proportionate.
Boundaries (CEO): only for extension packages. The dsh engine in TypeScript is NOT taken into the product
(«dsh не трогаем делаем свое»). Nothing is installed on this machine now.

## Read
`research/supply-chain-ts.md` (our layers — the basis), `docs/window.md` §5 and §7, `scripts/ui-supply-check.sh` and
`scripts/tests/ui-supply/README.md` (what our window build already enforces: hashes, 7-day age, install scripts off,
licenses, audit, clean environment), `research/polyglot-packages.md` if it already exists.

## Describe for "Node shipped with the product or next to it"
1. Where Node itself comes from and how its binary is verified (official release, SHASUMS256 + signature, pinned version,
   update policy; bundled vs. user-installed; size).
2. How a package written by the user or by the agent gets dependencies: pinned versions and hashes (lock file), age delay
   for new versions, install scripts off, install without network or through our proxy/mirror with an allow-list; what
   tooling (pnpm with its settings, or our own fetcher) and what we would write.
3. What the package code runs in: sandbox (Node permission model `--permission`, plus OS-level: bubblewrap/landlock as we
   already use for bash), files and network by allow-list, rights confirmed on save (our approval flow with MAC).
4. What the user sees in the window (install, rights, updates, warnings) — plain words.
Then compare with the variants without Node (embedded JS engine; TS→Wasm; external runtime) on the same four points and
on volume of work (marked as estimate). Recommend one, proportionate to the founder's stance.

## Output
`research/polyglot-node.md`, at most 1000 words. Say what you did not check.
