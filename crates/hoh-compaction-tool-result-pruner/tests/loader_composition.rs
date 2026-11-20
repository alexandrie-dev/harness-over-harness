//! Port of loader-composition.spec.ts.

use hoh_compaction_tool_result_pruner::{ResolvedConfig, ToolResultPruner, ToolResultPrunerPlugin};
use hoh_kernel::{Kernel, KernelError};
use hoh_loader::{BootOptions, EvalEnv, PluginRegistry, boot};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_token_meter::TokenMeterPlugin;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn loads_and_resolves_the_flat_yaml_plugin_shape() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("cordis.yml");
    std::fs::write(
        &base,
        [
            "- name: session-projection",
            "- name: token-meter",
            "- name: compaction-tool-result-pruner",
            "  config:",
            "    thresholdChars: 100",
            "    headChars: 20",
            "    tailChars: 10",
            "",
        ]
        .join("\n"),
    )
    .unwrap();
    let kernel = Kernel::start();
    let mut registry = PluginRegistry::default();
    registry
        .add::<SessionProjectionRegistry>("session-projection")
        .add::<TokenMeterPlugin>("token-meter")
        .add::<ToolResultPrunerPlugin>("compaction-tool-result-pruner");
    let root = kernel.root();
    boot(
        &root,
        BootOptions {
            registry: Arc::new(registry),
            base,
            layers: vec![],
            env: EvalEnv {
                home: dir.path().join("home"),
                cwd: dir.path().into(),
                exe: dir.path().join("hoh"),
                platform: "linux",
                var: Arc::new(|_| None),
            },
        },
    )
    .await
    .unwrap();
    let pruner = root.get::<ToolResultPruner>().unwrap();
    assert_eq!(
        pruner.config,
        ResolvedConfig {
            threshold_chars: 100,
            head_chars: 20,
            tail_chars: 10
        }
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_stale_config_after_plugin_schema_normalization() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    // Satisfy the declared injections first: config normalization runs in the
    // plugin body, which a pending fiber never reaches.
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<TokenMeterPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let error: KernelError = ctx
        .plugin::<ToolResultPrunerPlugin>(json!({ "maxChars": 100 }))
        .unwrap()
        .settled()
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unknown key \"maxChars\""),
        "{error}"
    );
    kernel.shutdown().await;
}
