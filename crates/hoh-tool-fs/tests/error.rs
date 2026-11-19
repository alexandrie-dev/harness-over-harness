//! Unit tests for model-facing guarded-mutation diagnostics: normalized unread failures, the
//! stale-version remedy, code preservation, and passthrough.

use hoh_error::HarnessError;
use hoh_fs::{FsError, FsErrorCode};
use hoh_tool_fs::error::remediate_fs_error;

fn cause_text(error: &HarnessError) -> Option<String> {
    error.cause().map(ToString::to_string)
}

#[test]
fn appends_the_re_read_remedy_to_stale_version_preserving_code_and_chaining_cause() {
    let original = FsError::new(
        "cannot edit \"x\": file changed since it was read",
        FsErrorCode::StaleVersion,
    );
    let remedied = remediate_fs_error(&original, "x");
    assert!(remedied.is_named(FsError::NAME));
    assert_eq!(
        remedied.message(),
        "cannot edit \"x\": file changed since it was read — re-read the file, then retry"
    );
    assert_eq!(remedied.code(), "FS_STALE_VERSION");
    assert_eq!(cause_text(&remedied), Some(original.to_string()));
}

#[test]
fn normalizes_policy_and_provider_not_observed_failures_to_one_diagnostic() {
    let sources = [
        FsError::new(
            "edit requires reading \"x\" first",
            FsErrorCode::NotObserved,
        ),
        FsError::new(
            "cannot overwrite existing \"x\" without reading it first",
            FsErrorCode::NotObserved,
        ),
    ];
    let remedied: Vec<HarnessError> = sources
        .iter()
        .map(|error| remediate_fs_error(error, "x"))
        .collect();
    let messages: Vec<&str> = remedied.iter().map(HarnessError::message).collect();
    assert_eq!(
        messages,
        [
            "cannot modify \"x\": file has not been read — read the file, then retry",
            "cannot modify \"x\": file has not been read — read the file, then retry",
        ]
    );
    assert!(
        remedied
            .iter()
            .all(|error| error.code() == "FS_NOT_OBSERVED")
    );
    let causes: Vec<Option<String>> = remedied.iter().map(cause_text).collect();
    let expected: Vec<Option<String>> = sources
        .iter()
        .map(|error| Some(error.to_string()))
        .collect();
    assert_eq!(causes, expected);
}

#[test]
fn leaves_other_fs_error_codes_untouched() {
    let original = FsError::new("no match anywhere", FsErrorCode::EditNotFound);
    let result = remediate_fs_error(&original, "x");
    assert_eq!(result.message(), "no match anywhere");
    assert_eq!(result.code(), "FS_EDIT_NOT_FOUND");
    assert!(cause_text(&result).is_none());
}

#[test]
fn leaves_non_fs_error_values_untouched() {
    let original = HarnessError::new("boom", "SOME_CODE");
    let result = remediate_fs_error(&original, "x");
    assert!(!result.is_named(FsError::NAME));
    assert_eq!(result.message(), "boom");
    assert_eq!(result.code(), "SOME_CODE");
}
