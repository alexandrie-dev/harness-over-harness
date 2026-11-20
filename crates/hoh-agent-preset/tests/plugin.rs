use hoh_agent_preset::{AgentPreset, Config, PACKAGE_NAME, register_plugin};
use hoh_agent_preset_registry::{
    AgentPresetRegistry, AgentPresets, Config as RegistryConfig, PresetRuntime,
};
use hoh_kernel::{Kernel, PluginDef};
use hoh_loader::{EvalEnv, Expr, PluginRegistry};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

fn runtime() -> PresetRuntime {
    PresetRuntime::new(
        Arc::new(PluginRegistry::default()),
        EvalEnv {
            home: PathBuf::from("/fixture/home"),
            cwd: PathBuf::from("/fixture/work"),
            exe: PathBuf::from("/fixture/hoh"),
            platform: "linux",
            var: Arc::new(|_| None),
        },
    )
}

async fn fixture() -> (Kernel, hoh_kernel::Context, AgentPresetRegistry) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = AgentPresetRegistry::new(
        &ctx,
        RegistryConfig {
            default: "standard".into(),
            selected_default: Default::default(),
        },
        runtime(),
    )
    .unwrap();
    ctx.provide::<AgentPresets>(Arc::new(registry.clone()))
        .await
        .unwrap();
    (kernel, ctx, registry)
}

#[test]
fn config_schema_requires_an_id_and_a_plugin_array() {
    let schema = PluginDef::of::<AgentPreset>().schema().clone();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["plugins"]["type"], "array");
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&json!("id")));
    assert!(required.contains(&json!("plugins")));

    assert!(serde_json::from_value::<Config>(json!({"plugins":[]})).is_err());
    assert!(serde_json::from_value::<Config>(json!({"id":"standard","plugins":{}})).is_err());
}

#[tokio::test]
async fn registers_all_fields_and_unregisters_with_its_context() {
    let (kernel, ctx, registry) = fixture().await;
    let fiber = ctx
        .plugin::<AgentPreset>(json!({
            "id":"standard",
            "name":"Standard",
            "description":"General-purpose preset",
            "order":1.5,
            "plugins":[]
        }))
        .unwrap();
    fiber.settled().await.unwrap();

    let roster = registry.remote_export_list().await;
    assert_eq!(roster.presets.len(), 1);
    let row = &roster.presets[0];
    assert_eq!(row.id, "standard");
    assert_eq!(row.name.as_deref(), Some("Standard"));
    assert_eq!(row.description.as_deref(), Some("General-purpose preset"));
    assert_eq!(row.order, Some(1.5));
    assert!(row.is_default);

    fiber.dispose().await;
    assert!(registry.remote_export_list().await.presets.is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn propagates_duplicate_registration_without_removing_the_first_row() {
    let (kernel, ctx, registry) = fixture().await;
    let first = ctx
        .plugin::<AgentPreset>(json!({"id":"standard","plugins":[]}))
        .unwrap();
    first.settled().await.unwrap();
    let second = ctx
        .plugin::<AgentPreset>(json!({"id":"standard","plugins":[]}))
        .unwrap();
    let error = second.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Duplicate agent preset: standard")
    );
    assert_eq!(registry.remote_export_list().await.presets.len(), 1);

    first.dispose().await;
    second.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn loader_registration_exposes_schema_resources_and_deferred_child_rows() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let mut registry = PluginRegistry::default();
    register_plugin(&mut registry);
    assert!(registry.resolve(PACKAGE_NAME).is_some());
    assert_eq!(
        registry.schema(PACKAGE_NAME).unwrap()["properties"]["plugins"]["type"],
        "array"
    );

    let env = EvalEnv {
        home: PathBuf::from("/fixture/home"),
        cwd: PathBuf::from("/fixture/work"),
        exe: PathBuf::from("/fixture/hoh"),
        platform: "linux",
        var: Arc::new(|_| None),
    };
    let expression = format!(
        "process.getBuiltinModule('node:path').join(process.getBuiltinModule('node:path').dirname(process.getBuiltinModule('node:module').createRequire(baseUrl).resolve('{PACKAGE_NAME}/package.json')), 'skills')"
    );
    let resource = Expr::parse(&expression)
        .unwrap()
        .eval(&env, &ctx, &registry, &env.cwd)
        .unwrap()
        .unwrap();
    assert_eq!(
        PathBuf::from(resource.as_str().unwrap()),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skills")
    );
    kernel.shutdown().await;
}
