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

#[test]
fn navigation_is_exact_origin_and_path_and_new_windows_are_denied() {
    for url in [
        "tauri://localhost/index.html",
        "tauri://localhost/",
        "tauri://localhost/legacy.html",
        // routes of the T3 client's router after a reload
        "tauri://localhost/settings/general",
        "tauri://localhost/env-1/thread-2",
        "tauri://localhost/draft/$draftId",
    ] {
        assert!(main_navigation_allowed(&url.parse().unwrap()), "{url}");
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
        assert!(!main_navigation_allowed(&url.parse().unwrap()), "{url}");
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
    assert!(!main_navigation_allowed(
        &"http://tauri.localhost/index.html".parse().unwrap()
    ));
    assert!(!approval_navigation_allowed(
        &"http://tauri.localhost/approval.html".parse().unwrap()
    ));
}

#[cfg(any(windows, target_os = "android"))]
#[test]
fn platform_http_tauri_origin_remains_available_where_tauri_uses_it() {
    assert!(main_navigation_allowed(
        &"http://tauri.localhost/index.html".parse().unwrap()
    ));
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
    assert!(main_navigation_allowed(&initial_url));
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
