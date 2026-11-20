use hoh_tool_subagent::{Config, MaxDepthConfig};
use serde_json::json;

#[test]
fn depth_configuration_preserves_numeric_and_provider_managed_forms() {
    let numeric: Config =
        serde_json::from_value(json!({ "provider": "spawn", "maxDepth": 3 })).unwrap();
    assert_eq!(numeric.max_depth, Some(MaxDepthConfig::Value(3)));

    let managed: Config = serde_json::from_value(json!({
        "provider": "spawn",
        "maxDepth": "provider-managed"
    }))
    .unwrap();
    assert_eq!(
        managed.max_depth,
        Some(MaxDepthConfig::ProviderManaged("provider-managed".into()))
    );
}
