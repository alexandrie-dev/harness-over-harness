use hoh_agent_preset_registry::{
    CompositionRowEnablement, definition_composition, entry_list_problem,
};
use serde_json::{Value, json};

#[test]
fn reports_evaluated_and_unresolved_conditions_with_ancestor_enablement() {
    let rows = json!([
        {"name":"enabled","id":"enabled-id"},
        {"name":"off","disabled":true},
        {"name":"on","disabled":{"__jsExpr":"false"}},
        {"name":"conditional","disabled":{"__jsExpr":"unknownVariable"}},
        {"name":"group","group":true,"disabled":{"__jsExpr":"unknownVariable"},"config":[
            {"name":"child"},{"name":"disabled-child","disabled":true}
        ]},
        {"name":"off-group","group":true,"disabled":true,"config":[{"name":"buried"}]}
    ]);
    let read = definition_composition(&rows, &|expression| match expression {
        "false" => Ok(Value::Bool(false)),
        _ => Err("unknown variable".into()),
    })
    .unwrap();
    assert_eq!(read.len(), 7);
    assert_eq!(read[0].entry_id.as_deref(), Some("enabled-id"));
    assert_eq!(read[0].enabled, CompositionRowEnablement::Enabled(true));
    assert_eq!(read[1].enabled, CompositionRowEnablement::Enabled(false));
    assert_eq!(read[2].enabled, CompositionRowEnablement::Enabled(true));
    assert!(matches!(
        read[3].enabled,
        CompositionRowEnablement::Conditional(_)
    ));
    assert!(matches!(
        read[4].enabled,
        CompositionRowEnablement::Conditional(_)
    ));
    assert_eq!(read[5].enabled, CompositionRowEnablement::Enabled(false));
    assert_eq!(read[6].enabled, CompositionRowEnablement::Enabled(false));
    assert!(entry_list_problem(&json!([{"group":true}])).is_some());
}
