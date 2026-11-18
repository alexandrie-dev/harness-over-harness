# Loader part B: one round of fixes after two independent reviews

Part B is handed in (228 tests by your report). Two reviewers read it independently (Sonnet and Kimi, read-only);
the lead checked every finding below against the source. Same boundaries and environment: only
`crates/hoh-loader`; `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-l1`.
The kernel is in a sixth round that fixes R54 and adds the read-only provision store for L19 (see
`plans/briefs/fix3-stage0-k1.md`); when it lands, un-ignore your three tests and drop the workarounds — if it has
not landed when you finish, say so. For every bug: test first, see it fail.

## Bugs

1. **An ancestor's `disabled` is ignored** (`runtime.rs:413-429`; source `vendor/loader/src/config/entry.ts:75-85`).
   `Entry.disabled` walks up `entry.parent.ctx.fiber.entry`: a non-group row is disabled when any owning row's
   `disabled` (value or expression) is truthy; the group row itself always mounts. Input
   `{id: g, group: true, disabled: true, config: [{id: c, name: noop}]}`: the source mounts no fiber for `c`.
   Test also toggling the group's `disabled` and the child following.
2. **`ConfigEditor` rewrites the user's file** (`config_editor.rs:228-314`; source
   `packages/boot/config-editor/src/index.ts:109-146`). The source edits a `yaml` Document (`setIn`, `add`,
   `delete`, `row.delete('config')`) and writes `String(document)`: comments, anchors, layout, order and relative
   plugin names of every other row survive; only `document.contents.flow = false` is forced, and `{__jsExpr}`
   maps are written back as `!!js` scalars. The port re-serializes parsed patches: `# note` is lost and
   `./local.mjs` becomes a `file://` URL. Decision O4 covers only the journal of inclusions, not this. Edit the
   text in place: locate the row and its `config` node by the positions the YAML reader already has
   (`saphyr-parser` marks), replace, insert or delete only that span, and print the new node in the style the
   `yaml` library would (block style; decided in `docs/porting/DEPS.md` 5.5, item 2, variant (a)). Everything
   outside the edited span stays byte for byte. Also: when the rollback `reconcile` fails the source rejects
   with the rollback's error (`:143-146`); the port returns the original. Tests: a file with a comment, an
   anchor, a flow map and a relative insert name, each of the three operations (set config on an existing row,
   add a row, remove the override), comparing the whole file text.
3. **Missing or invalid root file** (`runtime.rs:1402-1436`, `914-943`; source `app-boot/src/index.ts:1168-1179,
   1250-1277`, `vendor/include/src/index.ts:23, 80-82, 175-184`, test `app-boot.spec.ts:1203-1217`). The source
   fails the bootstrap Include and throws a `StartupError` whose diagnostic carries `config file not found:
   <path>`, `failed to read/parse/validate config file`, `config file must be a top-level array`; other failures
   get `<bin>: plugin tree failed to load: `. The port returns raw `LoadError::Io/Yaml/Shape`. Reproduce the
   source's type, texts and prefix; port case #56 with its three messages exactly (today it asserts only the
   variant — PORTING.md section 10).
4. **`initial`** (`runtime.rs:914-934`; source `vendor/include/src/index.ts:180-188`): the source runs
   `ensureIds(initial, …)` before `_writeText(dump(initial))`, so the written file has generated `id`s; the port
   writes `initial` verbatim and adds a newline to JSON. Match the file text.
5. **`recompose` re-reads the base file** (`runtime.rs:1205-1211, 914`; source `index.ts:160-168, 204-207`):
   `internal/update` only schedules an apply over `cache.data`; the file is re-read only on `refresh()`.
6. **`recompose` reports only the first new failure** (`runtime.rs:1255-1259`; source
   `app-boot/src/index.ts:410-420`): the source throws `activationDiagnostic(binName, introduced, false)` for all
   introduced failures with its header. Test with two rows failing in one recompose.
7. **Diagnostics lost the phase `failed to import`** (`runtime.rs:555-559`, `tests/app_boot.rs:48-58`; source
   `index.ts:1056-1072`, `app-boot.spec.ts:663-681`): an optional row without a fiber prints
   `<id> (<name>): failed to import`; the port prints `cannot resolve plugin <name>`. Keep the source's label;
   the reason after it may be ours.
8. **Schema diagnostics lost line and column** (`config_schema.rs:443-455`; source
   `config-schema/collect.ts:148-156`): `invalid YAML include <file> at line <L>, column <C>`.
9. **The published JSON Schema lost its annotations** (`config_schema.rs:192-193`; source
   `config-schema/document.ts:130-160`): `$defs.entry.description`, the top-level `description` and `$comment`
   with the source's texts.
10. **`profile-sanitize` texts** (`profile_sanitize.rs:21-33`; source `profile.ts:546-561`): `<bin>: profile
    manifest <path> must hold a JSON object`, `<bin>: failed to read profile manifest <path>: …`; no `<bin>:`
    prefix on a JSON parse error; a non-object `dsh` or `dsh.profile` is spread as the source spreads it, not
    replaced with `{}`. Assert the exact messages (today `is_err`).
11. **Volatile-only detection is wider than the source** (`runtime.rs:632-651`; source `entry.ts:147-153`
    requires `changes == ['config']`): `disabled: false → null` together with a volatile edit is an ordinary
    update in the source.
12. **Start order of new rows with integer-like ids** (`runtime.rs:362-409`; source `group.ts:49-65, 295`,
    `Reflect.ownKeys`): integer-index keys come first in numeric order, then the other strings in insertion
    order; new rows `[{id:"10"},{id:"2"}]` start as 2, 10. Test through the order of `apply`.

## Exclusions to settle

- `startup-diagnostics` #5 and #7 (`PORT_NOTES.md:522-523`; source spec `:82-94, 111-127`): "JavaScript
  `undefined` cannot be represented" does not excuse the ownership and visibility checks: provide `Arc<()>`,
  keep the second service missing. Port both.
- `app-boot` #1–33 (`resolveConfigPath`, `loadEnv`, `loadLayeredEnv`, `installFailLoud`, with the refusals of
  bootstrap names in `.env`): decided — they belong to the launcher and are ported in the next task (`cmdline`
  and `apps/hoh`); record that owner in the notes instead of "Launcher helpers".
- `profile` #3, #5, #6, #12–#14 (home traversal, profile creation and manifest writing, templates, retired
  bundles) and `config-schema` #39: O6 does not obviously cover them. For each: port it, or cite the decision
  that excludes it.
- A duplicate id across groups of one tree is a move in the source (the fiber is kept), a rebuild in the port:
  record it under "Отличия" with the source lines, or reproduce it if a ported test of the source observes it.

## Done when

`cargo test -p hoh-loader`, clippy with `-D warnings` and fmt check clean; each bug has a test that failed before
the fix. Your last message: the three result lines, one line per item (1–12 and the four exclusions), anything
decided differently and why. At most 350 words.
