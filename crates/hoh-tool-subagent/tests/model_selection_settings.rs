use hoh_tool_subagent::model_selection_settings::Config;
use serde_json::json;

#[test]
fn settings_default_off_with_no_allowed_routes() {
    let config: Config = serde_json::from_value(json!({})).unwrap();
    assert!(!config.enabled.0);
    assert!(config.allowed_models.0.is_empty());
}
