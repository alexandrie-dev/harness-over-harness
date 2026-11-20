use hoh_tool_subagent::model_selection_state::subagent_model_selection_projection_definition;

#[test]
fn policy_projection_registration_identity_is_stable() {
    let definition = subagent_model_selection_projection_definition();
    assert_eq!(definition.key.as_str(), "subagentModelSelectionPolicy");
    assert_eq!(definition.state_version, 1);
}
