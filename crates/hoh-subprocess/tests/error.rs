use hoh_error::{HarnessError, SharedError};
use hoh_subprocess::*;
use serde_json::{Value, json};
use std::{io, sync::Arc};

fn foreign(message: &str, code: Value) -> SharedError {
    SharedError::new(SubprocessForeignError::Coded {
        message: message.into(),
        code,
    })
}

fn classified(original: &SharedError, program: &str) -> SharedError {
    let SubprocessFailure::Error(error) =
        subprocess_spawn_failure(original.clone().into(), program)
    else {
        panic!("expected error");
    };
    error
}

fn assert_cause(error: &HarnessError, original: &SharedError) {
    assert!(
        error
            .cause()
            .unwrap()
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(original)
    );
}

#[test]
fn carries_a_literal_name_and_a_code_from_the_closed_list() {
    let cause = SharedError::new(io::Error::other("low level"));
    let error = SubprocessError::new("runner failed", SubprocessErrorCode::RunnerFailed)
        .with_cause(cause.clone());
    assert!(HarnessError::of(&error).is_some());
    assert_eq!(error.name(), "SubprocessError");
    assert_eq!(error.code(), "SUBPROCESS_RUNNER_FAILED");
    assert_eq!(error.message(), "runner failed");
    assert!(SUBPROCESS_ERROR_CODES.contains(&error.code()));
    assert_cause(&error, &cause);
}

#[test]
fn keeps_the_executable_lookup_miss_as_its_own_subclass() {
    let error = SubprocessExecutableNotFoundError::new("not on PATH");
    assert!(SubprocessError::is(&error));
    assert_eq!(error.name(), "SubprocessExecutableNotFoundError");
    assert_eq!(error.code(), "SUBPROCESS_EXECUTABLE_NOT_FOUND");
    assert_eq!(error.message(), "not on PATH");
    assert!(!SubprocessError::is(&HarnessError::new(
        "not a subclass",
        error.code()
    )));
}

#[test]
fn classifies_enoent_and_other_spawn_rejections_keeping_the_original_as_cause() {
    let missing = foreign("spawn pwsh ENOENT", json!("ENOENT"));
    assert!(HarnessError::of(missing.get_ref()).is_none());
    let wrapped = classified(&missing, "pwsh");
    let error = HarnessError::of(wrapped.get_ref()).unwrap();
    assert_eq!(error.name(), "SubprocessError");
    assert_eq!(error.code(), "SUBPROCESS_SPAWN_NOT_FOUND");
    assert_eq!(
        error.message(),
        "cannot start \"pwsh\": the program or the working directory does not exist (spawn pwsh ENOENT)"
    );
    assert_cause(error, &missing);
    let denied = foreign("spawn bash EACCES", json!("EACCES"));
    let wrapped = classified(&denied, "bash");
    let error = HarnessError::of(wrapped.get_ref()).unwrap();
    assert_eq!(error.code(), "SUBPROCESS_SPAWN_FAILED");
    assert_eq!(error.message(), "cannot start \"bash\": spawn bash EACCES");
    assert_cause(error, &denied);
}

#[test]
fn returns_values_without_an_errno_code_unchanged() {
    for error in [
        SharedError::new(io::Error::other(
            "subprocess implementation dropped a requested collect stream",
        )),
        foreign("exit", json!(1)),
        foreign("not an errno", Value::Null),
    ] {
        assert!(classified(&error, "bash").ptr_eq(&error));
    }
    for value in [
        json!("raw"),
        json!({"code": "ENOENT", "message": "not an Error"}),
        Value::Null,
    ] {
        let original = Arc::new(value);
        let SubprocessFailure::Value(returned) =
            subprocess_spawn_failure(SubprocessFailure::Value(original.clone()), "bash")
        else {
            panic!("expected unchanged value")
        };
        assert!(Arc::ptr_eq(&returned, &original));
    }
    assert!(matches!(
        subprocess_spawn_failure(SubprocessFailure::Undefined, "bash"),
        SubprocessFailure::Undefined
    ));
}

#[test]
fn returns_coded_errors_and_cancellations_unchanged() {
    for error in [
        SharedError::new(HarnessError::new(
            "sandbox unavailable",
            "SANDBOX_UNAVAILABLE",
        )),
        SharedError::new(HarnessError::new("not a platform error", "ENOENT")),
        SharedError::new(SubprocessExecutableNotFoundError::new("missing")),
        SharedError::new(hoh_timeout::AbortError::new("aborted")),
    ] {
        assert!(classified(&error, "bash").ptr_eq(&error));
    }
}

#[test]
fn quotes_the_program_like_javascript_without_rewriting_the_provider_message() {
    let error = foreign("provider detail", json!("EACCES"));
    let wrapped = classified(&error, "a\"b\n\\программа");
    assert_eq!(
        wrapped.to_string(),
        "cannot start \"a\\\"b\\n\\\\программа\": provider detail"
    );
    assert_cause(HarnessError::of(wrapped.get_ref()).unwrap(), &error);
}

#[cfg(unix)]
#[test]
fn formats_native_start_failures_through_the_shared_node_helpers() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing-program");
    let non_executable = directory.path().join("non-executable");
    std::fs::write(&non_executable, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&non_executable, std::fs::Permissions::from_mode(0o644)).unwrap();
    let not_directory = non_executable.join("program");
    for (path, errno, detail_includes_program) in [
        (missing, "ENOENT", true),
        (non_executable, "EACCES", true),
        (not_directory, "ENOTDIR", false),
    ] {
        let program = path.to_str().unwrap();
        let native = std::process::Command::new(program).spawn().unwrap_err();
        assert_eq!(hoh_error::node_error_code(&native), Some(errno));
        let original = SharedError::new(native);
        let wrapped = classified(&original, program);
        let error = HarnessError::of(wrapped.get_ref()).unwrap();
        let quoted = hoh_util_values::js_json_stringify(&Value::String(program.to_owned()));
        let detail = if detail_includes_program {
            format!("spawn {program} {errno}")
        } else {
            format!("spawn {errno}")
        };
        let expected = if errno == "ENOENT" {
            format!(
                "cannot start {quoted}: the program or the working directory does not exist ({detail})"
            )
        } else {
            format!("cannot start {quoted}: {detail}")
        };
        assert_eq!(error.message(), expected);
        assert_cause(error, &original);
    }
}
