use hoh_subprocess_local::{runner_launch::*, runner_protocol::*};
use indexmap::IndexMap;
use std::{
    os::unix::fs::PermissionsExt,
    process::{Command, Output, Stdio},
};

struct Exchange(LinuxLaunchFiles);
impl Drop for Exchange {
    fn drop(&mut self) {
        cleanup_linux_launch_files(&self.0);
    }
}
fn request(cwd: &str, env: IndexMap<String, String>) -> Exchange {
    Exchange(
        create_linux_launch_files(&LinuxLaunchRequest {
            cwd: cwd.into(),
            env,
            pipe_fds: Some(vec![0, 1, 2]),
            control: None,
            control_path: None,
        })
        .unwrap(),
    )
}
fn launch(files: &Exchange, argv: &[&str], cwd: Option<&std::path::Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hoh-subprocess-runner"));
    command
        .env_clear()
        .env(SUBPROCESS_RUNNER_ENV, &files.0.request_path)
        .args(argv)
        .stdin(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.output().unwrap()
}

#[test]
fn starts_the_native_launcher_as_a_session_and_process_group_leader() {
    let files = request("/tmp", Default::default());
    let output = launch(
        &files,
        &[
            "--hoh-detached-exec",
            "--",
            "/bin/sh",
            "-c",
            "printf '%s\\n' \"$$\"; /bin/ps -o pgid= -o sid= -p \"$$\"",
        ],
        None,
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    let values = String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(values.len(), 3);
    assert_eq!(values, [values[0]; 3]);
    assert!(
        files.0.request_path.exists(),
        "launcher must leave the target request untouched"
    );
}

#[test]
fn keeps_the_target_in_the_native_launchers_session_and_group() {
    let files = request("/tmp", Default::default());
    let output = launch(
        &files,
        &[
            "--hoh-detached-exec",
            "--",
            "/bin/sh",
            "-c",
            "printf '%s\\n' \"$$\"; \"$1\" -- /bin/sh -c \"$2\"; status=$?; exit \"$status\"",
            "outer",
            env!("CARGO_BIN_EXE_hoh-subprocess-runner"),
            "printf '%s\\n' \"$$\"; /bin/ps -o pgid= -o sid= -p \"$$\"; test -z \"$__HOH_SUBPROCESS_BOOTSTRAP_SESSION\"",
        ],
        None,
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    let values = String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(values.len(), 4);
    assert_ne!(
        values[0], values[1],
        "the target is a separate child of the launcher"
    );
    assert_eq!(&values[2..], [values[0]; 2]);
    assert!(!files.0.request_path.exists());
}

#[test]
fn publishes_a_native_launcher_exec_error_without_consuming_the_target_request() {
    let files = request("/tmp", Default::default());
    let output = launch(
        &files,
        &["--hoh-detached-exec", "--", "/missing-hoh-systemd-run"],
        None,
    );
    assert_eq!(output.status.code(), Some(127));
    let Some(LinuxStartupError::Error { error }) =
        read_linux_startup_error(&files.0.startup_error_path).unwrap()
    else {
        panic!("missing native launcher diagnostic");
    };
    assert_eq!(error.message, "spawn /missing-hoh-systemd-run ENOENT");
    assert_eq!(error.code.as_deref(), Some("ENOENT"));
    assert!(files.0.request_path.exists());
}
#[test]
fn loads_the_runner_from_an_isolated_application_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hoh-subprocess-runner"))
        .env_clear()
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(127));
    assert!(output.stderr.is_empty());
}
#[test]
fn resolves_the_runner_entry_and_checks_concrete_paths_without_executing_it() {
    assert!(runner_invocation_available(&spawn_runner_invocation()));
    assert!(!runner_invocation_available(&[
        "/definitely/missing-dsh-runner".into()
    ]));
    assert!(runner_invocation_available(&["node".into()]));
    assert!(runner_invocation_available(&[
        "node".into(),
        "runner.js".into()
    ]));
    assert!(!runner_invocation_available(&[
        "/bin/sh".into(),
        "/missing-entry".into()
    ]));
}
#[test]
fn publishes_request_and_early_protocol_failures_through_the_single_error_branch() {
    let files = request("/tmp", Default::default());
    std::fs::write(&files.0.request_path, "{").unwrap();
    assert_eq!(
        launch(&files, &["--", "tool"], None).status.code(),
        Some(127)
    );
    assert!(!files.0.request_path.exists());
    assert!(
        read_linux_startup_error(&files.0.startup_error_path)
            .unwrap()
            .is_some()
    );
    let files = request("/tmp", Default::default());
    assert_eq!(launch(&files, &["tool"], None).status.code(), Some(127));
    let Some(LinuxStartupError::Error { error }) =
        read_linux_startup_error(&files.0.startup_error_path).unwrap()
    else {
        panic!("missing diagnostic")
    };
    assert_eq!(
        error.message,
        "subprocess runner requires target argv after a private -- delimiter"
    );
}
#[test]
fn uses_final_cwd_and_env_path_while_preserving_the_original_argv() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("work")).unwrap();
    std::fs::create_dir(root.path().join("work/bin")).unwrap();
    let script = root.path().join("work/bin/tool");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '%s|%s|%s' \"$PWD\" \"$1\" \"$DSH_SUBPROCESS_RUNNER\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let files = request(
        "work",
        [
            ("PATH".into(), "bin:".into()),
            (SUBPROCESS_RUNNER_ENV.into(), "target-value".into()),
        ]
        .into(),
    );
    let output = launch(&files, &["--", "tool", "literal $HOME"], Some(root.path()));
    assert!(
        output.status.success(),
        "{:?}",
        read_linux_startup_error(&files.0.startup_error_path)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}/work|literal $HOME|target-value", root.path().display())
    );
    assert!(!files.0.request_path.exists());
    assert!(!files.0.startup_error_path.exists());
}
#[test]
fn retries_enoexec_through_bin_sh() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("tool");
    std::fs::write(&script, "printf '%s' \"$1\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let files = request(
        dir.path().to_str().unwrap(),
        [("PATH".into(), "".into())].into(),
    );
    let output = launch(&files, &["--", "tool", "literal arg"], None);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"literal arg");
}
#[test]
fn preserves_node_shaped_enoent_and_eacces_spawn_failures() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tool"), "exit 0").unwrap();
    std::fs::set_permissions(
        dir.path().join("tool"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    for (program, expected) in [("missing-tool", "ENOENT"), ("tool", "EACCES")] {
        let files = request(
            dir.path().to_str().unwrap(),
            [("PATH".into(), ":/missing".into())].into(),
        );
        assert_eq!(
            launch(&files, &["--", program], None).status.code(),
            Some(127)
        );
        let Some(LinuxStartupError::Error { error }) =
            read_linux_startup_error(&files.0.startup_error_path).unwrap()
        else {
            panic!("missing diagnostic")
        };
        assert_eq!(error.name, "Error");
        assert_eq!(error.message, format!("spawn {program} {expected}"));
        assert_eq!(error.code.as_deref(), Some(expected));
        assert_eq!(error.path.as_deref(), Some(program));
        assert_eq!(
            error.syscall.as_deref(),
            Some(format!("spawn {program}").as_str())
        );
    }
}
#[test]
fn passes_blocking_standard_streams_to_a_real_native_executable() {
    let files = request("/tmp", Default::default());
    let output = launch(
        &files,
        &[
            "--",
            "/bin/cat",
            "/proc/self/fdinfo/0",
            "/proc/self/fdinfo/1",
            "/proc/self/fdinfo/2",
        ],
        None,
    );
    assert!(output.status.success());
    let flags = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            line.strip_prefix("flags:\t")
                .map(|octal| u32::from_str_radix(octal, 8).unwrap())
        })
        .collect::<Vec<_>>();
    assert_eq!(flags.len(), 3);
    assert_eq!(
        flags.iter().map(|flags| flags & 0x800).collect::<Vec<_>>(),
        [0, 0, 0]
    );
}
