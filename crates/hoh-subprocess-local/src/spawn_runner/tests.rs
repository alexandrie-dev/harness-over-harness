use super::*;
use crate::runner_protocol::RunnerFailure;
use hoh_subprocess::SubprocessControlMode;

fn request(cwd: &str, path: Option<&str>) -> LinuxLaunchRequest {
    LinuxLaunchRequest {
        cwd: cwd.into(),
        env: path
            .map(|p| [("PATH".into(), p.into())].into())
            .unwrap_or_default(),
        pipe_fds: None,
        control: None,
        control_path: None,
    }
}
fn failure(code: &str) -> BoxError {
    deserialize_runner_error(SerializedRunnerError {
        name: "Error".into(),
        message: code.into(),
        code: Some(code.into()),
        syscall: None,
        path: None,
    })
}
fn code(error: &BoxError) -> &str {
    error
        .downcast_ref::<RunnerFailure>()
        .unwrap()
        .diagnostic()
        .code
        .as_deref()
        .unwrap()
}
#[test]
fn uses_final_cwd_env_path_while_preserving_the_original_argv() {
    let mut request = request("/final/work", Some("relative::/absolute"));
    request
        .env
        .insert("DSH_SUBPROCESS_RUNNER".into(), "target-value".into());
    let argv = vec!["tool".into(), "literal arg".into()];
    let mut calls = vec![];
    let error = exec_target_with(&request, &argv, |file, args, target| {
        calls.push(file.to_owned());
        assert_eq!(args, argv);
        assert_eq!(target.env["DSH_SUBPROCESS_RUNNER"], "target-value");
        Err(failure("ENOENT"))
    })
    .unwrap_err();
    assert_eq!(
        calls,
        [
            "/final/work/relative/tool",
            "/final/work/tool",
            "/absolute/tool"
        ]
    );
    assert_eq!(error.to_string(), "spawn tool ENOENT");
}
#[test]
fn resolves_relative_path_entries_from_the_cwd_after_chdir() {
    for (cwd, path, expected) in [
        (
            "/base/work",
            "bin:",
            vec!["/base/work/bin/tool", "/base/work/tool"],
        ),
        ("/", "", vec!["/tool"]),
    ] {
        let mut calls = vec![];
        let _ = exec_target_with(&request(cwd, Some(path)), &["tool".into()], |file, _, _| {
            calls.push(file.to_owned());
            Err(failure("ENOENT"))
        });
        assert_eq!(calls, expected);
    }
}
#[test]
fn preserves_symlink_sensitive_parent_traversal_in_path_candidates() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().join("cwd");
    let target = directory.path().join("target");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::create_dir_all(target.join("child")).unwrap();
    std::fs::write(target.join("tool"), b"").unwrap();
    std::os::unix::fs::symlink(target.join("child"), cwd.join("link")).unwrap();
    let request = request(cwd.to_str().unwrap(), Some("link/.."));
    let error = exec_target_with(&request, &["tool".into()], |file, _, _| {
        assert_eq!(file, format!("{}/link/../tool", cwd.display()));
        Err(failure(if std::path::Path::new(file).exists() {
            "EIO"
        } else {
            "ENOENT"
        }))
    })
    .unwrap_err();
    assert_eq!(code(&error), "EIO");
}
#[test]
fn retries_enoexec_through_bin_sh_with_and_without_control() {
    for control in [None, Some(SubprocessControlMode::Pipe)] {
        let mut request = request("/work", Some("bin"));
        request.control = control;
        request.pipe_fds = Some(vec![0, 2]);
        let mut calls = vec![];
        let error = exec_target_with(
            &request,
            &["tool".into(), "literal arg".into()],
            |file, argv, target| {
                assert_eq!(target.control, control);
                assert_eq!(target.pipe_fds, Some(vec![0, 2]));
                calls.push((file.to_owned(), argv.to_vec()));
                Err(failure(if calls.len() == 1 { "ENOEXEC" } else { "EIO" }))
            },
        )
        .unwrap_err();
        assert_eq!(
            calls,
            vec![
                (
                    "/work/bin/tool".into(),
                    vec!["tool".into(), "literal arg".into()]
                ),
                (
                    "/bin/sh".into(),
                    vec![
                        "/bin/sh".into(),
                        "/work/bin/tool".into(),
                        "literal arg".into()
                    ]
                ),
            ]
        );
        assert_eq!(code(&error), "EIO");
    }
}
#[test]
fn uses_the_default_path_and_stops_on_a_non_search_error() {
    let mut calls = vec![];
    let error = exec_target_with(&request("/work", None), &["tool".into()], |file, _, _| {
        calls.push(file.to_owned());
        Err(failure("EACCES"))
    })
    .unwrap_err();
    assert_eq!(calls, ["/usr/bin/tool", "/bin/tool"]);
    assert_eq!(code(&error), "EACCES");
    for program in ["./tool", "/tool"] {
        let mut calls = vec![];
        let error = exec_target_with(&request("/work", None), &[program.into()], |file, _, _| {
            calls.push(file.to_owned());
            Err(crate::error("unclassified failure"))
        })
        .unwrap_err();
        assert_eq!(calls, [program]);
        assert_eq!(error.to_string(), "unclassified failure");
    }
    let mut calls = 0;
    let error = exec_target_with(&request("/work", None), &["tool".into()], |_, _, _| {
        calls += 1;
        Err(failure(if calls == 1 { "ENOTDIR" } else { "EIO" }))
    })
    .unwrap_err();
    assert_eq!(calls, 2);
    assert_eq!(code(&error), "EIO");
}
