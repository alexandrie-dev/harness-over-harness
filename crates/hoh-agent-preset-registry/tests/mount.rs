mod support;

use hoh_agent_preset_registry::{
    CompositionRowEnablement, active_composition_modules, audit_rows, mount_preset_with_runtime,
    mounted_composition_rows,
};
use hoh_kernel::Kernel;
use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use serde_json::json;
use support::{definition, runtime};

#[tokio::test]
async fn keeps_active_module_order_and_excludes_inactive_disabled_and_group_rows() {
    let kernel = Kernel::start();
    let root = kernel.root();
    let scope = create_scope(&root, ScopeKey::new(), CreateScopeOptions::default()).unwrap();
    let definition = definition(
        "ordered",
        json!([
            {"id":"first","name":"fixture:active"},
            {"id":"group","name":"cordis:group","group":true,"config":[]},
            {"id":"disabled","name":"fixture:active","disabled":true},
            {"id":"waiting","name":"fixture:needs-missing"},
            {"id":"last","name":"fixture:active"}
        ]),
    );
    let mount = mount_preset_with_runtime(&scope.ctx, "ordered", &definition, &runtime())
        .await
        .unwrap();
    let modules: Vec<_> = active_composition_modules(&mount)
        .into_iter()
        .map(|module| module.module_name)
        .collect();
    assert_eq!(modules, ["fixture:active", "fixture:active"]);
    let audit = audit_rows(&mount.tree).await;
    assert_eq!(audit.pending.len(), 1);
    assert!(audit.pending[0].contains("serviceThatDoesNotExist"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_plugin_failures_and_refuses_an_unscoped_mount() {
    let kernel = Kernel::start();
    let root = kernel.root();
    let scope = create_scope(&root, ScopeKey::new(), Default::default()).unwrap();
    let definition = definition(
        "broken",
        json!([
            {"id":"missing","name":"fixture:not-registered"},
            {"id":"throws","name":"fixture:failing"}
        ]),
    );
    let failure = mount_preset_with_runtime(&scope.ctx, "broken", &definition, &runtime())
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(failure.contains("missing (fixture:not-registered): never started"));
    assert!(failure.contains("throws (fixture:failing): refused"));
    let unscoped = mount_preset_with_runtime(&root, "unscoped", &definition, &runtime())
        .await
        .err()
        .unwrap();
    assert!(unscoped.to_string().contains("requires a scope"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn does_not_replace_the_declaring_tree_and_keeps_relative_child_ids() {
    let kernel = Kernel::start();
    let root = kernel.root();
    let first = create_scope(&root, ScopeKey::new(), Default::default()).unwrap();
    let second = create_scope(&root, ScopeKey::new(), Default::default()).unwrap();
    let definition = definition(
        "owned",
        json!([{"id":"child","name":"fixture:active","disabled":true}]),
    );
    let first_mount = mount_preset_with_runtime(&first.ctx, "owned", &definition, &runtime())
        .await
        .unwrap();
    let second_mount =
        mount_preset_with_runtime(&second.ctx, "owned-again", &definition, &runtime())
            .await
            .unwrap();
    assert_eq!(
        mounted_composition_rows(&first_mount)[0]
            .entry_id
            .as_deref(),
        Some("child")
    );
    assert_eq!(
        mounted_composition_rows(&second_mount)[0]
            .entry_id
            .as_deref(),
        Some("child")
    );
    assert_ne!(first_mount.fiber.uid(), second_mount.fiber.uid());
    kernel.shutdown().await;
}

#[tokio::test]
async fn reports_grouped_and_conditional_plugin_rows_from_the_activated_tree() {
    let kernel = Kernel::start();
    let root = kernel.root();
    let scope = create_scope(&root, ScopeKey::new(), Default::default()).unwrap();
    let definition = definition(
        "standard",
        json!([{"name":"cordis:group","group":true,"config":[
            {"id":"off","name":"fixture:not-registered","disabled":{"__jsExpr":"true"}}
        ]}]),
    );
    let mount = mount_preset_with_runtime(&scope.ctx, "standard", &definition, &runtime())
        .await
        .unwrap();
    let rows = mounted_composition_rows(&mount);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry_id.as_deref(), Some("off"));
    assert_eq!(rows[0].enabled, CompositionRowEnablement::Enabled(false));
    assert_eq!(rows[0].condition.as_deref(), Some("true"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn mounts_a_disabled_row_without_loading_it_and_an_enabled_row_as_active() {
    let kernel = Kernel::start();
    let root = kernel.root();
    let denied = create_scope(&root, ScopeKey::new(), Default::default()).unwrap();
    let allowed = create_scope(&root, ScopeKey::new(), Default::default()).unwrap();
    let denied_definition = definition(
        "denied",
        json!([{"id":"row","name":"fixture:active","disabled":true}]),
    );
    let allowed_definition = definition("allowed", json!([{"id":"row","name":"fixture:active"}]));
    let denied_mount =
        mount_preset_with_runtime(&denied.ctx, "denied", &denied_definition, &runtime())
            .await
            .unwrap();
    let allowed_mount =
        mount_preset_with_runtime(&allowed.ctx, "allowed", &allowed_definition, &runtime())
            .await
            .unwrap();
    assert_eq!(
        mounted_composition_rows(&denied_mount)[0].enabled,
        CompositionRowEnablement::Enabled(false)
    );
    assert_eq!(
        mounted_composition_rows(&allowed_mount)[0].enabled,
        CompositionRowEnablement::Enabled(true)
    );
    kernel.shutdown().await;
}
