# w3: third round for hoh-attachment-local

The closure check of round 2 closed all 8 items. Three things remain; facts below were established from the
libvips 8.18.7 and sharp 0.35.5 sources (permalinks given; you have no network — trust these, cite them in
PORT_NOTES). Each with a test that fails before the fix. Same environment as before (`target-w3`).

1. **Write errors swallowed (data loss).** `src/store.rs:238-250`: `write_all` then `sync_all` without `flush()`.
   tokio's `sync_all` discards the write error (tokio-1.54.1 `fs/file.rs:317-323, 1194-1204`), so on ENOSPC/EIO/EFBIG
   a short temp file is published. Add `handle.flush().await` (mapped to the source's write error) before `sync_all`,
   everywhere you write through `tokio::fs::File`. Test: a child process / dedicated test binary with
   `setrlimit(RLIMIT_FSIZE)` and SIGXFSZ ignored — the store call fails with the errno and leaves no published file.
   `docs/porting/PORTING.md` §6 states the rule.
2. **APP13.** libvips keeps an APP13 segment as IPTC metadata only if `data_length > 5` and the data starts with the
   five bytes `Photo` (jpeg2vips.c v8.18.7 L719-L735,
   https://github.com/libvips/libvips/blob/v8.18.7/libvips/foreign/jpeg2vips.c#L719-L735); sharp reports `iptc`
   only then (metadata.cc v0.35.5 L120-126, L315-316). Round 2 counted any APP13 (`src/image.rs:195`). Fix: count
   exactly that condition; tests: `Photoshop 3.0\0…`, `PhotoXYZ…`, `Photo` (length 5, not counted), other prefix
   (not counted, JPEG passes through unchanged).
3. **WebP scale-on-load and JPEG load rounding.**
   - WebP: with `shrink = min(h, v) > 1` from ResolveShrink, sharp reloads with `scale = 1/shrink`; the loaded size
     is `rint(W·scale) × rint(H·scale)` (round half to even; webp2vips.c L513-514), then shrink is recomputed from the
     loaded size (pipeline.cc L353-355) and the final size per axis is `(int)(in/shrink + 0.5)` (resize.c L170-204).
     This can be 1 px smaller than a plain resize: e.g. 4828×408 into a 107 cap → plain 107×9, sharp 106×9. Emulate it
     (geometry only; pixels may differ as recorded) and test that case plus the three in your table.
   - GIF: no shrink-on-load in sharp (no GIF branch in pipeline.cc; nsgifload has no scale) — plain resize of the
     first frame. Check your code does that.
   - JPEG: libvips 8.18.7 rounds **down** at load: `output_width = image_width / shrink` (jpeg2vips.c L639-640). Your
     `src/image.rs:484-485` uses `div_ceil` and the comment at :492 says "ceil-loading" — that came from my earlier
     brief and is wrong. Fix and correct the dimension tests whose expected values relied on it, citing this line.
4. Remove the now-dead `inside` branch of `resize_srgb`.

Report: per item what changed and the tests, result lines. At most 250 words.
