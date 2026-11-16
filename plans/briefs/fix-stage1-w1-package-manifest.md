# Fix round for `hoh-package-manifest` after an independent review

This round is only about `crates/hoh-package-manifest`. A second message will follow later for
`hoh-native-command`; do not touch that crate now. Same boundaries and commands as
`plans/briefs/stage1-writer.md`; environment
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w1`.

The source package is types only: nothing in it runs, the readers of a manifest validate it themselves
(your own first line of `src/types.rs` says so). A Rust type with `serde` does run, so it must not change
what goes through it. Three things do today.

## 1. `LocalizedText` reorders its keys

`LocalizedText::Translations { en, locales }` always writes `en` first. `{"zh":"x","en":"y"}` comes back as
`{"en":"y","zh":"x"}`. This type travels in data that is compared byte for byte (the model list of
`packages/llm/llm/src/types.ts:9`, `packages/web/web/src/types.ts:9`, the plugin market), and JavaScript
keeps the order in which the keys were written.

Decided: `Translations` holds one ordered map with every locale, `en` among them, in the order of the
input; the type offers a way to read the `en` text. A map without `en` still parses (the source has no
check at run time; `packages/util/text-search/src/index.ts` and the other readers decide what to do).

## 2. `DshEnginesManifest` reorders its keys

`dsh`, `node`, `npm` are written first, then the rest: `{"node":">=20","dsh":"1"}` comes back reordered.
Decided: one ordered map of every engine in input order, with accessors for the three named ones.

## 3. Tests that can see order

`tests/json_shapes.rs` compares `serde_json::Value`s, and equality of JSON objects ignores key order. Add
round trips compared as text (`serde_json::to_string` of the parsed value equals the input string) for
`{"zh":"x","en":"y"}`, for an `engines` object with `node` before `dsh` and an unknown engine between them,
and for one full manifest whose keys are not in the order of the struct's fields — and say in the notes
which order a struct writes (its field order, as `JSON.stringify` writes an object built field by field).

## 4. Notes for the consumers

`PORT_NOTES.md`, section on differences, in Russian, for whoever ports `packages/boot/app-boot`:

- `app-boot` declares `ProfileManifest = Partial<DshPackageManifest>` (`packages/boot/app-boot/src/profile.ts:45`):
  a local profile has no `name` or `version`. `DshPackageManifest` here keeps them required, as the source type does;
- `app-boot` reads a `package.json`, changes `dsh.profile.bundles` and writes the whole document back
  (`profile.ts:548-617`, `readProfileManifest`, `withBundles`, `writeProfileManifest`), keeping every key it
  does not know and the order of all of them. The typed structs of this crate drop unknown keys, write keys
  in field order, erase an explicit `null`, and fail on a value of an unexpected type where the source's
  readers report their own text (`bundlePatchFiles`, `profile.ts:54-67`). Reading and rewriting an existing
  manifest must therefore work on the JSON object itself (`serde_json::Map`, which keeps order in this
  workspace); these structs are for building a new manifest and for reading a part already checked;
- which exports are used only by packages that are not ported (`DshClientModuleManifest` and the other
  client module types: their consumers are `packages/client/*`), instead of "the consumers use only four types".

If you see a simpler way to meet items 1–3 than the one decided here, keep the decided behaviour and say so.

## Done when

`cargo test -p hoh-package-manifest`, `cargo clippy -p hoh-package-manifest --all-targets -- -D warnings` and
`cargo fmt -p hoh-package-manifest -- --check` pass. Write the three order tests first and see them fail.

Your last message is the report: the result line of each command, one line per item. At most 150 words.
