mod common;
use common::*;
use hoh_native_command::{host_environment, runner::*, *};
use std::sync::Arc;

#[tokio::test]
async fn opens_with_macos_open_1() {
    let run = Mock::output("");
    open_native_path(
        "/Users/test/file.txt",
        &Signal::default(),
        &facts("darwin", &run),
    )
    .await
    .unwrap();
    assert_eq!(
        run.calls(),
        [call("open", &["/Users/test/file.txt"], Window::Hidden)]
    );
}
#[tokio::test]
async fn bypasses_macos_file_associations_for_text_documents() {
    let run = Mock::output("");
    open_native_text_file(
        "/Users/test/settings.yaml",
        &Signal::default(),
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
}
#[tokio::test]
async fn uses_the_linux_desktop_association_for_text_documents() {
    let run = Mock::output("");
    open_native_text_file(
        "/tmp/settings.yaml",
        &Signal::default(),
        &facts("linux", &run),
    )
    .await
    .unwrap();
    assert_eq!(
        run.calls(),
        [call("xdg-open", &["/tmp/settings.yaml"], Window::Hidden)]
    );
}
#[tokio::test]
async fn hands_wsl_text_documents_to_the_windows_desktop_from_each_marker() {
    for (markers, release) in [
        (env(&[("WSL_DISTRO_NAME", "Ubuntu")]), "generic"),
        (env(&[("WSL_INTEROP", "/run/WSL/123_interop")]), "generic"),
        (env(&[]), "5.15.153.1-microsoft-standard-WSL2"),
    ] {
        let run = Mock::output("\\\\wsl.localhost\\Ubuntu\\home\\test user\\settings.yaml\r\n");
        let mut f = facts("linux", &run);
        f.env = Some(markers);
        f.os_release = Some(release.into());
        open_native_text_file("/home/test user/settings.yaml", &Signal::default(), &f)
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
    }
}
#[tokio::test]
async fn rejects_an_empty_wsl_path_translation_before_invoking_windows() {
    let run = Mock::output("\r\n");
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    assert_eq!(
        open_native_text_file("/file", &Signal::default(), &f)
            .await
            .unwrap_err()
            .message,
        "wslpath returned no Windows path"
    );
    assert_eq!(run.calls().len(), 1);
}
#[tokio::test]
async fn does_not_invoke_windows_when_the_request_aborts_during_wsl_path_translation() {
    let abort = Arc::new(Signal::default());
    let held = abort.clone();
    let run = Mock::new(move |_, _, _, _| {
        held.abort("closed");
        Ok(output("C:\\file\n"))
    });
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    assert_eq!(
        open_native_text_file("/file", &*abort, &f)
            .await
            .unwrap_err()
            .message,
        "closed"
    );
    assert_eq!(run.calls().len(), 1);
}
#[tokio::test]
async fn opens_each_path_through_the_one_encoded_explorer_target() {
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
        open_native_path(path, &Signal::default(), &facts("win32", &run))
            .await
            .unwrap();
        assert_eq!(
            run.calls(),
            [call("explorer.exe", &[target], Window::Visible)]
        );
    }
}
#[tokio::test]
async fn encodes_a_separator_terminated_drive_root_as_the_hosting_platform_resolves_it() {
    let run = Mock::output("");
    open_native_path("C:\\", &Signal::default(), &facts("win32", &run))
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
}
#[tokio::test]
async fn encodes_a_directory_target_the_same_way_it_encodes_a_file_target() {
    let run = Mock::output("");
    open_native_associated_path(
        "C:\\work\\dir,comma",
        &Signal::default(),
        &facts("win32", &run),
    )
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
}
#[tokio::test]
async fn uses_the_windows_desktop_association_for_text_documents() {
    let run = Mock::output("");
    open_native_text_file(
        "C:\\work\\settings.yaml",
        &Signal::default(),
        &facts("win32", &run),
    )
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
}
#[tokio::test]
async fn opens_with_linux_xdg_open() {
    let run = Mock::output("");
    open_native_path("/tmp/a.txt", &Signal::default(), &facts("linux", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [call("xdg-open", &["/tmp/a.txt"], Window::Hidden)]
    );
}
#[tokio::test]
async fn rejects_unsupported_platforms() {
    let run = Mock::output("");
    assert_eq!(
        open_native_path("/x", &Signal::default(), &facts("freebsd", &run))
            .await
            .unwrap_err()
            .message,
        "native path opener is unsupported on freebsd"
    );
}
#[tokio::test]
async fn uses_the_current_process_platform_when_no_platform_override_is_supplied() {
    let run = Mock::output("");
    let mut f = facts("linux", &run);
    f.platform = None;
    open_native_path("/tmp/platform-default.txt", &Signal::default(), &f)
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
}
#[tokio::test]
async fn samples_ambient_wsl_markers_and_kernel_release_when_no_fact_overrides_are_supplied() {
    let run = Mock::output("C:\\settings.yaml\n");
    let f = PathOpenerInternals {
        platform: Some("linux".into()),
        run: Some(&run),
        ..Default::default()
    };
    open_native_path("/tmp/ambient-facts.yaml", &Signal::default(), &f)
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
}
#[tokio::test]
async fn keeps_explorer_windows_visible_through_each_adapter() {
    for reveal in [false, true] {
        let run = Mock::output("");
        let f = facts("win32", &run);
        let signal = Signal::default();
        if reveal {
            reveal_native_path("C:\\work\\report.txt", &signal, &f)
                .await
                .unwrap();
        } else {
            open_native_path("C:\\work\\report.txt", &signal, &f)
                .await
                .unwrap();
        }
        assert_eq!(run.calls()[0].window, Window::Visible);
    }
}
#[tokio::test]
async fn runs_the_command_adapter_without_a_shell_and_preserves_command_failures() {
    let run = Mock::new(|_, _, _, _| {
        let mut error = exit(1);
        error.message = "open failed".into();
        error.stdout = "partial output".into();
        error.stderr = "failure details".into();
        error.cause = Some(Box::new(std::io::Error::other("open failed")));
        Err(error)
    });
    let error = open_native_path(
        "/tmp/missing.txt",
        &Signal::default(),
        &facts("darwin", &run),
    )
    .await
    .unwrap_err();
    assert_eq!(
        run.calls(),
        [call("open", &["/tmp/missing.txt"], Window::Hidden)]
    );
    assert_eq!(error.message, "open failed");
    assert_eq!(error.code, Some(CommandCode::Exit(1)));
    assert_eq!(error.stdout, "partial output");
    assert_eq!(error.stderr, "failure details");
    assert!(error.cause.is_some());
}

const LS_PLIST: &str = "{ LSHandlers = ( { LSHandlerPreferredVersions = { LSHandlerRoleAll = \"-\"; }; LSHandlerRoleAll = \"com.google.chrome\"; LSHandlerURLScheme = https; } ); }";
#[tokio::test]
async fn opens_a_page_with_the_default_browser_rather_than_the_html_handler_on_darwin() {
    let run = Mock::output(LS_PLIST);
    open_native_path("/w/page.html", &Signal::default(), &facts("darwin", &run))
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
}
#[tokio::test]
async fn leaves_every_other_document_to_the_default_application() {
    let run = Mock::output("");
    open_native_path("/w/report.md", &Signal::default(), &facts("darwin", &run))
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [call("open", &["/w/report.md"], Window::Hidden)]
    );
}
#[tokio::test]
async fn falls_back_to_the_default_application_when_no_browser_can_be_named() {
    for throws in [true, false] {
        let run = Mock::new(move |command, _, _, _| {
            if throws && command == "defaults" {
                Err(NativeCommandError::message("domain not found"))
            } else {
                Ok(output("{ LSHandlers = ( ); }"))
            }
        });
        open_native_path("/w/page.html", &Signal::default(), &facts("darwin", &run))
            .await
            .unwrap();
        assert_eq!(
            run.calls()[1],
            call("open", &["/w/page.html"], Window::Hidden)
        );
    }
}
#[tokio::test]
async fn honors_browser_on_linux_and_leaves_windows_to_its_association() {
    for (platform, browser, expected) in [
        ("linux", "firefox", "firefox"),
        ("linux", "", "xdg-open"),
        ("win32", "firefox", "explorer.exe"),
    ] {
        let run = Mock::output("");
        let mut f = facts(platform, &run);
        f.env = Some(env(&[("BROWSER", browser)]));
        open_native_path(
            if platform == "win32" {
                "C:\\w\\page.html"
            } else {
                "/w/page.html"
            },
            &Signal::default(),
            &f,
        )
        .await
        .unwrap();
        assert_eq!(run.calls()[0].command, expected);
        assert_eq!(run.calls().len(), 1);
    }
}
#[tokio::test]
async fn hands_browser_renderable_wsl_paths_to_the_windows_desktop() {
    let run = Mock::output("C:\\workspace\\page.html\n");
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    f.env = Some(env(&[("BROWSER", "firefox")]));
    open_native_path("/home/test/page.html", &Signal::default(), &f)
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
        reveal_native_path(path, &Signal::default(), &f)
            .await
            .unwrap();
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
    }
}
#[tokio::test]
async fn selects_a_translated_wsl_path_in_explorer_and_never_starts_a_linux_file_manager() {
    let run = Mock::output("C:\\work\\报告.txt\r\n");
    let mut f = facts("linux", &run);
    f.env = Some(env(&[("WSL_DISTRO_NAME", "Ubuntu")]));
    assert_eq!(native_file_manager(&f), Some(NativeFileManager::Explorer));
    reveal_native_path("/mnt/c/work/报告.txt", &Signal::default(), &f)
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
}
#[tokio::test]
async fn refuses_empty_wsl_translations_and_cancelled_translation_without_launching_explorer() {
    let run = Mock::output("");
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    assert!(
        reveal_native_path("/file", &Signal::default(), &f)
            .await
            .unwrap_err()
            .message
            .contains("no Windows path")
    );
    let abort = Arc::new(Signal::default());
    let held = abort.clone();
    let run = Mock::new(move |_, _, _, _| {
        held.abort("stopped");
        Ok(output("C:\\file"))
    });
    f.run = Some(&run);
    assert_eq!(
        reveal_native_path("/file", &*abort, &f)
            .await
            .unwrap_err()
            .message,
        "stopped"
    );
    assert!(run.calls().iter().all(|c| c.command == "wslpath"));
}
#[tokio::test]
async fn rejects_unsupported_platforms_cancellation_and_launcher_errors() {
    let run = Mock::new(|_, _, _, _| Err(NativeCommandError::message("desktop failed")));
    assert_eq!(native_file_manager(&facts("aix", &run)), None);
    assert!(
        reveal_native_path("/file", &Signal::default(), &facts("aix", &run))
            .await
            .unwrap_err()
            .message
            .contains("unsupported")
    );
    let abort = Signal::default();
    abort.abort("cancelled");
    assert_eq!(
        reveal_native_path("/file", &abort, &facts("linux", &run))
            .await
            .unwrap_err()
            .message,
        "cancelled"
    );
    assert!(run.calls().is_empty());
    assert_eq!(
        reveal_native_path("/file", &Signal::default(), &facts("darwin", &run))
            .await
            .unwrap_err()
            .message,
        "desktop failed"
    );
    assert_eq!(
        native_file_manager(&facts("linux", &run)),
        Some(NativeFileManager::Directory)
    );
}
#[tokio::test]
async fn uses_the_native_runner_for_a_file_manager_handoff_when_none_is_injected() {
    // Real default runner lookup, cancelled before any desktop command is launched.
    let signal = Signal::default();
    signal.abort("closed");
    assert_eq!(
        reveal_native_path("/tmp/report.txt", &signal, &Default::default())
            .await
            .unwrap_err()
            .message,
        "closed"
    );
    let run = Mock::output("");
    reveal_native_path("/tmp/report.txt", &Signal::default(), &facts("linux", &run))
        .await
        .unwrap();
    assert!(!run.calls().is_empty());
}
#[tokio::test]
async fn accepts_explorer_delegate_exit_1_through_the_runner_on_each_platform() {
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
            &Signal::default(),
            &f,
        )
        .await
        .unwrap();
    }
}
#[tokio::test]
async fn preserves_each_explorer_failure() {
    for code in [
        Some(CommandCode::Exit(2)),
        Some(CommandCode::Name("ENOENT".into())),
        None,
    ] {
        let held = code.clone();
        let run = Mock::new(move |_, _, _, _| {
            Err(NativeCommandError {
                code: held.clone(),
                ..NativeCommandError::message("launch failed")
            })
        });
        assert_eq!(
            reveal_native_path("C:\\file.txt", &Signal::default(), &facts("win32", &run))
                .await
                .unwrap_err()
                .code,
            code
        );
    }
}
#[tokio::test]
async fn preserves_cancellation_even_when_explorer_returns_delegate_exit_1() {
    let abort = Arc::new(Signal::default());
    let held = abort.clone();
    let run = Mock::new(move |_, _, _, _| {
        held.abort("cancelled");
        Err(exit(1))
    });
    assert_eq!(
        reveal_native_path("C:\\file.txt", &*abort, &facts("win32", &run))
            .await
            .unwrap_err()
            .message,
        "cancelled"
    );
}
#[tokio::test]
async fn accepts_the_explorer_delegate_exit_1_when_opening_on_each_platform() {
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
            &Signal::default(),
            &f,
        )
        .await
        .unwrap();
    }
}
#[tokio::test]
async fn preserves_a_non_delegate_explorer_failure_when_opening() {
    let run = Mock::new(|_, _, _, _| Err(exit(2)));
    assert_eq!(
        open_native_path("C:\\file.txt", &Signal::default(), &facts("win32", &run))
            .await
            .unwrap_err()
            .code,
        Some(CommandCode::Exit(2))
    );
}
#[tokio::test]
async fn preserves_special_characters_in_each_explorer_target() {
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
        reveal_native_path(path, &Signal::default(), &facts("win32", &run))
            .await
            .unwrap();
        assert_eq!(
            run.calls(),
            [call("explorer.exe", &["/select,", target], Window::Visible)]
        );
    }
}
#[tokio::test]
async fn uses_the_file_association_for_each_extension_despite_a_configured_browser() {
    for extension in [".html", ".svg"] {
        for platform in ["darwin", "linux"] {
            let run = Mock::output("browser bundle");
            let mut f = facts(platform, &run);
            f.env = Some(env(&[("BROWSER", "custom-browser")]));
            let path = format!("/work/report{extension}");
            open_native_associated_path(&path, &Signal::default(), &f)
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
