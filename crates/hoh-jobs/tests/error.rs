use hoh_error::HarnessError;
use hoh_jobs::{JOB_ERROR_CODES, JobError, JobErrorCode};
use std::io;

fn assert_job_error(code: JobErrorCode) {
    let error =
        JobError::new("job operation failed", code).with_cause(io::Error::other("registry detail"));
    assert!(error.is_named(JobError::NAME));
    assert_eq!(error.name(), "JobError");
    assert_eq!(error.code(), code.as_str());
    assert_eq!(error.message(), "job operation failed");
    assert_eq!(
        error.cause().map(ToString::to_string).as_deref(),
        Some("registry detail")
    );
    let as_error: &(dyn std::error::Error + 'static) = &error;
    assert!(HarnessError::of(as_error).is_some());
    assert!(JOB_ERROR_CODES.contains(&code));
}

#[test]
fn is_a_harness_error_named_job_error_with_code_job_controller_unavailable() {
    assert_job_error(JobErrorCode::ControllerUnavailable);
}

#[test]
fn is_a_harness_error_named_job_error_with_code_job_not_found() {
    assert_job_error(JobErrorCode::NotFound);
}

#[test]
fn is_a_harness_error_named_job_error_with_code_job_foreign_session() {
    assert_job_error(JobErrorCode::ForeignSession);
}

#[test]
fn is_a_harness_error_named_job_error_with_code_job_owner_not_live() {
    assert_job_error(JobErrorCode::OwnerNotLive);
}

#[test]
fn is_a_harness_error_named_job_error_with_code_job_owner_registry_unavailable() {
    assert_job_error(JobErrorCode::OwnerRegistryUnavailable);
}

#[test]
fn is_a_harness_error_named_job_error_with_code_job_limit_reached() {
    assert_job_error(JobErrorCode::LimitReached);
}
