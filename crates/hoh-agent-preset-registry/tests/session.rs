use hoh_agent_preset_registry::agent_preset_projection_definition;
use hoh_session::{
    SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionId, SessionLogOffset,
};
use serde_json::{Map, json};
use std::sync::Arc;

fn header(agent_preset: Option<&str>) -> SessionHeader {
    SessionHeader {
        version: SESSION_FORMAT_VERSION,
        id: SessionId::from("s"),
        created_at: 1,
        cwd: None,
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: Some(0),
        agent_preset: agent_preset.map(str::to_owned),
        extra: Map::new(),
    }
}

#[test]
fn starts_from_the_creation_header_including_no_configured_preset() {
    let definition = agent_preset_projection_definition();
    assert_eq!(
        *(definition.init)(&header(Some("standard")), SessionLogOffset::default()).unwrap(),
        Some("standard".into())
    );
    assert_eq!(
        *(definition.init)(&header(None), SessionLogOffset::default()).unwrap(),
        None
    );
}

#[test]
fn starts_from_the_header_and_keeps_the_latest_selected_preset() {
    let definition = agent_preset_projection_definition();
    let mut state =
        (definition.init)(&header(Some("standard")), SessionLogOffset::default()).unwrap();
    for (seq, preset) in [(0, "minimal"), (2, "cordis")] {
        let event = SessionEvent::from_value(json!({
            "type":"agent-preset/selected","seq":seq,"time":seq,
            "data":{"agentPreset":preset}
        }));
        state = (definition.apply)(&state, &event).unwrap();
        if seq == 0 {
            let unrelated = SessionEvent::from_value(json!({
                "type":"turn/end","seq":1,"time":1,
                "data":{"turn":1,"reason":{"kind":"completed"}}
            }));
            state = (definition.apply)(&state, &unrelated).unwrap();
        }
    }
    assert_eq!(*state, Some("cordis".into()));
    let wire = definition.wire.unwrap();
    assert_eq!(
        (wire.view)(&state).unwrap(),
        Some(Arc::new(json!("cordis")))
    );
    assert!((definition.state_schema)(Some(&json!("cordis"))).is_ok());
}
