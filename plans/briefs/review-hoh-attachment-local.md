# Review: hoh-attachment-local against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model (Codex). You only read. Do not run builds, tests, servers or any command that writes; do not edit or create any file.

## Read
- `docs/porting/PORTING.md` (in Russian) — binding rules; checklist in section 14. `docs/porting/DEPS.md`, the row of `sharp` and decision 5.5 item 7: re-encoded JPEG and WebP cannot match libvips byte for byte and that is accepted; images passed through untouched must keep their exact bytes and `attachmentId`.
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/attachment/attachment-local` (`src/`, `tests/`, `locale/`, `README.md`) and `packages/attachment/attachment` for the service it implements.
- Port: `crates/hoh-attachment-local` (`src/`, `tests/`, `assets/`, `PORT_NOTES.md`); `crates/hoh-attachment`.

## What to check
1. The passthrough rule (`normalization.ts`, `canPassThroughNormalization`): exactly which images are stored as they are — every condition (format, size, orientation, metadata, colour profile, animation, alpha, bit depth). An image the source passes through and the port re-encodes, or the reverse, changes `sha256` and therefore the `attachmentId` in the session log: that is a `bug`.
2. Limits and admission: byte limits, pixel limits, dimension limits, the downscale rule (`fit: inside`, no enlargement), the quality ladder 85/75/60 and when each step is taken, media types accepted and rejected, error codes and texts (`error.ts` of the attachment package).
3. The store (`store.ts`, `file-store.ts`): layout under `DSH_HOME/attachments/v1`, content addressing, staging and atomic publication, directory fsync, aliases by hard link, what happens on an existing target, concurrency (`compression-limiter.ts`), cancellation.
4. `request-image.ts`: what goes into a model request, the cache/upload identity "covering every transform input".
5. References returned: fields, `originalDimensions` only when downscaled, `name` only when given.
6. Tests: the writer reports 96 of 99 source cases ported or adapted and three lazy-loading cases of sharp excluded. Check the exclusions and sample the adapted cases: are assertions weaker than the source's where bytes are not at stake?
7. The CMYK path added last: four-channel decode through `zune-jpeg`, conversion through `moxcms`, the embedded profile in `assets/` (the writer says: libvips' built-in `cmyk` profile, public domain, by Graeme W. Gill, with licence text and SHA-256 recorded). Check what the source does for a CMYK JPEG with and without an embedded profile (sharp's `toColourspace('srgb')`; the notes cite the lines), and whether the provenance and licence statement of the asset is supported by what is in the repository; say "not checked" for anything you cannot see on disk.
8. Each entry of `PORT_NOTES.md` "Отличия", in particular "repeated home-directory fsync instead of global mutable caching".

## Triage rule
This is a port to Rust, not an emulation of JavaScript. Pixel and byte differences of re-encoded images are accepted by decision; anything else that an input the ported API accepts turns into a different result, error, text or stored bytes than the source is a `bug`. Statements about sharp, libvips or Node made from memory must be checked in the clone or marked "not checked".

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `unjustified exclusion`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete input with both outputs, and a test that would confirm it. If a category has no findings, say so in one line. At most 900 words. Say plainly what you did not check.
