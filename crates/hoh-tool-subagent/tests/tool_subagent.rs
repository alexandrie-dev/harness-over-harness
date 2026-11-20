use hoh_tool_subagent::{Config, INJECT, NAME, provider_wording};
use serde_json::json;

#[test]
fn namespace_and_model_facing_defaults_match_the_source() {
    assert_eq!(NAME, "tool-subagent");
    assert_eq!(
        INJECT,
        ["tools", "subagents", "systemPrompt", "sessionProjections"]
    );
    let config: Config = serde_json::from_value(json!({ "provider": "spawn" })).unwrap();
    assert_eq!(config.tool_name, "subagent");
    let (description, prompt) = provider_wording(false);
    assert!(description.contains("separate agent that works in its own context"));
    assert!(prompt.contains("does not share this conversation's context"));
}
