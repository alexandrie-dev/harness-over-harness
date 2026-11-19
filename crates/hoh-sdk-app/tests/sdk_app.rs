use hoh_loader::{Patch, parse_patches};
use hoh_sdk_app::CORDIS_PATCH;
use serde_json::{Value, json};
use std::path::Path;

fn selected<'a>(patches: &'a [Patch], id: &str) -> &'a Patch {
    patches
        .iter()
        .find(|patch| patch.id.as_ref() == Some(&json!(id)))
        .unwrap_or_else(|| panic!("missing patch for {id}"))
}

#[test]
fn enables_plugin_management_while_keeping_json_rpc_dependencies_stable_until_restart() {
    let patches = parse_patches(CORDIS_PATCH, Path::new("/sdk-app/cordis.patch.yml")).unwrap();
    assert_eq!(
        selected(&patches, "hmr").set.get("disabled"),
        Some(&json!(true))
    );
    assert_eq!(
        selected(&patches, "tool-plugin-manager")
            .set
            .get("disabled"),
        Some(&json!(false))
    );
    assert_eq!(
        selected(&patches, "session-title-llm").set.get("disabled"),
        Some(&json!(true))
    );
    assert_eq!(
        selected(&patches, "system-prompt").set.get("config"),
        Some(&json!({
            "personaPrefix": "You are a coding agent powered by the {{model}} model."
        }))
    );

    let rows = patches
        .iter()
        .filter_map(|patch| patch.insert.as_ref())
        .flatten()
        .collect::<Vec<_>>();
    let startup = rows.iter().find(|row| row.id == "sdk-app-startup").unwrap();
    assert_eq!(startup.name, "@deepseek-ai/dsh-sdk-app");
    let server = rows
        .iter()
        .find(|row| row.id == "sdk-jsonrpc-server")
        .unwrap();
    assert_eq!(server.name, "@deepseek-ai/dsh-sdk-jsonrpc-server");
    assert_eq!(
        server.inject,
        Some(vec!["sdkAppStartup".into(), "loader".into()])
    );
    assert_eq!(
        rows.iter()
            .position(|row| row.id == "sdk-app-startup")
            .unwrap()
            + 1,
        rows.iter()
            .position(|row| row.id == "sdk-jsonrpc-server")
            .unwrap()
    );
    assert!(matches!(&server.config, Value::Object(_)));
}

#[test]
fn keeps_the_source_plugin_service_and_config_shape() {
    use hoh_kernel::{Plugin, ServiceKey};
    assert_eq!(hoh_sdk_app::SdkApp::NAME, "sdk-app-startup");
    assert_eq!(
        hoh_sdk_app::SdkAppStartupKey::NAME,
        hoh_sdk_app::SDK_APP_STARTUP_SERVICE
    );
    assert_eq!(hoh_sdk_app::Config::default().profile, "sdk");
    let parsed: hoh_sdk_app::Config = serde_json::from_value(json!({})).unwrap();
    assert_eq!(parsed.profile, "sdk");
    let schema = serde_json::to_value(schemars::schema_for!(hoh_sdk_app::Config)).unwrap();
    assert_eq!(schema["properties"]["profile"]["default"], "sdk");
}
