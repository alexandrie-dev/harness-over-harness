use hoh_session_format::SessionFormatError;
use std::{error::Error, io};

#[test]
fn format_failure_retains_its_cause_without_changing_class_or_message() {
    let error = SessionFormatError::new_with_cause(
        "invalid embedded stream",
        io::Error::other("bad chunk"),
    );
    assert_eq!(error.to_string(), "invalid embedded stream");
    assert!(!error.is_unsupported_migration());
    assert!(
        (&error as &dyn Error)
            .downcast_ref::<SessionFormatError>()
            .is_some()
    );
    let cause = error.source().unwrap();
    assert_eq!(
        cause.downcast_ref::<io::Error>().unwrap().to_string(),
        "bad chunk"
    );
}

#[test]
fn original_constructors_and_nested_format_causes_still_work() {
    let direct = SessionFormatError::Format("old constructor".into());
    assert_eq!(direct.to_string(), "old constructor");
    assert!(direct.source().is_none());
    assert!(SessionFormatError::new("no cause").source().is_none());
    let error = SessionFormatError::new_with_cause(
        "outer",
        SessionFormatError::new_with_cause("inner", io::Error::other("root")),
    );
    let inner = error
        .source()
        .unwrap()
        .downcast_ref::<SessionFormatError>()
        .unwrap();
    assert_eq!(inner.to_string(), "inner");
    assert_eq!(
        inner
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .to_string(),
        "root"
    );
    assert!(SessionFormatError::unsupported_migration("unsupported").is_unsupported_migration());
}
