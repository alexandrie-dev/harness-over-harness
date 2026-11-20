use hoh_tool_subagent::Config;
use serde_json::json;

#[test]
fn discovery_defaults_off_independently_of_model_selection() {
    let default: Config = serde_json::from_value(json!({ "provider": "spawn" })).unwrap();
    assert!(!default.model_discovery);
    assert!(!default.model_selection);

    let discovery: Config = serde_json::from_value(json!({
        "provider": "spawn",
        "modelDiscovery": true
    }))
    .unwrap();
    assert!(discovery.model_discovery);
    assert!(!discovery.model_selection);
}
