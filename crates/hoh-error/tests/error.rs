use hoh_error::{BoxError, HarnessError};
use std::error::Error;

struct SampleError;
impl SampleError {
    const NAME: &str = "SampleError";
    // PORTING.md §7 requires subclass constructor namespaces returning HarnessError.
    #[allow(clippy::new_ret_no_self)]
    fn new(message: &str, code: &str) -> HarnessError {
        HarnessError::named(Self::NAME, message, code)
    }
}
#[test]
fn carries_a_code_a_cause_and_the_constructing_class_name() {
    let error = SampleError::new("operation failed", "SAMPLE_FAILED").with_cause(
        std::io::Error::new(std::io::ErrorKind::NotFound, "underlying failure"),
    );
    assert_eq!(error.name(), "SampleError");
    assert_eq!(error.code(), "SAMPLE_FAILED");
    assert_eq!(error.message(), "operation failed");
    assert_eq!(error.to_string(), "operation failed");
    let cause = error
        .source()
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .unwrap();
    assert_eq!(cause.kind(), std::io::ErrorKind::NotFound);
    assert!(std::ptr::eq(
        cause,
        error
            .cause()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
    ));
}
#[test]
fn names_a_direct_instance_harness_error() {
    assert_eq!(HarnessError::new("failed", "FAILED").name(), "HarnessError");
}
#[test]
fn recognizes_only_the_dynamic_error_itself() {
    let error = HarnessError::new("failed", "FAILED");
    assert!(std::ptr::eq(HarnessError::of(&error).unwrap(), &error));
    let boxed: BoxError = Box::new(error);
    assert_eq!(HarnessError::of(boxed.as_ref()).unwrap().code(), "FAILED");
    #[derive(Debug, thiserror::Error)]
    #[error("plain wrapper")]
    struct PlainError {
        #[source]
        source: HarnessError,
    }
    let plain = PlainError {
        source: HarnessError::new("cause", "CAUSE"),
    };
    assert!(HarnessError::of(&plain).is_none());
    assert!(HarnessError::of(plain.source().unwrap()).is_some());
}
#[test]
fn recognizes_subclass_names() {
    let error = SampleError::new("failed", "FAILED");
    assert!(error.is_named(SampleError::NAME));
    assert!(!error.is_named("OtherError"));
    assert!(!error.is_named("HarnessError"));
}
#[derive(Debug, PartialEq)]
struct Detail {
    path: String,
}
#[test]
fn retrieves_present_detail_and_rejects_absent_or_wrong_type() {
    let error = HarnessError::new("failed", "FAILED");
    assert!(error.detail::<Detail>().is_none());
    let error = error.with_detail(Detail {
        path: "archive".into(),
    });
    assert_eq!(error.detail::<Detail>().unwrap().path, "archive");
    assert!(error.detail::<String>().is_none());
}
#[test]
fn accepts_an_already_boxed_cause_without_adding_a_wrapper() {
    let cause: BoxError = Box::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    let error = HarnessError::new("failed", "FAILED").with_cause(cause);
    assert_eq!(
        error
            .cause()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
}
#[test]
fn clone_shares_the_concrete_cause_and_detail() {
    let error = SampleError::new("failed", "FAILED")
        .with_cause(std::io::Error::other("cause"))
        .with_detail(Detail {
            path: "archive".into(),
        });
    let clone = error.clone();
    assert!(std::ptr::eq(error.cause().unwrap(), clone.cause().unwrap()));
    assert!(std::ptr::eq(
        error.detail::<Detail>().unwrap(),
        clone.detail::<Detail>().unwrap()
    ));
    drop(error);
    assert_eq!(clone.name(), SampleError::NAME);
    assert_eq!(clone.code(), "FAILED");
    assert_eq!(clone.message(), "failed");
    assert!(
        clone
            .cause()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .is_some()
    );
    assert_eq!(clone.detail::<Detail>().unwrap().path, "archive");
}
