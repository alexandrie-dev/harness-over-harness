mod common;
use base64::{Engine, engine::general_purpose::STANDARD};
use common::*;
use hoh_native_command::{runner::*, *};
use serde_json::{Value, json};

fn application() -> Value {
    json!({"id":"/Applications/Music.app","name":"Music","default":true,"icon":null})
}
fn copy(id: &str, name: &str, bundle: Option<&str>, version: Option<&str>, default: bool) -> Value {
    json!({"id":id,"name":name,"default":default,"icon":null,"bundle":bundle,"version":version})
}
#[tokio::test]
async fn passes_file_paths_as_arguments_and_preserves_the_desktop_default() {
    let run = Mock::output(&json!([application()]).to_string());
    let path = "/tmp/中文 $(touch nope).mp3";
    let apps = native_file_applications(path, &Signal::default(), &facts("darwin", &run))
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(apps).unwrap(), json!([application()]));
    let calls = run.calls();
    assert_eq!(calls[0].command, "osascript");
    assert_eq!(&calls[0].args[..3], ["-l", "JavaScript", "-e"]);
    assert_eq!(&calls[0].args[4..], [path, "icons"]);
    assert_eq!(calls[0].window, Window::Hidden);
}
#[tokio::test]
async fn rejects_each_malformed_native_output() {
    for stdout in ["{}".to_owned(),"[null]".into(),"[{\"id\":1}]".into(),"[{\"id\":\"/a.app\",\"default\":false,\"icon\":null,\"bundle\":\"b\"}]".into(),"[{\"id\":\"/a.app\",\"name\":\"A\",\"default\":false,\"icon\":null,\"bundle\":5}]".into(),json!([{ "icon":"javascript:alert(1)","id":"/Applications/Music.app","name":"Music","default":true }]).to_string()]{
        let run=Mock::output(&stdout);assert!(native_file_applications("/file.mp3",&Signal::default(),&facts("darwin",&run)).await.is_err());
    }
}
#[tokio::test]
async fn collapses_duplicate_copies_of_one_application_to_its_default_or_newest_install() {
    let values = vec![
        copy(
            "/Applications/Quark.app",
            "夸克",
            Some("com.quark.desktop"),
            Some("7.1.5"),
            true,
        ),
        copy(
            "/Applications/Doubao.app",
            "豆包",
            Some("com.bot.pc.doubao"),
            Some("2.18.12"),
            false,
        ),
        copy(
            "/Applications/Xcode.app",
            "Xcode",
            Some("com.apple.dt.Xcode"),
            Some("26.2"),
            false,
        ),
        copy(
            "/Applications/Xcode-26.0.0.app",
            "Xcode-26.0.0",
            Some("com.apple.dt.Xcode"),
            Some("26.0"),
            false,
        ),
        copy(
            "/stale/WeChat.app",
            "WeChat",
            Some("com.tencent.xinWeChat"),
            Some("4.0"),
            false,
        ),
        copy(
            "/updates/Quark.app",
            "夸克",
            Some("com.quark.desktop"),
            Some("9.9"),
            false,
        ),
        copy(
            "/versions/Doubao.app",
            "豆包",
            Some("com.bot.pc.doubao"),
            Some("2.25.16"),
            false,
        ),
        copy(
            "/Applications/WeChat.app",
            "WeChat",
            Some("com.tencent.xinWeChat"),
            Some("3.9"),
            true,
        ),
        copy("/script-a.app", "Script", None, None, false),
        copy("/script-b.app", "Script", None, None, false),
    ];
    let run = Mock::output(&json!(values).to_string());
    let apps = native_file_applications("/file.txt", &Signal::default(), &facts("darwin", &run))
        .await
        .unwrap();
    assert_eq!(
        apps.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        [
            "/Applications/Quark.app",
            "/versions/Doubao.app",
            "/Applications/Xcode.app",
            "/Applications/Xcode-26.0.0.app",
            "/Applications/WeChat.app",
            "/script-a.app",
            "/script-b.app"
        ]
    );
    assert!(apps[4].default);
    for app in serde_json::to_value(apps).unwrap().as_array().unwrap() {
        assert!(app.get("bundle").is_none());
        assert!(app.get("version").is_none());
    }
}
#[tokio::test]
async fn keeps_the_first_copy_on_version_ties_and_prefers_any_version_over_none() {
    let mut values = Vec::new();
    for (id, bundle, version) in [
        ("/a/H.app", "h", Some("1.0")),
        ("/b/H.app", "h", Some("1.0.0")),
        ("/a/N.app", "n", None),
        ("/b/N.app", "n", None),
        ("/a/V.app", "v", Some("")),
        ("/b/V.app", "v", Some("0.1")),
        ("/a/W.app", "w", Some("0.1")),
        ("/b/W.app", "w", None),
    ] {
        values.push(copy(id, bundle, Some(bundle), version, false));
    }
    let run = Mock::output(&json!(values).to_string());
    let apps = native_file_applications("/file.txt", &Signal::default(), &facts("darwin", &run))
        .await
        .unwrap();
    assert_eq!(
        apps.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["/a/H.app", "/a/N.app", "/b/V.app", "/a/W.app"]
    );
}
#[tokio::test]
async fn rejects_a_malformed_copy_even_when_deduplication_would_discard_it() {
    let mut invalid = copy(
        "/updates/Music.app",
        "Music",
        Some("com.apple.Music"),
        Some("1.0"),
        false,
    );
    invalid.as_object_mut().unwrap().remove("default");
    let run = Mock::output(
        &json!([
            copy(
                "/Applications/Music.app",
                "Music",
                Some("com.apple.Music"),
                Some("2.0"),
                true
            ),
            invalid
        ])
        .to_string(),
    );
    assert_eq!(
        native_file_applications("/file.mp3", &Signal::default(), &facts("darwin", &run))
            .await
            .unwrap_err()
            .message,
        "Invalid native application entry"
    );
}
#[tokio::test]
async fn launches_only_a_currently_registered_application_with_argv() {
    let run = Mock::output(&json!([application()]).to_string());
    let f = facts("darwin", &run);
    let signal = Signal::default();
    open_native_file_application("/file.mp3", "/Applications/Music.app", &signal, &f)
        .await
        .unwrap();
    assert_eq!(run.calls()[0].args.last().unwrap(), "handlers");
    assert_eq!(
        run.calls().last().unwrap(),
        &call(
            "open",
            &["-a", "/Applications/Music.app", "/file.mp3"],
            Window::Hidden
        )
    );
    run.calls.lock().unwrap().clear();
    assert!(
        open_native_file_application("/file.mp3", "/arbitrary.app", &signal, &f)
            .await
            .unwrap_err()
            .message
            .contains("not registered")
    );
    assert_eq!(run.calls().len(), 1);
}
#[tokio::test]
async fn launches_a_registered_copy_that_deduplication_collapses_out_of_the_display_list() {
    let run = Mock::output(
        &json!([
            copy(
                "/Applications/Music.app",
                "Music",
                Some("com.apple.Music"),
                Some("2.0"),
                true
            ),
            copy(
                "/updates/Music.app",
                "Music",
                Some("com.apple.Music"),
                Some("1.0"),
                false
            )
        ])
        .to_string(),
    );
    let f = facts("darwin", &run);
    let signal = Signal::default();
    let shown = native_file_applications("/file.mp3", &signal, &f)
        .await
        .unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].id, "/Applications/Music.app");
    open_native_file_application("/file.mp3", "/updates/Music.app", &signal, &f)
        .await
        .unwrap();
    assert_eq!(
        run.calls().last().unwrap(),
        &call(
            "open",
            &["-a", "/updates/Music.app", "/file.mp3"],
            Window::Hidden
        )
    );
}
#[tokio::test]
async fn does_not_query_after_cancellation_or_on_unsupported_platforms() {
    let run = Mock::output("");
    assert!(
        native_file_applications("/file.mp3", &Signal::default(), &facts("freebsd", &run))
            .await
            .unwrap()
            .is_empty()
    );
    let signal = Signal::default();
    signal.abort("closed");
    assert!(
        native_file_applications("/file.mp3", &signal, &facts("darwin", &run))
            .await
            .is_err()
    );
    assert!(run.calls().is_empty());
}
#[tokio::test]
async fn uses_the_production_command_adapter_and_current_platform_when_no_override_is_supplied() {
    let run = Mock::new(|cmd, _, _, _| {
        Ok(output(if cmd == "gio" {
            "standard::content-type: audio/mpeg"
        } else if cmd == "env" {
            "No applications found"
        } else {
            "[{\"id\":\"/Applications/Music.app\",\"name\":\"Music\",\"default\":true,\"icon\":null}]"
        }))
    });
    let f = PathOpenerInternals {
        run: Some(&run),
        os_release: Some("generic".into()),
        env: Some(env(&[])),
        ..Default::default()
    };
    let apps = native_file_applications("/file.mp3", &Signal::default(), &f)
        .await
        .unwrap();
    assert_eq!(apps.len(), if cfg!(target_os = "linux") { 0 } else { 1 });
    open_native_file_application(
        "/file.mp3",
        "/Applications/Music.app",
        &Signal::default(),
        &facts("darwin", &run),
    )
    .await
    .unwrap();
    assert_eq!(run.calls().last().unwrap().command, "open");
    assert!(PathOpenerInternals::default().run.is_none());
}
#[tokio::test]
async fn encodes_windows_query_and_invocation_data_separately_from_native_adapter_source() {
    let scripts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let held = scripts.clone();
    let path = "C:\\测试\\a'; write-host nope.mp3";
    let run = Mock::new(move |_, args, _, _| {
        held.lock()
            .unwrap()
            .push(std::fs::read_to_string(args.last().unwrap()).unwrap());
        Ok(output(&json!([application()]).to_string()))
    });
    let signal = Signal::default();
    let f = facts("win32", &run);
    let apps = native_file_applications(path, &signal, &f).await.unwrap();
    assert_eq!(serde_json::to_value(apps).unwrap(), json!([application()]));
    open_native_file_application(path, "/Applications/Music.app", &signal, &f)
        .await
        .unwrap();
    let scripts = scripts.lock().unwrap();
    assert!(scripts[0].contains(&STANDARD.encode(path)));
    assert!(!scripts[0].contains(path));
    assert!(scripts[0].contains("::List($path)"));
    assert!(scripts[1].contains("::Open($path, $application)"));
    assert!(scripts[1].starts_with('\u{feff}'));
    let calls = run.calls();
    assert_eq!(calls[1].command, "powershell.exe");
    assert_eq!(
        &calls[1].args[..6],
        [
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-ExecutionPolicy",
            "Bypass",
            "-File"
        ]
    );
    for call in calls {
        assert!(!std::path::Path::new(call.args.last().unwrap()).exists());
    }
}
#[tokio::test]
async fn keeps_the_complete_windows_command_line_below_the_create_process_limit_for_long_multibyte_paths()
 {
    let run = Mock::output("[]");
    let paths = [
        format!("C:\\work\\{}.txt", "测".repeat(210)),
        format!("C:\\work\\{}.txt", "测".repeat(3000)),
    ];
    for path in &paths {
        native_file_applications(path, &Signal::default(), &facts("win32", &run))
            .await
            .unwrap();
    }
    let calls = run.calls();
    assert_eq!(calls.len(), paths.len());
    for (call, path) in calls.iter().zip(paths) {
        let line = format!("{} {}", call.command, call.args.join(" "));
        assert!(line.encode_utf16().count() < 1000);
        assert!(line.contains("powershell.exe"));
        assert!(line.contains("-File"));
        assert!(!line.contains(&path));
        assert!(!line.contains(&STANDARD.encode(path)));
    }
}
#[tokio::test]
async fn uses_the_windows_desktop_for_wsl_paths_and_rejects_an_empty_translation() {
    let run = Mock::new(|cmd, _, _, _| {
        Ok(output(if cmd == "wslpath" {
            "C:\\音频.mp3\n"
        } else {
            "[{\"id\":\"/Applications/Music.app\",\"name\":\"Music\",\"default\":true,\"icon\":null}]"
        }))
    });
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    let apps = native_file_applications("/mnt/c/音频.mp3", &Signal::default(), &f)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(apps).unwrap(), json!([application()]));
    open_native_file_application(
        "/mnt/c/音频.mp3",
        "/Applications/Music.app",
        &Signal::default(),
        &f,
    )
    .await
    .unwrap();
    assert!(
        run.calls()
            .contains(&call("wslpath", &["-w", "/mnt/c/音频.mp3"], Window::Hidden))
    );
    let empty = Mock::output("");
    f.run = Some(&empty);
    assert!(
        native_file_applications("/a", &Signal::default(), &f)
            .await
            .unwrap_err()
            .message
            .contains("no Windows path")
    );
}
#[tokio::test]
async fn uses_production_environment_and_runner_defaults_for_linux_and_wsl() {
    let run = Mock::new(|cmd, _, _, _| {
        Ok(output(match cmd {
            "gio" => "standard::content-type: audio/mpeg",
            "env" => "No applications found",
            "wslpath" => "C:\\file.mp3\n",
            _ => {
                "[{\"id\":\"/Applications/Music.app\",\"name\":\"Music\",\"default\":true,\"icon\":null}]"
            }
        }))
    });
    let f = PathOpenerInternals {
        platform: Some("linux".into()),
        os_release: Some("linux".into()),
        run: Some(&run),
        ..Default::default()
    };
    assert!(
        native_file_applications("/file.mp3", &Signal::default(), &f)
            .await
            .unwrap()
            .is_empty()
    );
    let mut f = facts("linux", &run);
    f.os_release = Some("microsoft".into());
    assert_eq!(
        native_file_applications("/file.mp3", &Signal::default(), &f)
            .await
            .unwrap()
            .len(),
        1
    );
}
