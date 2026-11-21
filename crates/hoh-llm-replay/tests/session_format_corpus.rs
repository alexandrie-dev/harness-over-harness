//! The committed Session corpus of the source inventory: every generation is restored through the
//! session format catalog, except the exact historical refusals the inventory lists.
use hoh_llm_replay::parse_session_log;
use std::{collections::HashMap, path::Path};
const REFS: &str = "/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness";
fn inventory() -> String {
    std::fs::read_to_string(
        Path::new(REFS)
            .join("packages/test-support/llm-replay/tests/session-format-corpus-inventory.ts"),
    )
    .unwrap()
}
fn listed_keys(source: &str) -> Vec<String> {
    regex::Regex::new(r"(?m)^  '([^']+)'")
        .unwrap()
        .captures_iter(source)
        .map(|c| c[1].to_owned())
        .collect()
}
/// `expectedUnsupported`: fixture key -> (source version, exact reason).
fn expected_unsupported(source: &str) -> HashMap<String, (u64, String)> {
    let table = source
        .split("export const unversionedProtocolFixtures")
        .next()
        .unwrap();
    regex::Regex::new(r"(?s)'([^']+)': \{\s*sourceVersion: ([0-9]+),\s*reason: '([^']*)',")
        .unwrap()
        .captures_iter(table)
        .map(|c| (c[1].to_owned(), (c[2].parse().unwrap(), c[3].to_owned())))
        .collect()
}
/// Committed fixtures, listed by git (the corpus lives on a slow disk: no directory walk).
fn fixtures() -> Vec<String> {
    let out = std::process::Command::new("git")
        .args([
            "-C",
            REFS,
            "ls-files",
            "-s",
            "--",
            "snapshots",
            "packages",
            "scripts/snapshots/python-sdk-single-exe",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let canonical =
        regex::Regex::new(r"^session(?:\.[1-9][0-9]*)?(?:\.v[1-9][0-9]*)?\.jsonl$").unwrap();
    let excluded = ["dist", "lib", "node_modules"];
    // `-s` lists the mode: tracked symlinks (120000) are skipped, as the source skips them.
    let mut files: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(info, _)| !info.starts_with("120000"))
        .map(|(_, path)| path)
        .filter(|path| {
            let mut parts = path.split('/');
            let name = path.rsplit('/').next().unwrap();
            name.starts_with("session")
                && name.ends_with(".jsonl")
                && !parts.any(|p| excluded.contains(&p))
        })
        .map(str::to_owned)
        .collect();
    for path in &files {
        assert!(
            canonical.is_match(path.rsplit('/').next().unwrap()),
            "{path}"
        );
    }
    files.sort();
    files
}
#[test]
fn restores_the_whole_corpus_through_the_format_catalog_except_the_listed_refusals() {
    let inventory = inventory();
    let unsupported = expected_unsupported(&inventory);
    let protocol = listed_keys(
        inventory
            .split("export const unversionedProtocolFixtures")
            .nth(1)
            .unwrap(),
    );
    let canonical =
        regex::Regex::new(r"^session(?:\.[1-9][0-9]*)?(?:\.v([1-9][0-9]*))?\.jsonl$").unwrap();
    let (mut restored, mut refused, mut historical) = (0, 0, 0);
    for key in fixtures() {
        let file = Path::new(REFS).join(&key);
        let bytes = std::fs::read(&file).unwrap();
        let source = std::str::from_utf8(&bytes).unwrap();
        let header = source
            .lines()
            .find(|s| !hoh_util_values::js_trim(s).is_empty())
            .map(hoh_util_values::js_json_parse)
            .transpose()
            .unwrap()
            .unwrap_or_default();
        let name = key.rsplit('/').next().unwrap();
        let filename_version = canonical
            .captures(name)
            .unwrap()
            .get(1)
            .map(|m| m.as_str().parse::<u64>().unwrap())
            .unwrap_or(0);
        if protocol.contains(&key) {
            assert!(
                header.get("version").is_none(),
                "{key}: must remain unversioned"
            );
            assert_eq!(filename_version, 0, "{key}: protocol fixture filename");
            assert!(!unsupported.contains_key(&key), "{key}");
            continue;
        }
        let version = header["version"]
            .as_u64()
            .unwrap_or_else(|| panic!("{key}: Session header must declare its format"));
        assert_eq!(filename_version, version, "{key}: filename/header mismatch");
        let result = parse_session_log(source).map(drop);
        let shown = result.as_ref().err().map(ToString::to_string);
        assert_restoration(
            version,
            result,
            unsupported.get(&key).map(|(v, r)| (*v, r.as_str())),
        )
        .unwrap_or_else(|why| panic!("{key}: {why}: {shown:?}"));
        if unsupported.contains_key(&key) {
            refused += 1;
        } else {
            restored += 1;
        }
        if version < hoh_session::SESSION_FORMAT_VERSION {
            historical += 1;
        }
        assert_eq!(
            std::fs::read(&file).unwrap(),
            bytes,
            "{key}: source changed"
        );
    }
    assert_eq!(refused, unsupported.len());
    assert!(restored > 200, "corpus coverage disappeared: {restored}");
    assert!(historical > 0, "historical inventory unexpectedly empty");
}
#[test]
fn keeps_every_exception_tied_to_an_existing_fixture() {
    let inventory = inventory();
    let keys: std::collections::HashSet<_> = fixtures().into_iter().collect();
    let mut listed: std::collections::HashSet<_> = listed_keys(&inventory).into_iter().collect();
    listed.extend(expected_unsupported(&inventory).into_keys());
    assert_eq!(listed.len(), 25);
    for key in listed {
        assert!(keys.contains(&key), "{key}");
    }
}
type Refusal<'a> = (u64, &'a str);
fn assert_restoration(
    source: u64,
    result: hoh_llm_replay::Result<()>,
    expected: Option<Refusal<'_>>,
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
