mod support;
use hoh_loader::*;
use std::path::{Path, PathBuf};
use support::*;
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dsh")
}
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for file in std::fs::read_dir(dir).unwrap() {
        let path = file.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|s| s == "yml") {
            out.push(path);
        }
    }
}
#[test]
fn every_shipped_bundle_and_preset_reads_unchanged_and_every_expression_parses() {
    let mut paths = Vec::new();
    files(&fixture().join("packages/bundle"), &mut paths);
    assert!(paths.len() >= 6);
    for path in paths {
        let text = std::fs::read_to_string(&path).unwrap();
        parse_patches(&text, &path).unwrap_or_else(|e| panic!("{e}"));
    }
}
#[test]
fn layers_of_the_real_headless_profile_compose_without_l10_warnings() {
    // packages/util/official-bundles/src/bundles.ts declares this exact order.
    // app-boot/profile.ts composes over an empty root (no checked-in profile base).
    let root = tempfile::tempdir().unwrap();
    let names = ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-headless"];
    let dir = write_profile(root.path(), &names);
    let source = Source(
        names
            .iter()
            .zip(["base", "headless"])
            .map(|(name, folder)| {
                let dir = fixture().join("packages/bundle").join(folder);
                Bundle {
                    name: (*name).into(),
                    files: vec![(
                        "cordis.patch.yml".into(),
                        std::fs::read_to_string(dir.join("cordis.patch.yml")).unwrap(),
                    )],
                    dir: Some(dir),
                    platforms: None,
                }
            })
            .collect(),
    );
    let profile = Profile::load(
        &dir,
        &source,
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert!(profile.skipped.is_empty(), "{:?}", profile.skipped);
    let composition = compose(&[], &profile.layers, &mut |w| panic!("{w}"));
    assert!(!composition.entries.is_empty());
    assert!(
        composition
            .entries
            .iter()
            .any(|e| e.id == "session-telemetry-otel")
    );
    let dump = render_dump(&composition);
    assert_eq!(
        parse_entries(&dump, &root.path().join("dump.yml")).unwrap(),
        composition.entries
    );
}
#[test]
fn every_snapshot_entry_or_patch_document_reads_unchanged() {
    let mut paths = Vec::new();
    files(&fixture().join("snapshots"), &mut paths);
    let mut checked = 0;
    for path in paths {
        if matches!(
            path.file_name().and_then(|s| s.to_str()),
            Some("cordis.yml" | "cordis.snapshot.yml")
        ) {
            let text = std::fs::read_to_string(&path).unwrap();
            // Replay documents sometimes are overlay patches, despite the cordis basename.
            if let Err(entry_error) = parse_entries(&text, &path) {
                parse_patches(&text, &path).unwrap_or_else(|patch_error| {
                    panic!("entries: {entry_error}; patches: {patch_error}")
                });
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 244);
}
