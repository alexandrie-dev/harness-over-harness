mod common;
use common::*;
use hoh_native_command::{host_environment, runner::*, *};
use std::sync::Arc;

#[tokio::test]
async fn opens_with_macos_open_1() {
    let request = signal();
    let run = Mock::output("");
    open_native_path("/Users/test/file.txt", &request, &facts("darwin", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [call("open", &["/Users/test/file.txt"], Window::Hidden)]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn bypasses_macos_file_associations_for_text_documents() {
    let request = signal();
    let run = Mock::output("");
    open_native_text_file(
        "/Users/test/settings.yaml",
        &request,
        &facts("darwin", &run),
    )
    .await
    .unwrap();
    assert_eq!(
        run.calls(),
        [call(
            "open",
            &["-t", "/Users/test/settings.yaml"],
            Window::Hidden
        )]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn uses_the_linux_desktop_association_for_text_documents() {
    let request = signal();
    let run = Mock::output("");
    open_native_text_file("/tmp/settings.yaml", &request, &facts("linux", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [call("xdg-open", &["/tmp/settings.yaml"], Window::Hidden)]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn hands_wsl_text_documents_to_the_windows_desktop_from_each_marker() {
    let request = signal();
    for (markers, release) in [
        (env(&[("WSL_DISTRO_NAME", "Ubuntu")]), "generic"),
        (env(&[("WSL_INTEROP", "/run/WSL/123_interop")]), "generic"),
        (env(&[]), "5.15.153.1-microsoft-standard-WSL2"),
    ] {
        let run = Mock::output("\\\\wsl.localhost\\Ubuntu\\home\\test user\\settings.yaml\r\n");
        let mut f = facts("linux", &run);
        f.env = Some(markers);
        f.os_release = Some(release.into());
        open_native_text_file("/home/test user/settings.yaml", &request, &f)
            .await
            .unwrap();
        assert_eq!(
            run.calls(),
            [
                call(
                    "wslpath",
                    &["-w", "/home/test user/settings.yaml"],
                    Window::Hidden
                ),
                call(
                    "explorer.exe",
                    &["file://wsl.localhost/Ubuntu/home/test%20user/settings.yaml"],
                    Window::Visible
                )
            ]
        );
        run.assert_signals(&request);
    }
}
#[tokio::test]
async fn rejects_an_empty_wsl_path_translation_before_invoking_windows() {
    let request = signal();
    let run = Mock::output("\r\n");
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    assert_eq!(
        open_native_text_file("/file", &request, &f)
            .await
            .unwrap_err()
            .to_string(),
        "wslpath returned no Windows path"
    );
    assert_eq!(run.calls().len(), 1);
    run.assert_signals(&request);
}
#[tokio::test]
async fn does_not_invoke_windows_when_the_request_aborts_during_wsl_path_translation() {
    let controller = Arc::new(AbortController::new());
    let held = controller.clone();
    let reason = SharedError::new(std::io::Error::other("closed"));
    let thrown = reason.clone();
    let run = Mock::new(move |_, _, _, _| {
        held.abort_with(thrown.clone());
        Ok(output("C:\\file\n"))
    });
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    let request = controller.signal();
    let error = open_native_text_file("/file", &request, &f)
        .await
        .unwrap_err();
    assert!(error.ptr_eq(&reason));
    assert_eq!(
        run.calls(),
        [call("wslpath", &["-w", "/file"], Window::Hidden)]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn opens_each_path_through_the_one_encoded_explorer_target() {
    let request = signal();
    for (path, target) in [
        (
            "C:\\work\\o'reilly, & 100%.txt",
            "file:///C:/work/o'reilly%2C%20&%20100%25.txt",
        ),
        ("C:\\work\\plain.txt", "file:///C:/work/plain.txt"),
        ("C:\\work\\my report.txt", "file:///C:/work/my%20report.txt"),
        ("C:\\work\\key=value.txt", "file:///C:/work/key%3Dvalue.txt"),
        (
            "C:\\my files\\报告,#%.txt",
            "file:///C:/my%20files/报告%2C%23%25.txt",
        ),
        ("C:\\目录\\a,b=c.txt", "file:///C:/目录/a%2Cb%3Dc.txt"),
        ("C:\\工作\\报告 2026.txt", "file:///C:/工作/报告%202026.txt"),
        (
            "\\\\server\\share\\a,b.txt",
            "file://server/share/a%2Cb.txt",
        ),
        ("\\\\?\\C:\\work\\a,b.txt", "file:///C:/work/a%2Cb.txt"),
        (
            "\\\\?\\UNC\\server\\share\\a,b.txt",
            "file://server/share/a%2Cb.txt",
        ),
        ("\\\\.\\C:\\work\\a,b.txt", "file://./C:/work/a%2Cb.txt"),
        (
            "\\\\wsl$\\Ubuntu\\home\\t,est a.txt",
            "file://wsl$/Ubuntu/home/t%2Cest%20a.txt",
        ),
    ] {
        let run = Mock::output("");
        open_native_path(path, &request, &facts("win32", &run))
            .await
            .unwrap();
        assert_eq!(
            run.calls(),
            [call("explorer.exe", &[target], Window::Visible)]
        );
        run.assert_signals(&request);
    }
}
#[tokio::test]
async fn encodes_a_separator_terminated_drive_root_as_the_hosting_platform_resolves_it() {
    let request = signal();
    let run = Mock::output("");
    open_native_path("C:\\", &request, &facts("win32", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [call(
            "explorer.exe",
            &[if cfg!(windows) {
                "file:///C:/"
            } else {
                "file:///C://"
            }],
            Window::Visible
        )]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn encodes_a_directory_target_the_same_way_it_encodes_a_file_target() {
    let request = signal();
    let run = Mock::output("");
    open_native_associated_path("C:\\work\\dir,comma", &request, &facts("win32", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [call(
            "explorer.exe",
            &["file:///C:/work/dir%2Ccomma"],
            Window::Visible
        )]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn uses_the_windows_desktop_association_for_text_documents() {
    let request = signal();
    let run = Mock::output("");
    open_native_text_file("C:\\work\\settings.yaml", &request, &facts("win32", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [call(
            "explorer.exe",
            &["file:///C:/work/settings.yaml"],
            Window::Visible
        )]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn opens_with_linux_xdg_open() {
    let request = signal();
    let run = Mock::output("");
    open_native_path("/tmp/a.txt", &request, &facts("linux", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [call("xdg-open", &["/tmp/a.txt"], Window::Hidden)]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn rejects_unsupported_platforms() {
    let request = signal();
    let run = Mock::output("");
    assert_eq!(
        open_native_path("/x", &request, &facts("freebsd", &run))
            .await
            .unwrap_err()
            .to_string(),
        "native path opener is unsupported on freebsd"
    );
    assert!(run.calls().is_empty());
}
#[tokio::test]
async fn uses_the_current_process_platform_when_no_platform_override_is_supplied() {
    let request = signal();
    let run = Mock::output("");
    let mut f = facts("linux", &run);
    f.platform = None;
    open_native_path("/tmp/platform-default.txt", &request, &f)
        .await
        .unwrap();
    assert_eq!(
        run.calls()[0].command,
        match host_environment::platform() {
            "win32" => "explorer.exe",
            "linux" => "xdg-open",
            _ => "open",
        }
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn samples_ambient_wsl_markers_and_kernel_release_when_no_fact_overrides_are_supplied() {
    let request = signal();
    let run = Mock::output("C:\\settings.yaml\n");
    let f = PathOpenerInternals {
        platform: Some("linux".into()),
        run: Some(&run),
        ..Default::default()
    };
    open_native_path("/tmp/ambient-facts.yaml", &request, &f)
        .await
        .unwrap();
    let wsl = is_wsl(&HostEnvironmentFacts {
        platform: Some("linux".into()),
        ..Default::default()
    });
    assert_eq!(
        run.calls()[0].command,
        if wsl { "wslpath" } else { "xdg-open" }
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn keeps_explorer_windows_visible_through_each_adapter() {
    let request = signal();
    for reveal in [false, true] {
        let run = Mock::output("");
        let f = facts("win32", &run);
        let request = signal();
        if reveal {
            reveal_native_path("C:\\work\\report.txt", &request, &f)
                .await
                .unwrap();
        } else {
            open_native_path("C:\\work\\report.txt", &request, &f)
                .await
                .unwrap();
        }
        let args = if reveal {
            vec!["/select,", "file:///C:/work/report.txt"]
        } else {
            vec!["file:///C:/work/report.txt"]
        };
        assert_eq!(run.calls(), [call("explorer.exe", &args, Window::Visible)]);
        run.assert_signals(&request);
    }
    #[cfg(unix)]
    for window in [Window::Hidden, Window::Visible] {
        let output = SystemCommandRunner
            .run(
                "/bin/sh",
                &["-c".into(), "printf 'visible✓'".into()],
                &request,
                window,
            )
            .await
            .unwrap();
        assert_eq!(
            output,
            CommandOutput {
                stdout: "visible✓".into(),
                stderr: String::new()
            }
        );
    }
}
#[tokio::test]
async fn runs_the_command_adapter_without_a_shell_and_preserves_command_failures() {
    #[cfg(unix)]
    {
        let request = signal();
        let text = "spaces ' quotes $ and ✓; $(touch nope)";
        let result = SystemCommandRunner
            .run(
                "/bin/sh",
                &[
                    "-c".into(),
                    "printf '%s' \"$1\"; printf err >&2".into(),
                    "fixture".into(),
                    text.into(),
                ],
                &request,
                Window::Hidden,
            )
            .await
            .unwrap();
        assert_eq!(
            result,
            CommandOutput {
                stdout: text.into(),
                stderr: "err".into()
            }
        );
        let error = SystemCommandRunner
            .run(
                "/bin/sh",
                &[
                    "-c".into(),
                    "printf 'partial output'; printf 'failure details' >&2; exit 1".into(),
                ],
                &request,
                Window::Hidden,
            )
            .await
            .unwrap_err();
        assert_eq!(detail(&error).code, Some(CommandCode::Exit(1)));
        assert_eq!(detail(&error).stdout.as_deref(), Some("partial output"));
        assert_eq!(detail(&error).stderr.as_deref(), Some("failure details"));
        let message = "Command failed: /bin/sh -c printf 'partial output'; printf 'failure details' >&2; exit 1\nfailure details";
        assert_eq!(error.to_string(), message);
        assert_eq!(detail(&error).cause.as_ref().unwrap().to_string(), message);
    }
}

const LS_PLIST: &str = "{ LSHandlers = ( { LSHandlerPreferredVersions = { LSHandlerRoleAll = \"-\"; }; LSHandlerRoleAll = \"com.google.chrome\"; LSHandlerURLScheme = https; } ); }";
#[tokio::test]
async fn opens_a_page_with_the_default_browser_rather_than_the_html_handler_on_darwin() {
    let request = signal();
    let run = Mock::output(LS_PLIST);
    open_native_path("/w/page.html", &request, &facts("darwin", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [
            call(
                "defaults",
                &[
                    "read",
                    "com.apple.LaunchServices/com.apple.launchservices.secure"
                ],
                Window::Hidden
            ),
            call(
                "open",
                &["-b", "com.google.chrome", "/w/page.html"],
                Window::Hidden
            )
        ]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn leaves_every_other_document_to_the_default_application() {
    let request = signal();
    let run = Mock::output("");
    open_native_path("/w/report.md", &request, &facts("darwin", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [call("open", &["/w/report.md"], Window::Hidden)]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn falls_back_to_the_default_application_when_no_browser_can_be_named() {
    for throws in [true, false] {
        let run = Mock::new(move |command, _, _, _| {
            if throws && command == "defaults" {
                Err(NativeCommandError::message("domain not found").into())
            } else {
                Ok(output("{ LSHandlers = ( ); }"))
            }
        });
        let request = signal();
        open_native_path("/w/page.html", &request, &facts("darwin", &run))
            .await
            .unwrap();
        assert_eq!(
            run.calls(),
            [
                call(
                    "defaults",
                    &[
                        "read",
                        "com.apple.LaunchServices/com.apple.launchservices.secure"
                    ],
                    Window::Hidden
                ),
                call("open", &["/w/page.html"], Window::Hidden)
            ]
        );
        run.assert_signals(&request);
    }
}
#[tokio::test]
async fn honors_browser_on_linux_and_leaves_windows_to_its_association() {
    for (platform, browser, command, argument) in [
        ("linux", Some("firefox"), "firefox", "/w/page.html"),
        ("linux", None, "xdg-open", "/w/page.html"),
        ("win32", None, "explorer.exe", "file:///C:/w/page.html"),
    ] {
        let run = Mock::output("");
        let mut f = facts(platform, &run);
        f.env = Some(browser.map(|b| env(&[("BROWSER", b)])).unwrap_or_default());
        let request = signal();
        open_native_path(
            if platform == "win32" {
                "C:\\w\\page.html"
            } else {
                "/w/page.html"
            },
            &request,
            &f,
        )
        .await
        .unwrap();
        assert_eq!(
            run.calls(),
            [call(
                command,
                &[argument],
                if platform == "win32" {
                    Window::Visible
                } else {
                    Window::Hidden
                }
            )]
        );
        run.assert_signals(&request);
    }
}
#[tokio::test]
async fn hands_browser_renderable_wsl_paths_to_the_windows_desktop() {
    let request = signal();
    let run = Mock::output("C:\\workspace\\page.html\n");
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    f.env = Some(env(&[("BROWSER", "firefox")]));
    open_native_path("/home/test/page.html", &request, &f)
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [
            call("wslpath", &["-w", "/home/test/page.html"], Window::Hidden),
            call(
                "explorer.exe",
                &["file:///C:/workspace/page.html"],
                Window::Visible
            )
        ]
    );
    run.assert_signals(&request);
}
#[test]
fn always_answers_yes_where_the_desktop_is_part_of_the_platform() {
    let run = Mock::output("");
    for platform in ["darwin", "win32"] {
        assert!(can_open_native_path(&facts(platform, &run)));
    }
}
#[test]
fn requires_a_display_server_or_wsl_interop_on_linux() {
    let run = Mock::output("");
    let mut f = facts("linux", &run);
    assert!(!can_open_native_path(&f));
    for (key, value) in [("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-0")] {
        f.env = Some(env(&[(key, value)]));
        assert!(can_open_native_path(&f));
    }
    f.env = Some(env(&[]));
    f.os_release = Some("5.15-microsoft-standard-WSL2".into());
    assert!(can_open_native_path(&f));
}
#[test]
fn answers_no_on_a_platform_the_opener_does_not_support() {
    let run = Mock::output("");
    assert!(!can_open_native_path(&facts("freebsd", &run)));
}
#[test]
fn samples_the_ambient_environment_when_no_override_is_supplied() {
    let expected = [
        "WSL_DISTRO_NAME",
        "WSL_INTEROP",
        "DISPLAY",
        "WAYLAND_DISPLAY",
    ]
    .iter()
    .any(|key| std::env::var(key).is_ok_and(|v| !v.is_empty()));
    assert_eq!(
        can_open_native_path(&PathOpenerInternals {
            platform: Some("linux".into()),
            os_release: Some("generic".into()),
            ..Default::default()
        }),
        expected
    );
}
#[test]
fn samples_the_ambient_platform_when_none_is_named() {
    assert_eq!(
        can_open_native_path(&Default::default()),
        can_open_native_path(&PathOpenerInternals {
            platform: Some(host_environment::platform().into()),
            ..Default::default()
        })
    );
}
#[tokio::test]
async fn reveals_through_each_platform_without_opening_the_file_association() {
    let request = signal();
    for (platform, manager, path, cmd, args) in [
        (
            "darwin",
            NativeFileManager::Finder,
            "/tmp/my report.txt",
            "open",
            vec!["-R", "/tmp/my report.txt"],
        ),
        (
            "win32",
            NativeFileManager::Explorer,
            "C:\\work\\my report.txt",
            "explorer.exe",
            vec!["/select,", "file:///C:/work/my%20report.txt"],
        ),
        (
            "linux",
            NativeFileManager::Directory,
            "/tmp/a $b; report.txt",
            "xdg-open",
            vec!["/tmp"],
        ),
    ] {
        let run = Mock::output("");
        let f = facts(platform, &run);
        assert_eq!(native_file_manager(&f), Some(manager));
        reveal_native_path(path, &request, &f).await.unwrap();
        assert_eq!(
            run.calls(),
            [call(
                cmd,
                &args,
                if platform == "win32" {
                    Window::Visible
                } else {
                    Window::Hidden
                }
            )]
        );
        run.assert_signals(&request);
    }
}
#[tokio::test]
async fn selects_a_translated_wsl_path_in_explorer_and_never_starts_a_linux_file_manager() {
    let request = signal();
    let run = Mock::output("C:\\work\\报告.txt\r\n");
    let mut f = facts("linux", &run);
    f.env = Some(env(&[("WSL_DISTRO_NAME", "Ubuntu")]));
    assert_eq!(native_file_manager(&f), Some(NativeFileManager::Explorer));
    reveal_native_path("/mnt/c/work/报告.txt", &request, &f)
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [
            call("wslpath", &["-w", "/mnt/c/work/报告.txt"], Window::Hidden),
            call(
                "explorer.exe",
                &["/select,", "file:///C:/work/报告.txt"],
                Window::Visible
            )
        ]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn refuses_empty_wsl_translations_and_cancelled_translation_without_launching_explorer() {
    let request = signal();
    let run = Mock::output("");
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    assert_eq!(
        reveal_native_path("/file", &request, &f)
            .await
            .unwrap_err()
            .to_string(),
        "wslpath returned no Windows path"
    );
    let controller = Arc::new(AbortController::new());
    let held = controller.clone();
    let reason = SharedError::new(std::io::Error::other("stopped"));
    let thrown = reason.clone();
    let run = Mock::new(move |_, _, _, _| {
        held.abort_with(thrown.clone());
        Ok(output("C:\\file"))
    });
    f.run = Some(&run);
    let request = controller.signal();
    let error = reveal_native_path("/file", &request, &f).await.unwrap_err();
    assert!(error.ptr_eq(&reason));
    assert_eq!(
        run.calls(),
        [call("wslpath", &["-w", "/file"], Window::Hidden)]
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn rejects_unsupported_platforms_cancellation_and_launcher_errors() {
    let request = signal();
    let failure = SharedError::new(NativeCommandError::message("desktop failed"));
    let held = failure.clone();
    let run = Mock::new(move |_, _, _, _| Err(held.clone()));
    assert_eq!(native_file_manager(&facts("aix", &run)), None);
    assert_eq!(
        reveal_native_path("/file", &request, &facts("aix", &run))
            .await
            .unwrap_err()
            .to_string(),
        "native file manager is unsupported on aix"
    );
    let controller = AbortController::new();
    let reason = SharedError::new(std::io::Error::other("cancelled"));
    controller.abort_with(reason.clone());
    assert!(
        reveal_native_path("/file", &controller.signal(), &facts("linux", &run))
            .await
            .unwrap_err()
            .ptr_eq(&reason)
    );
    assert!(run.calls().is_empty());
    assert!(
        reveal_native_path("/file", &request, &facts("darwin", &run))
            .await
            .unwrap_err()
            .ptr_eq(&failure)
    );
    assert_eq!(
        native_file_manager(&facts("linux", &run)),
        Some(NativeFileManager::Directory)
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn uses_the_native_runner_for_a_file_manager_handoff_when_none_is_injected() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        for command in ["xdg-open", "explorer.exe", "open"] {
            let file = root.path().join(command);
            tokio::fs::write(&file, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOH_DEFAULT_RUNNER_MARKER\"\ncase \"$0\" in */open) printf 'out✓'; printf err >&2; exit 3;; esac\n").await.unwrap();
            tokio::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700))
                .await
                .unwrap();
        }
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "default_runner_probe", "--nocapture"])
            .env("PATH", root.path())
            .env("HOH_DEFAULT_RUNNER_MARKER", root.path().join("marker"))
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output
                .stdout
                .windows(b"default_runner_probe".len())
                .any(|s| s == b"default_runner_probe")
        );
    }
}
#[tokio::test]
async fn accepts_explorer_delegate_exit_1_through_the_runner_on_each_platform() {
    let request = signal();
    for platform in ["win32", "linux"] {
        let run = Mock::new(|cmd, _, _, _| {
            if cmd == "wslpath" {
                Ok(output("C:\\work\\report.txt"))
            } else {
                Err(exit(1))
            }
        });
        let mut f = facts(platform, &run);
        f.env = Some(env(&[("WSL_DISTRO_NAME", "Ubuntu")]));
        reveal_native_path(
            if platform == "win32" {
                "C:\\work\\report.txt"
            } else {
                "/mnt/c/work/report.txt"
            },
            &request,
            &f,
        )
        .await
        .unwrap();
        run.assert_signals(&request);
    }
}
#[tokio::test]
async fn preserves_each_explorer_failure() {
    let request = signal();
    for code in [
        Some(CommandCode::Exit(2)),
        Some(CommandCode::Name("ENOENT".into())),
        None,
    ] {
        let failure = SharedError::new(NativeCommandError {
            code: code.clone(),
            ..NativeCommandError::message("launch failed")
        });
        let held = failure.clone();
        let run = Mock::new(move |_, _, _, _| Err(held.clone()));
        let error = reveal_native_path("C:\\file.txt", &request, &facts("win32", &run))
            .await
            .unwrap_err();
        assert!(error.ptr_eq(&failure));
        assert_eq!(detail(&error).code, code);
    }
}
#[tokio::test]
async fn preserves_cancellation_even_when_explorer_returns_delegate_exit_1() {
    let controller = Arc::new(AbortController::new());
    let held = controller.clone();
    let reason = SharedError::new(std::io::Error::other("cancelled"));
    let thrown = reason.clone();
    let run = Mock::new(move |_, _, _, _| {
        held.abort_with(thrown.clone());
        Err(exit(1))
    });
    let request = controller.signal();
    assert!(
        reveal_native_path("C:\\file.txt", &request, &facts("win32", &run))
            .await
            .unwrap_err()
            .ptr_eq(&reason)
    );
    run.assert_signals(&request);
}
#[tokio::test]
async fn accepts_the_explorer_delegate_exit_1_when_opening_on_each_platform() {
    let request = signal();
    for platform in ["win32", "linux"] {
        let run = Mock::new(|cmd, _, _, _| {
            if cmd == "wslpath" {
                Ok(output("C:\\work\\settings.yaml"))
            } else {
                Err(exit(1))
            }
        });
        let mut f = facts(platform, &run);
        f.env = Some(env(&[("WSL_DISTRO_NAME", "Ubuntu")]));
        open_native_path(
            if platform == "win32" {
                "C:\\work\\settings.yaml"
            } else {
                "/mnt/c/work/settings.yaml"
            },
            &request,
            &f,
        )
        .await
        .unwrap();
        run.assert_signals(&request);
    }
}
#[tokio::test]
async fn preserves_a_non_delegate_explorer_failure_when_opening() {
    let failure = exit(2);
    let held = failure.clone();
    let run = Mock::new(move |_, _, _, _| Err(held.clone()));
    let request = signal();
    let error = open_native_path("C:\\file.txt", &request, &facts("win32", &run))
        .await
        .unwrap_err();
    assert!(error.ptr_eq(&failure));
    run.assert_signals(&request);
}
#[tokio::test]
async fn preserves_special_characters_in_each_explorer_target() {
    let request = signal();
    for (path, target) in [
        (
            "C:\\my files\\报告,#%.txt",
            "file:///C:/my%20files/报告%2C%23%25.txt",
        ),
        (
            "\\\\server\\share\\a,b.txt",
            "file://server/share/a%2Cb.txt",
        ),
    ] {
        let run = Mock::output("");
        reveal_native_path(path, &request, &facts("win32", &run))
            .await
            .unwrap();
        assert_eq!(
            run.calls(),
            [call("explorer.exe", &["/select,", target], Window::Visible)]
        );
        run.assert_signals(&request);
    }
}
#[tokio::test]
async fn uses_the_file_association_for_each_extension_despite_a_configured_browser() {
    let request = signal();
    for extension in [".html", ".svg"] {
        for platform in ["darwin", "linux"] {
            let run = Mock::output("browser bundle");
            let mut f = facts(platform, &run);
            f.env = Some(env(&[("BROWSER", "custom-browser")]));
            let path = format!("/work/report{extension}");
            open_native_associated_path(&path, &request, &f)
                .await
                .unwrap();
            assert_eq!(
                run.calls(),
                [call(
                    if platform == "darwin" {
                        "open"
                    } else {
                        "xdg-open"
                    },
                    &[&path],
                    Window::Hidden
                )]
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn default_runner_probe() {
    let Some(marker) = std::env::var_os("HOH_DEFAULT_RUNNER_MARKER") else {
        return;
    };
    let request = signal();
    let linux = PathOpenerInternals {
        platform: Some("linux".into()),
        os_release: Some("generic".into()),
        env: Some(env(&[])),
        run: None,
    };
    reveal_native_path("/tmp/a $b; report.txt", &request, &linux)
        .await
        .unwrap();
    assert_eq!(tokio::fs::read_to_string(&marker).await.unwrap(), "/tmp\n");
    let windows = PathOpenerInternals {
        platform: Some("win32".into()),
        ..Default::default()
    };
    for reveal in [false, true] {
        if reveal {
            reveal_native_path("C:\\work\\report.txt", &request, &windows)
                .await
                .unwrap();
        } else {
            open_native_path("C:\\work\\report.txt", &request, &windows)
                .await
                .unwrap();
        }
        assert_eq!(
            tokio::fs::read_to_string(&marker).await.unwrap(),
            if reveal {
                "/select,\nfile:///C:/work/report.txt\n"
            } else {
                "file:///C:/work/report.txt\n"
            }
        );
    }
    let mac = PathOpenerInternals {
        platform: Some("darwin".into()),
        ..Default::default()
    };
    let literal = "/tmp/spaces ' quotes $; report✓.txt";
    let failure = open_native_path(literal, &request, &mac).await.unwrap_err();
    assert_eq!(
        tokio::fs::read_to_string(&marker).await.unwrap(),
        format!("{literal}\n")
    );
    assert_eq!(detail(&failure).stdout.as_deref(), Some("out✓"));
    assert_eq!(detail(&failure).stderr.as_deref(), Some("err"));
    assert_eq!(detail(&failure).code, Some(CommandCode::Exit(3)));
    assert_eq!(
        failure.to_string(),
        format!("Command failed: open {literal}\nerr")
    );
    assert!(detail(&failure).cause.is_some());
}
