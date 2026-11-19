//! Current-generation portion of the source inventory. Historical catalog policy remains blocked
//! on the absent owner crate; refusing old generations is tested separately, never counted as restore.
use hoh_llm_replay::parse_session_log;
use std::path::{Path, PathBuf};
fn inventory() -> String {
    std::fs::read_to_string("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/test-support/llm-replay/tests/session-format-corpus-inventory.ts").unwrap()
}
fn listed_keys(source: &str) -> Vec<String> {
    regex::Regex::new(r"(?m)^  '([^']+)'")
        .unwrap()
        .captures_iter(source)
        .map(|c| c[1].to_owned())
        .collect()
}
fn fixtures(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_owned()];
    let mut files = vec![];
    let canonical =
        regex::Regex::new(r"^session(?:\.[1-9][0-9]*)?(?:\.v[1-9][0-9]*)?\.jsonl$").unwrap();
    while let Some(dir) = pending.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let kind = entry.file_type().unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !["dist", "lib", "node_modules"].contains(&name.as_str()) {
                    pending.push(entry.path());
                }
            } else if name.starts_with("session") && name.ends_with(".jsonl") {
                assert!(canonical.is_match(&name), "{}", entry.path().display());
                files.push(entry.path());
            }
        }
    }
    files.sort();
    files
}
#[test]
fn restores_committed_current_generation_fixtures_without_changing_source_bytes() {
    let refs = Path::new("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness");
    let mut current = 0;
    let mut historical_pending = 0;
    let inventory = inventory();
    let protocol = listed_keys(
        inventory
            .split("export const unversionedProtocolFixtures")
            .nth(1)
            .unwrap(),
    );
    let canonical =
        regex::Regex::new(r"^session(?:\.[1-9][0-9]*)?(?:\.v([1-9][0-9]*))?\.jsonl$").unwrap();
    for root in [
        "snapshots",
        "packages",
        "scripts/snapshots/python-sdk-single-exe",
    ] {
        for file in fixtures(&refs.join(root)) {
            let bytes = std::fs::read(&file).unwrap();
            let source = std::str::from_utf8(&bytes).unwrap();
            let header = source
                .lines()
                .find(|s| !hoh_util_values::js_trim(s).is_empty())
                .map(hoh_util_values::js_json_parse)
                .transpose()
                .unwrap()
                .unwrap_or_default();
            if header.get("version").is_none() {
                assert!(
                    protocol.iter().any(|key| file == refs.join(key)),
                    "unlisted headerless fixture: {}",
                    file.display()
                );
                assert_eq!(std::fs::read(&file).unwrap(), bytes);
                continue;
            }
            let version = header["version"].as_u64().unwrap();
            let name = file.file_name().unwrap().to_str().unwrap();
            let filename_version = canonical
                .captures(name)
                .unwrap()
                .get(1)
                .map(|m| m.as_str().parse::<u64>().unwrap())
                .unwrap_or(0);
            assert_eq!(
                filename_version,
                version,
                "filename/header mismatch: {}",
                file.display()
            );
            if version == hoh_session::SESSION_FORMAT_VERSION {
                parse_session_log(source).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
                current += 1;
            } else {
                assert!(version < hoh_session::SESSION_FORMAT_VERSION);
                historical_pending += 1;
            }
            assert_eq!(
                std::fs::read(&file).unwrap(),
                bytes,
                "source changed: {}",
                file.display()
            );
        }
    }
    assert!(
        current > 200,
        "current fixture coverage disappeared: {current}"
    );
    assert!(
        historical_pending > 0,
        "historical inventory unexpectedly empty"
    );
}
#[test]
fn keeps_every_exception_tied_to_an_existing_fixture() {
    let refs = Path::new("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness");
    let keys = listed_keys(&inventory());
    assert_eq!(keys.len(), 25);
    for key in keys {
        assert!(refs.join(&key).is_file(), "{key}");
    }
}
type Refusal = (u64, &'static str);
fn assert_restoration(
    source: u64,
    result: hoh_llm_replay::Result<()>,
    expected: Option<Refusal>,
) -> std::result::Result<(), &'static str> {
    let Some((version, reason)) = expected else {
        return result.map_err(|_| "current-format restoration");
    };
    if source >= hoh_session::SESSION_FORMAT_VERSION {
        return Err("current-generation fixtures cannot be unsupported");
    }
    if source != version {
        return Err("inventoried source generation");
    }
    let failure = result.err().ok_or("expected unsupported migration")?;
    if !matches!(
        failure.downcast_ref::<hoh_llm_replay::ReplayError>(),
        Some(hoh_llm_replay::ReplayError::UnsupportedMigration { .. })
    ) {
        return Err("expected unsupported migration");
    }
    if failure.to_string() != reason {
        return Err("exact refusal reason");
    }
    Ok(())
}
fn refused() -> hoh_llm_replay::Result<()> {
    Err(Box::new(
        hoh_llm_replay::ReplayError::UnsupportedMigration {
            message: "deliberate historical refusal".into(),
            cause: None,
        },
    ))
}
const REFUSAL: Refusal = (2, "deliberate historical refusal");
#[test]
fn rejects_an_unlisted_migration_refusal() {
    assert_eq!(
        assert_restoration(2, refused(), None),
        Err("current-format restoration")
    );
}
#[test]
fn rejects_an_exception_that_starts_restoring_successfully() {
    assert_eq!(
        assert_restoration(2, Ok(()), Some(REFUSAL)),
        Err("expected unsupported migration")
    );
}
#[test]
fn rejects_a_changed_refusal_reason() {
    assert_eq!(
        assert_restoration(2, refused(), Some((2, "different"))),
        Err("exact refusal reason")
    );
}
#[test]
fn rejects_corruption_masquerading_as_unsupported_migration() {
    let result: hoh_llm_replay::Result<()> = Err(Box::new(hoh_llm_replay::ReplayError::Message(
        REFUSAL.1.into(),
    )));
    assert_eq!(
        assert_restoration(2, result, Some(REFUSAL)),
        Err("expected unsupported migration")
    );
}
#[test]
fn rejects_a_mismatched_source_generation() {
    assert_eq!(
        assert_restoration(1, refused(), Some(REFUSAL)),
        Err("inventoried source generation")
    );
}
#[test]
fn never_exempts_a_current_generation_fixture() {
    assert_eq!(
        assert_restoration(4, refused(), Some((4, REFUSAL.1))),
        Err("current-generation fixtures cannot be unsupported")
    );
}
#[test]
fn accepts_only_the_exact_typed_historical_refusal() {
    assert_eq!(assert_restoration(2, refused(), Some(REFUSAL)), Ok(()));
}
