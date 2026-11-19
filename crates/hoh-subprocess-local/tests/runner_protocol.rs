use hoh_subprocess::SubprocessControlMode;
use hoh_subprocess_local::{
    control_spawn::control_environment, runner_launch::*, runner_protocol::*,
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
fn request() -> LinuxLaunchRequest {
    LinuxLaunchRequest {
        cwd: "/target".into(),
        env: [("A".into(), "1".into())].into(),
        pipe_fds: None,
        control: None,
        control_path: None,
    }
}
#[test]
fn creates_consumes_reports_through_and_cleans_one_private_linux_exchange() {
    let files = create_linux_launch_files(&request()).unwrap();
    assert_eq!(
        std::fs::metadata(&files.directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&files.request_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        linux_launch_files_from_locator(files.request_path.to_str().unwrap())
            .unwrap()
            .directory,
        files.directory
    );
    assert_eq!(
        serde_json::to_value(consume_linux_launch_request(&files.request_path).unwrap()).unwrap(),
        json!({"cwd":"/target","env":{"A":"1"}})
    );
    assert!(!files.request_path.exists());
    let serialized = SerializedRunnerError {
        name: "SpawnError".into(),
        message: "spawn missing".into(),
        code: Some("ENOENT".into()),
        syscall: Some("spawn tool".into()),
        path: Some("tool".into()),
    };
    write_linux_startup_error(
        &files,
        &LinuxStartupError::Error {
            error: serialized.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(&files.startup_error_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let result = read_linux_startup_error(&files.startup_error_path)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"type":"error","error":{"name":"SpawnError","message":"spawn missing","code":"ENOENT","syscall":"spawn tool","path":"tool"}})
    );
    assert_eq!(
        deserialize_runner_error(serialized).to_string(),
        "spawn missing"
    );
    cleanup_linux_launch_files(&files);
    assert!(!files.directory.exists());
}
#[test]
fn preserves_the_explicit_control_request_and_rejects_other_transport_values() {
    let mut request = request();
    request.control = Some(SubprocessControlMode::Pipe);
    let files = create_linux_launch_files(&request).unwrap();
    let control_directory = files.control_directory.clone().unwrap();
    let control_path = files.control_path.clone().unwrap();
    assert_eq!(
        std::fs::metadata(&control_directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(control_path.as_os_str().as_encoded_bytes().len() < 108);
    let consumed = consume_linux_launch_request(&files.request_path).unwrap();
    assert_eq!(consumed.control, Some(SubprocessControlMode::Pipe));
    assert_eq!(
        consumed.control_path.as_deref(),
        Some(control_path.to_str().unwrap())
    );
    for value in [json!("bad"), json!(null), json!(true)] {
        std::fs::write(
            &files.request_path,
            format!("{{\"cwd\":\"/\",\"env\":{{}},\"control\":{value}}}"),
        )
        .unwrap();
        assert!(
            consume_linux_launch_request(&files.request_path)
                .unwrap_err()
                .to_string()
                .contains("invalid Linux launch request")
        );
        assert!(!files.request_path.exists());
    }
    cleanup_linux_launch_files(&files);
    assert!(!control_directory.exists());
}
#[test]
fn strictly_rejects_malformed_linux_messages() {
    let files = create_linux_launch_files(&request()).unwrap();
    for value in [
        json!({"cwd":"/ok","env":{},"extra":true}),
        json!({"cwd":"/ok","env":{},"pipeFds":[3]}),
        json!({"cwd":"/ok","env":{},"pipeFds":null}),
        json!({"cwd":"/ok","env":{"x":null}}),
        json!({"cwd":null,"env":{}}),
    ] {
        std::fs::write(&files.request_path, value.to_string()).unwrap();
        assert!(consume_linux_launch_request(&files.request_path).is_err());
        assert!(!files.request_path.exists());
    }
    assert!(
        linux_launch_files_from_locator("relative.json")
            .unwrap_err()
            .to_string()
            .contains("invalid Linux launch-request locator")
    );
    assert!(
        read_linux_startup_error(&files.startup_error_path)
            .unwrap()
            .is_none()
    );
    for (value, message) in [
        (json!(null), "invalid startup error"),
        (
            json!({"type":"unknown","error":{"name":"Error","message":"bad"}}),
            "unknown error result",
        ),
        (
            json!({"type":7,"error":{"name":"Error","message":"bad"}}),
            "unknown error result",
        ),
        (
            json!({"type":"error","error":{"name":"Error","message":"bad","code":null}}),
            "invalid error result",
        ),
    ] {
        std::fs::write(&files.startup_error_path, value.to_string()).unwrap();
        assert!(
            read_linux_startup_error(&files.startup_error_path)
                .unwrap_err()
                .to_string()
                .contains(message)
        );
    }
    cleanup_linux_launch_files(&files);
}
#[test]
fn contains_cleanup_failures_and_removes_a_substituted_symlink_only() {
    let files = create_linux_launch_files(&request()).unwrap();
    cleanup_linux_launch_files(&files);
    cleanup_linux_launch_files(&files);
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let link = temp.path().join("link");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    cleanup_linux_launch_files(&LinuxLaunchFiles {
        request_path: link.join("launch-request.json"),
        startup_error_path: link.join("startup-error.json"),
        directory: link.clone(),
        control_directory: None,
        control_path: None,
    });
    assert!(!link.exists());
    assert!(target.exists());
    let blocked = create_linux_launch_files(&request()).unwrap();
    std::fs::remove_file(&blocked.request_path).unwrap();
    std::fs::create_dir(&blocked.request_path).unwrap();
    cleanup_linux_launch_files(&blocked);
    assert!(blocked.directory.exists());
    std::fs::remove_dir_all(&blocked.directory).unwrap();
}
#[test]
fn keeps_target_state_out_of_the_bootstrap_environment_and_consumes_its_selector() {
    let mut environment = runner_environment("locator", None);
    assert_eq!(
        environment.get(SUBPROCESS_RUNNER_ENV).map(String::as_str),
        Some("locator")
    );
    assert_eq!(
        consume_runner_selection(&mut environment).as_deref(),
        Some("locator")
    );
    assert!(!environment.contains_key(SUBPROCESS_RUNNER_ENV));
    assert!(!environment.keys().any(
        |key| key.to_uppercase().starts_with("NODE_") || key.to_uppercase().starts_with("TSX_")
    ));
    assert!(parse_runner_target_argv(&["target".into()]).is_err());
    assert_eq!(
        parse_runner_target_argv(&["--".into(), "target".into(), "literal $HOME".into()]).unwrap(),
        ["target", "literal $HOME"]
    );
}
#[test]
fn serializes_only_the_private_protocol_diagnostic_fields() {
    let error = deserialize_runner_error(SerializedRunnerError {
        name: "Error".into(),
        message: "detailed".into(),
        code: Some("ENOENT".into()),
        syscall: Some("spawn tool".into()),
        path: Some("tool".into()),
    });
    assert_eq!(
        serde_json::to_value(serialize_runner_error(error.as_ref())).unwrap(),
        json!({"name":"Error","message":"detailed","code":"ENOENT","syscall":"spawn tool","path":"tool"})
    );
    let minimal = SerializedRunnerError {
        name: "Error".into(),
        message: "minimal".into(),
        code: None,
        syscall: None,
        path: None,
    };
    assert_eq!(
        serde_json::to_value(&minimal).unwrap(),
        json!({"name":"Error","message":"minimal"})
    );
    assert_eq!(deserialize_runner_error(minimal).to_string(), "minimal");
}
#[test]
fn rejects_a_caller_authored_control_marker_before_starting_a_child() {
    for key in ["DSH_SUBPROCESS_CONTROL", "dsh_subprocess_control"] {
        assert!(
            control_environment([(key.into(), Some("pipe".into()))].into(), false)
                .unwrap_err()
                .to_string()
                .contains("reserved")
        );
    }
    assert_eq!(
        control_environment(Default::default(), true)
            .unwrap()
            .get("DSH_SUBPROCESS_CONTROL"),
        Some(&Some("pipe".into()))
    );
    assert!(control_environment([("DSH_SUBPROCESS_CONTROL".into(), None)].into(), false).is_ok());
}

#[test]
fn consumes_utf8_requests_with_node_replacement_semantics() {
    let files = create_linux_launch_files(&request()).unwrap();
    std::fs::write(
        &files.request_path,
        b"{\"cwd\":\"/work\",\"env\":{\"A\":\"\xff\"}}",
    )
    .unwrap();
    let request = consume_linux_launch_request(&files.request_path).unwrap();
    assert_eq!(request.env["A"], "\u{fffd}");
    cleanup_linux_launch_files(&files);
}

#[test]
fn derives_locator_paths_with_node_dirname_and_join_semantics() {
    let files = linux_launch_files_from_locator("/x/./launch-request.json").unwrap();
    assert_eq!(files.directory.to_str(), Some("/x/."));
    assert_eq!(
        files.request_path.to_str(),
        Some("/x/./launch-request.json")
    );
    assert_eq!(
        files.startup_error_path.to_str(),
        Some("/x/startup-error.json")
    );
}

#[test]
fn serializes_syntax_errors_and_empty_error_names_without_changing_their_base_class() {
    let failure = hoh_util_values::js_json_parse("{").unwrap_err();
    let record = serialize_runner_error(&failure);
    assert_eq!(record.name, "SyntaxError");
    assert_eq!(record.message, failure.to_string());
    assert!(record.code.is_none());
    let record = SerializedRunnerError {
        name: String::new(),
        message: "message".into(),
        code: None,
        syscall: None,
        path: None,
    };
    let failure = deserialize_runner_error(record);
    assert!(hoh_error::HarnessError::of(failure.as_ref()).is_none());
    assert_eq!(serialize_runner_error(failure.as_ref()).name, "");
    let controller = hoh_timeout::AbortController::new();
    controller.abort_with(failure);
    let mut request = hoh_subprocess::SubprocessSpawnSpec {
        argv: vec!["/bin/true".into()],
        cwd: "/tmp".into(),
        env: None,
        signal: Some(controller.signal()),
        grace_ms: 10.0,
        stdio: hoh_subprocess::SubprocessStdio {
            stdin: hoh_subprocess::SubprocessStdinMode::Ignore,
            stdout: hoh_subprocess::SubprocessOutputMode::Inherit,
            stderr: hoh_subprocess::SubprocessOutputMode::Inherit,
            control: None,
        },
    };
    assert_eq!(
        hoh_subprocess_local::spawn::validate_subprocess_spec(&request)
            .unwrap_err()
            .to_string(),
        "aborted before spawn: message"
    );
    request.signal = None;
    assert!(hoh_subprocess_local::spawn::validate_subprocess_spec(&request).is_ok());
}
