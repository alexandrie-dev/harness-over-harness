//! Port of credentials-local/tests/migration.spec.ts: one-shot boot upgrade of the pre-release flat
//! layout. A key stored by an earlier build must survive the versioned-document change without a
//! hand edit, byte for byte, while everything the recognizer cannot prove flat keeps the loud
//! rejection local.rs exercises.
mod support;

use hoh_atomic_write::{
    FileLockOptions, WriteFileAtomicOptions, with_file_lock, write_file_atomic,
};
use hoh_credentials_local::render_flat_layout_migration;
use support::*;

async fn seed(path: &std::path::Path, text: &str) {
    write_file_atomic(
        path,
        text,
        WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .unwrap();
}

// Every spelling an earlier build accepted: plain, quoted, block-scalar, a comment header, an
// interior blank line, and a key that happens to spell a section name of the versioned layout.
const FLAT: &str = "# keys stored before the versioned layout\nDSH_CRED_TEST: stored\n\n# annotates the quoted entry\nDSH_CRED_OTHER: 'quoted value'\nDSH_CRED_BLOCK: |\n  first line\n  second line\nrecords: tricky\n";

const MIGRATED: &str = "version: 1\nrefs:\n  # keys stored before the versioned layout\n  DSH_CRED_TEST: stored\n\n  # annotates the quoted entry\n  DSH_CRED_OTHER: 'quoted value'\n  DSH_CRED_BLOCK: |\n    first line\n    second line\n  records: tricky\n";

#[tokio::test]
async fn upgrades_the_flat_document_in_place_byte_for_byte_and_serves_its_keys() {
    let dir = temp_dir();
    let path = doc(&dir);
    seed(&path, FLAT).await;
    let booted = boot(config(&path)).await;
    assert_eq!(read(&path), MIGRATED);
    assert_eq!(mode(&path), 0o600);
    let credentials = booted.credentials();
    let value = |name: &'static str| {
        let credentials = credentials.clone();
        async move { resolved(&*credentials, name).await }
    };
    assert_eq!(
        value("DSH_CRED_TEST").await,
        Some(("stored".into(), "file".into()))
    );
    assert_eq!(
        value("DSH_CRED_OTHER").await,
        Some(("quoted value".into(), "file".into()))
    );
    assert_eq!(
        value("DSH_CRED_BLOCK").await,
        Some(("first line\nsecond line\n".into(), "file".into()))
    );
    assert_eq!(
        value("records").await,
        Some(("tricky".into(), "file".into()))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn a_second_boot_reads_the_migrated_document_without_touching_it() {
    let dir = temp_dir();
    let path = doc(&dir);
    seed(&path, FLAT).await;
    boot(config(&path)).await.shutdown().await;
    let booted = boot(config(&path)).await;
    assert_eq!(read(&path), MIGRATED);
    assert_eq!(
        resolved(&*booted.credentials(), "DSH_CRED_TEST").await,
        Some(("stored".into(), "file".into()))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn yields_to_a_concurrent_migrator_under_the_writer_lock() {
    let dir = temp_dir();
    let path = doc(&dir);
    seed(&path, FLAT).await;
    let winner = "version: 1\nrefs:\n  DSH_CRED_TEST: winner\n";
    let (release, held) = tokio::sync::oneshot::channel::<()>();
    let (acquired, holding) = tokio::sync::oneshot::channel::<()>();
    let lock_path = path.clone();
    let holder = tokio::spawn(async move {
        with_file_lock(
            &lock_path,
            || async move {
                let _ = acquired.send(());
                let _ = held.await;
                Ok(())
            },
            FileLockOptions::default(),
        )
        .await
    });
    holding.await.unwrap();
    // The boot sees the flat text, then waits for the lock; the "other process" completes the
    // migration in the meantime.
    let booting = tokio::spawn({
        let config = config(&path);
        async move { try_boot(None, config).await }
    });
    pause(100).await;
    seed(&path, winner).await;
    release.send(()).unwrap();
    holder.await.unwrap().unwrap();
    let booted = booting.await.unwrap().unwrap();
    assert_eq!(read(&path), winner);
    assert_eq!(
        resolved(&*booted.credentials(), "DSH_CRED_TEST").await,
        Some(("winner".into(), "file".into()))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn leaves_an_empty_flow_mapping_alone() {
    let dir = temp_dir();
    let path = doc(&dir);
    seed(&path, "{}\n").await;
    let booted = boot(config(&path)).await;
    assert_eq!(read(&path), "{}\n");
    assert_eq!(
        resolved(&*booted.credentials(), "DSH_CRED_TEST").await,
        None
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn leaves_a_comment_only_document_alone() {
    let dir = temp_dir();
    let path = doc(&dir);
    seed(&path, "# nothing stored yet\n").await;
    let booted = boot(config(&path)).await;
    assert_eq!(read(&path), "# nothing stored yet\n");
    assert_eq!(
        resolved(&*booted.credentials(), "DSH_CRED_TEST").await,
        None
    );
    booted.shutdown().await;
}

#[test]
fn renders_a_final_newline_for_a_document_that_lacks_one() {
    assert_eq!(
        render_flat_layout_migration("DSH_CRED_TEST: bare").as_deref(),
        Some("version: 1\nrefs:\n  DSH_CRED_TEST: bare\n")
    );
}
