use hoh_protocol::{ModelProbeResult, validate_result};
use serde_json::json;

#[test]
fn probe_returns_provider_model_names_including_an_empty_listing() {
    for models in [json!(["local", "fixture-reasoner"]), json!([])] {
        let result = json!({"ok":true,"latencyMs":20,"toolCalls":true,"models":models});
        validate_result("hoh/model/probe", &result).unwrap();
        let typed: ModelProbeResult = serde_json::from_value(result.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), result);
    }
    for invalid in [
        json!({"ok":true,"latencyMs":20,"toolCalls":true}),
        json!({"ok":true,"latencyMs":20,"toolCalls":true,"models":[1]}),
    ] {
        assert!(validate_result("hoh/model/probe", &invalid).is_err());
    }
}
