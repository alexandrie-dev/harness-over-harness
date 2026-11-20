use hoh_tool_subagent::preset_selection::{PresetSelectionConfig, SubagentPresetSelection};
use serde_json::json;

#[test]
fn configured_default_and_model_choices_remain_distinct() {
    let selection = SubagentPresetSelection::new(PresetSelectionConfig {
        agent_preset: Some("worker".into()),
        allowed_agent_presets: Some(vec!["worker".into(), "reviewer".into()]),
    })
    .unwrap();
    assert_eq!(selection.resolve(None).unwrap().as_deref(), Some("worker"));
    assert_eq!(
        selection
            .resolve(Some(&json!("reviewer")))
            .unwrap()
            .as_deref(),
        Some("reviewer")
    );
}
