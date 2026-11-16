mod common;
use common::*;
use hoh_native_command::{desktop_application_icon, runner::*, *};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

struct Fixture {
    root: tempfile::TempDir,
    run: Mock,
    mode: Arc<AtomicU8>,
}
async fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(root.path().join("applications/nested"))
        .await
        .unwrap();
    tokio::fs::create_dir(root.path().join("pixmaps"))
        .await
        .unwrap();
    tokio::fs::write(root.path().join("applications/player.desktop"),"[Desktop Entry]\nName=Player\nName[zh_CN]=播放器\nIcon=player\nExec=player %f\n[Desktop Action Extra]\nName=Wrong\n").await.unwrap();
    tokio::fs::write(
        root.path().join("applications/nested/other.desktop"),
        "[Desktop Entry]\nName=Other\nExec=other %f\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("pixmaps/player.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .await
    .unwrap();
    let mode = Arc::new(AtomicU8::new(0));
    let held = mode.clone();
    let run = Mock::new(move |command, args, _, _| {
        Ok(output(match held.load(Ordering::SeqCst) {
            2 => "no content type",
            _ if command == "gio" && args[0] == "info" => "  standard::content-type: audio/mpeg\n",
            1 if command == "env" => {
                "Registered applications:\n  player.desktop\n  unnamed.desktop\n  nested-other.desktop\n"
            }
            _ if command == "env" => {
                "Default application for audio/mpeg: player.desktop\nRegistered applications:\n  player.desktop\n  nested-other.desktop\n  missing.desktop\nRecommended applications:\n  player.desktop\n"
            }
            _ => "",
        }))
    });
    Fixture { root, run, mode }
}
fn internals(f: &Fixture, locale: bool) -> PathOpenerInternals<'_> {
    let mut facts = facts("linux", &f.run);
    facts.env = Some(env(&[
        ("XDG_DATA_HOME", f.root.path().to_str().unwrap()),
        ("XDG_DATA_DIRS", ""),
        ("LANG", if locale { "zh_CN.UTF-8" } else { "" }),
    ]));
    facts
}
#[tokio::test]
async fn deduplicates_gio_handlers_resolves_nested_ids_localizes_names_and_reuses_desktop_artwork()
{
    let f = fixture().await;
    let signal = signal();
    let facts = internals(&f, true);
    let apps = native_file_applications("/file.mp3", &signal, &facts)
        .await
        .unwrap();
    f.run.assert_signals(&signal);
    assert_eq!(apps.len(), 2);
    assert_eq!(
        apps[0].id,
        f.root
            .path()
            .join("applications/player.desktop")
            .to_str()
            .unwrap()
    );
    assert_eq!(apps[0].name, "播放器");
    assert!(apps[0].default);
    assert!(
        apps[0]
            .icon
            .as_ref()
            .unwrap()
            .starts_with("data:image/svg+xml;base64,")
    );
    assert_eq!(
        apps[1],
        NativeFileApplication {
            id: f
                .root
                .path()
                .join("applications/nested/other.desktop")
                .to_string_lossy()
                .into_owned(),
            name: "Other".into(),
            default: false,
            icon: None
        }
    );
    open_native_file_application("/file.mp3", &apps[0].id, &signal, &facts)
        .await
        .unwrap();
    f.run.assert_signals(&signal);
    assert_eq!(
        f.run.calls().last().unwrap(),
        &call("gio", &["launch", &apps[0].id, "/file.mp3"], Window::Hidden)
    );
    assert!(
        open_native_file_application("/file.mp3", "/unregistered.desktop", &signal, &facts)
            .await
            .unwrap_err()
            .to_string()
            .contains("not registered")
    );
    assert_eq!(
        f.run.calls()[0],
        call(
            "gio",
            &["info", "-a", "standard::content-type", "/file.mp3"],
            Window::Hidden
        )
    );
    assert_eq!(
        f.run.calls()[1],
        call(
            "env",
            &["LC_ALL=C", "gio", "mime", "audio/mpeg"],
            Window::Hidden
        )
    );
}
#[tokio::test]
async fn keeps_nondefault_applications_when_the_desktop_has_no_default_and_omits_hidden_or_unnamed_entries()
 {
    let f = fixture().await;
    f.mode.store(1, Ordering::SeqCst);
    tokio::fs::write(
        f.root.path().join("applications/player.desktop"),
        "[Desktop Entry]\nHidden=true\nName=Hidden\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        f.root.path().join("applications/unnamed.desktop"),
        "[Desktop Entry]\nExec=unnamed\n",
    )
    .await
    .unwrap();
    let apps = native_file_applications("/file.mp3", &signal(), &internals(&f, true))
        .await
        .unwrap();
    assert_eq!(
        apps,
        [NativeFileApplication {
            id: f
                .root
                .path()
                .join("applications/nested/other.desktop")
                .to_string_lossy()
                .into_owned(),
            name: "Other".into(),
            default: false,
            icon: None
        }]
    );
}
#[tokio::test]
async fn rejects_missing_content_type_metadata_instead_of_fabricating_associations() {
    let f = fixture().await;
    f.mode.store(2, Ordering::SeqCst);
    assert!(
        native_file_applications("/file.mp3", &signal(), &internals(&f, true))
            .await
            .unwrap_err()
            .to_string()
            .contains("content type")
    );
}
#[tokio::test]
async fn tolerates_absent_desktop_roots_and_directory_shaped_icon_paths_without_a_locale_override()
{
    let f = fixture().await;
    let icon = f.root.path().join("folder.png");
    tokio::fs::create_dir(&icon).await.unwrap();
    assert_eq!(
        desktop_application_icon(icon.to_str().unwrap(), &[]).await,
        None
    );
    let mut facts = internals(&f, false);
    facts.env.as_mut().unwrap().insert(
        "XDG_DATA_DIRS".into(),
        f.root.path().join("absent").to_string_lossy().into_owned(),
    );
    let apps = native_file_applications("/file.mp3", &signal(), &facts)
        .await
        .unwrap();
    assert_eq!(apps[0].name, "Player");
}
#[tokio::test]
async fn preserves_main_section_field_order_and_icon_precedence() {
    let fields = desktop_entry_fields(
        "Name=outside\r\n[Desktop Entry]\r\n# comment\r\n Name = Main \r\nIcon=first\nIcon=second\n[Other]\nName=Wrong",
    );
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Name", "Icon"]
    );
    assert_eq!(fields["Name"], "Main");
    assert_eq!(fields["Icon"], "second");
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("icons/hicolor/512x512/apps");
    tokio::fs::create_dir_all(&path).await.unwrap();
    tokio::fs::write(path.join("app.png"), b"png")
        .await
        .unwrap();
    tokio::fs::write(path.join("app.svg"), b"svg")
        .await
        .unwrap();
    let icon = desktop_application_icon("app", &[root.path().to_string_lossy().into_owned()])
        .await
        .unwrap();
    assert_eq!(icon.bytes, b"png");
    assert_eq!(icon.content_type, "image/png");
    assert_eq!(
        desktop_data_directories("/home/test", &env(&[])),
        ["/home/test/.local/share", "/usr/local/share", "/usr/share"]
    );
    assert_eq!(
        desktop_data_directories(
            "/home/test",
            &env(&[("XDG_DATA_HOME", ""), ("XDG_DATA_DIRS", "a::b:")])
        ),
        ["", "a", "b"]
    );
}
