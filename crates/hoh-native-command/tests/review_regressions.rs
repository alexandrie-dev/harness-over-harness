#![cfg(unix)]
mod common;
use common::*;
use hoh_native_command::{runner::*, *};
use serde::Deserialize;
use std::os::unix::fs::{PermissionsExt, symlink};

async fn application(root: &std::path::Path, relative: &str, name: &str) {
    let path = root.join("applications").join(relative);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(path, format!("[Desktop Entry]\nName={name}\n"))
        .await
        .unwrap();
}
fn gio_run(reply: &str) -> Mock {
    let reply = reply.to_owned();
    Mock::new(move |command, _, _, _| {
        Ok(output(if command == "gio" {
            "standard::content-type: text/plain"
        } else {
            &reply
        }))
    })
}
fn xdg<'a>(root: &str, run: &'a Mock) -> PathOpenerInternals<'a> {
    let mut f = facts("linux", run);
    f.env = Some(env(&[("XDG_DATA_HOME", root), ("XDG_DATA_DIRS", "")]));
    f
}

#[test]
fn desktop_entry_uses_javascript_whitespace_including_a_leading_bom() {
    assert_eq!(
        desktop_entry_fields("\u{feff}[Desktop Entry]\nName=A")["Name"],
        "A"
    );
    let fields = desktop_entry_fields(
        "[Desktop Entry]\n\u{85}Name\u{85}=\u{85}A\u{85}\n\u{feff}Icon\u{feff}=\u{feff}icon\u{feff}",
    );
    assert_eq!(fields["\u{85}Name\u{85}"], "\u{85}A\u{85}");
    assert_eq!(fields["Icon"], "icon");
}

#[tokio::test]
async fn gio_default_pattern_uses_the_last_colon_and_preserves_the_capture() {
    let root = tempfile::tempdir().unwrap();
    for (id, name) in [
        ("x.desktop", "X"),
        (" .desktop", "Space"),
        (".desktop", "Dot"),
    ] {
        application(root.path(), id, name).await;
    }
    for (reply, name) in [
        ("Default application for 'a:b': x.desktop", "X"),
        ("Default application for x: .desktop", "Space"),
        ("Other\rDefault application for x: x.desktop\rOther", "X"),
        (
            "Other\u{2028}Default application for x: x.desktop\u{2029}Other",
            "X",
        ),
    ] {
        let run = gio_run(reply);
        let apps = native_file_applications(
            "/file",
            &signal(),
            &xdg(root.path().to_str().unwrap(), &run),
        )
        .await
        .unwrap();
        assert_eq!(apps.len(), 1, "{reply}");
        assert_eq!(apps[0].name, name, "{reply}");
        assert!(apps[0].default);
    }
}

#[tokio::test]
async fn gio_registered_pattern_matches_all_five_lead_cases() {
    let root = tempfile::tempdir().unwrap();
    application(root.path(), ".desktop", "Dot").await;
    application(root.path(), "a.desktop", "A").await;
    for (line, expected) in [
        ("\t.desktop", Some("Dot")),
        ("\ta.desktop", Some("A")),
        (" .desktop ", Some("Dot")),
        (".desktop", None),
        ("a.desktop", None),
        ("\u{feff}a.desktop", Some("A")),
        ("\u{85}a.desktop", None),
    ] {
        let run = gio_run(line);
        let apps = native_file_applications(
            "/file",
            &signal(),
            &xdg(root.path().to_str().unwrap(), &run),
        )
        .await
        .unwrap();
        assert_eq!(apps.first().map(|a| a.name.as_str()), expected, "{line:?}");
    }
}

#[tokio::test]
async fn gio_content_type_uses_javascript_whitespace() {
    for (stdout, mime) in [
        ("standard::content-type:\u{feff}text/plain", "text/plain"),
        (
            "standard::content-type:\u{85}text/plain",
            "\u{85}text/plain",
        ),
    ] {
        let run = Mock::new(move |command, _, _, _| {
            Ok(output(if command == "gio" {
                stdout
            } else {
                "No applications found"
            }))
        });
        native_file_applications("/file", &signal(), &facts("linux", &run))
            .await
            .unwrap();
        assert_eq!(run.calls()[1].args[3], mime);
    }
}

#[tokio::test]
async fn desktop_ids_use_byte_sorted_readdir_independent_of_creation_order() {
    for reversed in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut entries = [("a/b-c.desktop", "First"), ("a-b/c.desktop", "Second")];
        if reversed {
            entries.reverse();
        }
        for (id, name) in entries {
            application(root.path(), id, name).await;
        }
        let run = gio_run("  a-b-c.desktop");
        let apps = native_file_applications(
            "/file",
            &signal(),
            &xdg(root.path().to_str().unwrap(), &run),
        )
        .await
        .unwrap();
        assert_eq!(apps[0].name, "First");
        assert_eq!(
            apps[0].id,
            root.path()
                .join("applications/a/b-c.desktop")
                .to_str()
                .unwrap()
        );
    }
}

#[tokio::test]
async fn reveal_uses_node_dirname_for_root_and_dot_segments() {
    for (path, parent) in [("/", "/"), ("/a/.", "/a")] {
        let run = Mock::output("");
        reveal_native_path(path, &signal(), &facts("linux", &run))
            .await
            .unwrap();
        assert_eq!(run.calls(), [call("xdg-open", &[parent], Window::Hidden)]);
    }
}

#[tokio::test]
async fn xdg_roots_with_extra_slashes_produce_the_same_application_id() {
    let root = tempfile::tempdir().unwrap();
    application(root.path(), "a.desktop", "A").await;
    let run = gio_run("  a.desktop");
    let regular = native_file_applications(
        "/file",
        &signal(),
        &xdg(root.path().to_str().unwrap(), &run),
    )
    .await
    .unwrap();
    let doubled = native_file_applications(
        "/file",
        &signal(),
        &xdg(&format!("{}//", root.path().display()), &run),
    )
    .await
    .unwrap();
    assert_eq!(regular, doubled);
    assert_eq!(
        desktop_data_directories("/home//user", &env(&[]))[0],
        "/home/user/.local/share"
    );
}

#[tokio::test]
async fn explorer_matches_all_136_node_22_targets() {
    #[derive(Deserialize)]
    struct Case {
        path: String,
        target: String,
    }
    let mut count = 0;
    let mut differences = Vec::new();
    for line in include_str!("fixtures/explorer_target.jsonl").lines() {
        let case: Case = serde_json::from_str(line).unwrap();
        let run = Mock::output("");
        open_native_path(&case.path, &signal(), &facts("win32", &run))
            .await
            .unwrap();
        let expected = call("explorer.exe", &[&case.target], Window::Visible);
        if run.calls() != [expected.clone()] {
            differences.push((case.path, run.calls(), expected));
        }
        count += 1;
    }
    assert_eq!(count, 136);
    assert!(differences.is_empty(), "{differences:#?}");
}

#[tokio::test]
async fn spawn_failure_texts_match_node_for_non_executable_and_under_a_file() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    tokio::fs::write(&file, "fixture").await.unwrap();
    tokio::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
        .await
        .unwrap();
    for (command, code, message) in [
        (
            file.clone(),
            "EACCES",
            format!("spawn {} EACCES", file.display()),
        ),
        (file.join("x"), "ENOTDIR", "spawn ENOTDIR".into()),
    ] {
        let error = run_native_command(command.to_str().unwrap(), &[], &signal(), Window::Hidden)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), message);
        assert_eq!(detail(&error).code, Some(CommandCode::Name(code.into())));
        if code == "ENOTDIR" {
            assert_eq!(detail(&error).stdout, None);
            assert_eq!(detail(&error).stderr, None);
        } else {
            assert_eq!(detail(&error).stdout.as_deref(), Some(""));
            assert_eq!(detail(&error).stderr.as_deref(), Some(""));
        }
    }
}

#[tokio::test]
async fn exit_status_3_keeps_the_exact_node_command_error_text() {
    let error = run_native_command(
        "/bin/sh",
        &["-c".into(), "echo out; echo err >&2; exit 3".into()],
        &signal(),
        Window::Hidden,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Command failed: /bin/sh -c echo out; echo err >&2; exit 3\nerr\n"
    );
    assert_eq!(detail(&error).stdout.as_deref(), Some("out\n"));
    assert_eq!(detail(&error).stderr.as_deref(), Some("err\n"));
    assert_eq!(detail(&error).code, Some(CommandCode::Exit(3)));
}

#[tokio::test]
async fn vanished_desktop_file_reports_node_open_error_text() {
    let root = tempfile::tempdir().unwrap();
    tokio::fs::create_dir(root.path().join("applications"))
        .await
        .unwrap();
    let target = root.path().join("gone");
    tokio::fs::write(&target, "[Desktop Entry]\nName=A")
        .await
        .unwrap();
    let entry = root.path().join("applications/a.desktop");
    symlink(&target, &entry).unwrap();
    tokio::fs::remove_file(&target).await.unwrap();
    let run = gio_run("  a.desktop");
    let error = native_file_applications(
        "/file",
        &signal(),
        &xdg(root.path().to_str().unwrap(), &run),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "ENOENT: no such file or directory, open '{}'",
            entry.display()
        )
    );
}

#[tokio::test]
async fn non_unicode_os_environment_is_lossy_through_the_public_fact_seam() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let environment = host_environment::environment_from_os([
        (
            OsString::from("BROWSER"),
            OsString::from_vec(b"browser-\xff".to_vec()),
        ),
        (
            OsString::from_vec(b"invalid-\xff".to_vec()),
            OsString::from_vec(b"value-\xff".to_vec()),
        ),
        (OsString::from("DISPLAY"), OsString::from(":0")),
    ]);
    assert_eq!(environment["invalid-�"], "value-�");
    let run = Mock::output("");
    let mut f = facts("linux", &run);
    f.env = Some(environment.clone());
    assert!(!is_wsl(&HostEnvironmentFacts {
        platform: Some("linux".into()),
        env: Some(environment),
        os_release: Some("generic".into())
    }));
    assert!(can_open_native_path(&f));
    assert_eq!(native_file_manager(&f), Some(NativeFileManager::Directory));
    let request = signal();
    open_native_path("/work/report.html", &request, &f)
        .await
        .unwrap();
    open_native_associated_path("/work/report.html", &request, &f)
        .await
        .unwrap();
    open_native_text_file("/work/settings.yml", &request, &f)
        .await
        .unwrap();
    reveal_native_path("/work/file.txt", &request, &f)
        .await
        .unwrap();
    assert_eq!(
        run.calls(),
        [
            call("browser-�", &["/work/report.html"], Window::Hidden),
            call("xdg-open", &["/work/report.html"], Window::Hidden),
            call("xdg-open", &["/work/settings.yml"], Window::Hidden),
            call("xdg-open", &["/work"], Window::Hidden)
        ]
    );
    run.assert_signals(&request);
}

#[tokio::test]
async fn terminated_child_has_a_null_code_and_node_signal_name() {
    let error = run_native_command(
        "/bin/sh",
        &["-c".into(), "kill -TERM $$".into()],
        &signal(),
        Window::Hidden,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Command failed: /bin/sh -c kill -TERM $$\n"
    );
    assert_eq!(detail(&error).code, None);
    assert_eq!(detail(&error).signal.as_deref(), Some("SIGTERM"));
    assert_eq!(detail(&error).stdout.as_deref(), Some(""));
    assert_eq!(detail(&error).stderr.as_deref(), Some(""));
}

#[tokio::test]
async fn helper_json_uses_the_shared_javascript_parser_and_errors() {
    let missing = Mock::output("[");
    let error = native_file_applications("/file", &signal(), &facts("darwin", &missing))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Unexpected end of JSON input");
    let nested = format!(
        "[{{\"id\":\"/A.app\",\"name\":\"Old\",\"name\":\"New\",\"default\":false,\"icon\":null,\"bundle\":null,\"extra\":{}0{}}}]",
        "[".repeat(160),
        "]".repeat(160)
    );
    let run = Mock::output(&nested);
    let applications = native_file_applications("/file", &signal(), &facts("darwin", &run))
        .await
        .unwrap();
    assert_eq!(
        applications,
        [NativeFileApplication {
            id: "/A.app".into(),
            name: "New".into(),
            default: false,
            icon: None
        }]
    );
}
