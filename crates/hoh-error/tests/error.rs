use hoh_error::{CodedError, HarnessError};
use std::error::Error;

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
struct SampleError(HarnessError);
impl CodedError for SampleError {
    fn harness_error(&self) -> &HarnessError {
        &self.0
    }
}

#[test]
fn carries_a_code_a_cause_and_the_constructing_class_name() {
    let cause = std::io::Error::other("underlying failure");
    let error = SampleError(
        HarnessError::new("operation failed", "SAMPLE_FAILED")
            .with_name("SampleError")
            .with_cause(cause),
    );
    let routed: &dyn CodedError = &error;
    assert_eq!(routed.name(), "SampleError");
    assert_eq!(routed.code(), "SAMPLE_FAILED");
    assert_eq!(routed.message(), "operation failed");
    assert_eq!(error.to_string(), "operation failed");
    assert_eq!(error.source().unwrap().to_string(), "underlying failure");
}

#[test]
fn names_a_direct_instance_harness_error() {
    assert_eq!(HarnessError::new("failed", "FAILED").name, "HarnessError");
}
