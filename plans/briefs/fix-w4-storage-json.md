# w4: fix round for hoh-storage-json after review

Sonnet's read-only review of `crates/hoh-storage-json` found one data-loss bug; I confirmed its cause in tokio's
source. Each fix with a test that fails before it. Same environment as before (`target-w4`).

1. **Data loss (blocks acceptance).** `src/atomic.rs:141-152` (`replace`): `tokio::fs::File::write_all` only queues
   the write; `sync_all` swallows the write's error (tokio-1.54.1 `fs/file.rs:317-323, 1194-1204`), so on ENOSPC /
   EDQUOT / EIO / EFBIG the short temp file is renamed over the good one. Source `src/atomic.ts:29-36`: the call
   rejects, the temp file is removed, the target is untouched. Fix: `file.flush().await?` before `sync_all()`.
   Test: ignore SIGXFSZ, `setrlimit(RLIMIT_FSIZE, 64)` in a child process or a dedicated test binary (the limit is
   per process — do not let it affect other tests), `put_record("t","k", <1 KiB>)` over an existing record → error
   with the errno, old bytes intact; the same for single layout and `set_global`. `docs/porting/PORTING.md` §6 now
   states this rule.
2. **Quarantine error not a `StorageError` to one downcast** (`per_record_unit.rs:148-150`; source
   `per-record-unit.ts:286-291` throws `StorageError('unknown-state')`): a consumer doing
   `downcast_ref::<StorageError>()` gets `None` (your own test needs two steps, `per_record_batch.rs:81-90`;
   `json_backend.rs:50` uses one). Make the error a caller sees on a quarantined unit downcast to `StorageError` in one
   step, with code `unknown-state`, every time it is returned (e.g. a fresh `StorageError` per call carrying the
   original as cause, or making it `Clone` in `crates/hoh-storage` — you may edit `hoh-storage` for this only,
   keeping its tests green). Test with a single downcast.
3. **Missing tests:** crash residue (a leftover `.<uuid>.tmp` and a `*.deleting` file are ignored on open); a partial
   legacy bootstrap (record 1 written, then failure; state after reopen equals the source's — inherited behaviour,
   test documents it); `deleteRecords` over a failed publish restores removed keys in removal order.

Notes 3–5 of the review are inherited from the source (durability gaps, path traversal of legacy keys, 2^53
stamps): keep them as they are and make sure PORT_NOTES lists each.

Report: per item what changed, result lines. At most 250 words.
