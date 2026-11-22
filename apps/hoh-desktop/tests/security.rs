use hoh_desktop::native::{
    approval_navigation_allowed, deny_new_window, legacy_shell_requested, main_navigation_allowed,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn capability(source: &str) -> Value {
    serde_json::from_str(source).unwrap()
}

fn strings(value: &Value, key: &str) -> BTreeSet<String> {
    value[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn capabilities_keep_main_and_approval_authority_disjoint() {
    let main = capability(include_str!("../capabilities/main.json"));
    let approval = capability(include_str!("../capabilities/approval.json"));
    let main_permissions = strings(&main, "permissions");
    let approval_permissions = strings(&approval, "permissions");
    assert!(!main_permissions.contains("allow-approval-current"));
    // The T3 client answers the permission request shown in its conversation (founder 22.11 19:16);
    // the host accepts from the main window only that request (`Broker::decide_in_chat`).
    assert!(main_permissions.contains("allow-approval-decide"));
    assert!(main_permissions.contains("allow-question-answer"));
    assert!(main_permissions.contains("allow-open-link"));
    assert!(main_permissions.contains("allow-browse-folders"));
    assert!(!approval_permissions.contains("allow-browse-folders"));
    assert!(!approval_permissions.contains("allow-open-link"));
    assert!(!approval_permissions.contains("allow-question-answer"));
    assert!(main_permissions.contains("allow-approval-history"));
    assert!(!approval_permissions.contains("allow-approval-history"));
    assert_eq!(
        approval_permissions,
        BTreeSet::from([
            "allow-approval-current".to_owned(),
            "allow-approval-decide".to_owned(),
        ])
    );
    assert_eq!(strings(&main, "windows"), BTreeSet::from(["main".into()]));
    assert_eq!(
        strings(&approval, "windows"),
        BTreeSet::from(["approval-*".into()])
    );
    assert!(!"main".starts_with("approval-"));
}

#[test]
fn production_build_excludes_probe_runtime_switches() {
    assert!(!hoh_desktop::probe_compiled());
    let entrypoint = include_str!("../src/main.rs");
    let native = include_str!("../src/native.rs");
    for runtime_switch in ["HOH_APPROVAL_PROBE", "HOH_WEBKIT_PROBE"] {
        assert!(!entrypoint.contains(runtime_switch), "{runtime_switch}");
        assert!(!native.contains(runtime_switch), "{runtime_switch}");
    }
}

#[test]
fn production_vite_input_gates_probe_assets_by_mode() {
    let config = include_str!("../../../ui/vite.config.ts");
    assert!(config.contains("defineConfig(({ mode })"));
    assert!(config.contains("mode === 'probe' ? { probe:"));
}

#[test]
fn main_window_is_created_by_tauri_before_setup() {
    let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert_eq!(config["app"]["windows"][0]["create"], true);
}

#[test]
fn approval_probe_waits_for_ready_and_requires_native_close_marker() {
    let script = include_str!("../probe-approval/approval-probe.js");
    assert!(script.contains("waitUntilReady"));
    assert!(script.contains("hoh/settings/get"));
    let probe = include_str!("../src/probe.rs");
    let native = include_str!("../src/native.rs");
    assert!(probe.contains("APPROVAL_WINDOW_CLOSED.load"));
    assert!(native.contains("mark_approval_window_closed"));
}

#[test]
fn approval_probe_marks_the_stub_as_a_test_engine() {
    let script = include_str!("../scripts/run-approval-probe.sh");
    assert!(script.contains("HOH_TEST_ENGINE="));
    assert!(!script.contains("HOH_ENGINE_PATH="));
}

/// The files a build of the window may hold, as Tauri keys them (a `--mode probe` build adds
/// firstrun.html and probe.html; the shipped one has neither).
fn app_file(path: &str) -> bool {
    [
        "/index.html",
        "/legacy.html",
        "/approval.html",
        "/firstrun.html",
        "/probe.html",
        "/boot-theme.js",
        "/assets/main.js",
        "/t3-assets/index-x.js",
    ]
    .contains(&path)
}

/// The main window of the T3 shell (not HOH_WINDOW_SHELL=legacy).
fn main_allowed(url: &str) -> bool {
    main_navigation_allowed(&url.parse().unwrap(), false, app_file)
}

#[test]
fn navigation_is_exact_origin_and_path_and_new_windows_are_denied() {
    for url in [
        "tauri://localhost/index.html",
        "tauri://localhost/",
        // routes of the T3 client's router after a reload
        "tauri://localhost/settings/general",
        "tauri://localhost/env-1/thread-2",
        "tauri://localhost/hoh-local/thread-3f1c2a9e-6f7b-4c1d-9a8e-2b5d7c0e4f11",
        "tauri://localhost/threads/hoh-local/thread-2",
        "tauri://localhost/draft/$draftId",
        "tauri://localhost/welcome",
    ] {
        assert!(main_allowed(url), "{url}");
    }
    for url in [
        "https://example.test/index.html",
        "https://example.test/settings/general",
        "tauri://evil.test/index.html",
        "tauri://evil.test/settings/general",
        "tauri://localhost/approval.html",
        "tauri://localhost/firstrun.html",
        "tauri://localhost/assets/main.js",
        "tauri://localhost/settings//general",
        "tauri://localhost/settings/../approval.html",
        "http://tauri.localhost/other.html",
        "http://tauri.localhost:4173/index.html",
        "tauri://localhost:4173/index.html",
    ] {
        assert!(!main_allowed(url), "{url}");
    }
    let approval_url = "tauri://localhost/approval.html";
    assert!(
        approval_navigation_allowed(&approval_url.parse().unwrap()),
        "{approval_url}"
    );
    assert!(!approval_navigation_allowed(
        &"tauri://localhost/index.html".parse().unwrap()
    ));
    let response: tauri::webview::NewWindowResponse<tauri::Wry> = deny_new_window();
    assert!(matches!(response, tauri::webview::NewWindowResponse::Deny));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_rejects_http_tauri_localhost_for_every_window() {
    assert!(!main_allowed("http://tauri.localhost/index.html"));
    assert!(!approval_navigation_allowed(
        &"http://tauri.localhost/approval.html".parse().unwrap()
    ));
}

#[cfg(any(windows, target_os = "android"))]
#[test]
fn platform_http_tauri_origin_remains_available_where_tauri_uses_it() {
    assert!(main_allowed("http://tauri.localhost/index.html"));
    assert!(approval_navigation_allowed(
        &"http://tauri.localhost/approval.html".parse().unwrap()
    ));
}

#[test]
fn tauri_base_origin_is_valid_main_entrypoint() {
    // Tauri canonicalizes App("index.html") to this bare origin before it
    // invokes plugin navigation hooks. Rejecting its empty path blanks main.
    let initial_url: tauri::Url = "tauri://localhost".parse().unwrap();
    assert_eq!(initial_url.path(), "");
    assert!(main_navigation_allowed(&initial_url, false, app_file));
}

/// Review 222000: Tauri decodes `%XX` and serves `{p}`, `{p}.html`, `{p}/index.html` before it
/// falls back to index.html, so a path that is not a file by its name could still open one. None
/// of these may load in the main window; the routes of T3 still do (above).
#[test]
fn no_path_opens_another_page_of_the_app_in_the_main_window() {
    for path in [
        "/approval",
        "/approval/",
        "/firstrun",
        "/firstrun/",
        "/probe",
        "/legacy",
        "/index",
        "/approval%2Ehtml",
        "/approval%2ehtml",
        "/firstrun%2Ehtml",
        "/firstrun%2ehtml",
        "/legacy%2Ehtml",
        "/assets%2Fx",
        "/assets%2Fmain.js",
        "/assets%2fmain%2ejs",
        "/%61pproval",
        "/approva%6C",
        "/approval%2F",
        "/approval%00",
        "/approval%252Ehtml",
        "/settings%2Fgeneral",
        "/settings/..%2Fapproval",
        "/settings/../approval",
        "/settings/%2E%2E/approval",
    ] {
        let url = format!("tauri://localhost{path}");
        assert!(!main_allowed(&url), "{url}");
        assert!(
            !main_navigation_allowed(&url.parse().unwrap(), true, app_file),
            "legacy shell: {url}"
        );
    }
    // The previous shell is the main window's page only when it was asked for.
    assert!(!main_allowed("tauri://localhost/legacy.html"));
    assert!(main_navigation_allowed(
        &"tauri://localhost/legacy.html".parse().unwrap(),
        true,
        app_file
    ));
}

#[test]
fn the_window_opens_the_t3_client_unless_the_previous_shell_is_asked_for() {
    use std::ffi::OsStr;
    assert!(!legacy_shell_requested(None));
    assert!(!legacy_shell_requested(Some(OsStr::new(""))));
    assert!(!legacy_shell_requested(Some(OsStr::new("t3"))));
    assert!(legacy_shell_requested(Some(OsStr::new("legacy"))));
    let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let window = &conf["app"]["windows"][0];
    assert_eq!(window["url"], "index.html");
    assert_eq!(window["title"], "HoH");
}

/// The window never opens a second page: the main window's WebKit `create` handler always answers `None`, and the
/// only thing it does is hand a clicked http(s) link to the system opener (`opener::clicked_link`, tested in
/// tests/opener.rs). The approval window refuses new windows outright.
#[cfg(target_os = "linux")]
#[test]
fn a_new_window_request_is_never_a_window_and_a_link_needs_a_click() {
    let native = include_str!("../src/native.rs");
    let start = native
        .find("connect_create(|_, action|")
        .expect("main window create handler");
    let handler = &native[start..start + 600];
    assert!(
        handler.contains("crate::opener::clicked_link(&url, action.is_user_gesture())"),
        "{handler}"
    );
    assert!(handler.contains("crate::opener::open(&link)"), "{handler}");
    assert!(
        handler.trim_end().contains("None\n                });"),
        "{handler}"
    );
    assert_eq!(
        native.matches("connect_create(").count(),
        1,
        "one create handler, the main window's"
    );
    assert_eq!(native.matches("NewWindowResponse::Allow").count(), 0);
    assert!(native.contains(".on_new_window(|_, _| deny_new_window())"));
}

/// As T3 (`MIN_MAIN_WINDOW_SIZE` 400×360): the window can be narrowed to 800×550 and below.
#[test]
fn the_main_window_may_shrink_as_far_as_in_t3() {
    let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let window = &conf["app"]["windows"][0];
    assert_eq!(
        (window["minWidth"].as_u64(), window["minHeight"].as_u64()),
        (Some(400), Some(360))
    );
}

/// The window icon is offered in sizes from 32 to 512 (`_NET_WM_ICON`), square and RGBA.
#[test]
fn the_window_icon_has_several_sizes() {
    for size in [32u32, 48, 64, 128, 256, 512] {
        let bytes = std::fs::read(format!("icons/{size}x{size}.png")).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), size);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), size);
        assert_eq!(bytes[25], 6, "RGBA");
    }
}
