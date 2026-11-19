use hoh_session::{SessionEvent, SessionHeader, SessionLogOffset};
use hoh_token_meter::context_breakdown_projection_definition;
use serde_json::{Value, json};

fn event(seq: u64, kind: &str, data: Value, surface_op: Option<Value>) -> SessionEvent {
    let mut value = json!({"seq":seq,"time":seq,"type":kind,"data":data});
    if let Some(surface_op) = surface_op {
        value["surfaceOp"] = surface_op;
    }
    SessionEvent::from_value(value)
}

fn message(role: &str, text: &str, id: &str) -> Value {
    let message = json!({
        "role":role,
        "content":if text.is_empty() { vec![] } else { vec![json!({"type":"text","text":text})] },
        "source":{"kind":if role == "system" { "system-prompt" } else { "user" }},
        "id":id
    });
    if role == "system" {
        json!({"message":message})
    } else {
        message
    }
}

fn view(
    definition: &hoh_session_projection::ProjectionDefinition<
        hoh_token_meter::breakdown_projection::ContextBreakdownState,
    >,
    state: &std::sync::Arc<hoh_token_meter::breakdown_projection::ContextBreakdownState>,
) -> std::sync::Arc<Value> {
    (definition.wire.as_ref().unwrap().view)(state)
        .unwrap()
        .unwrap()
}

#[test]
fn serves_zeros_and_newest_envelope_is_last_wins_without_phantom_change() {
    let definition = context_breakdown_projection_definition();
    let header = SessionHeader::new("breakdown".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    assert_eq!(
        view(&definition, &state).as_ref(),
        &json!({"systemTokens":0,"toolsTokens":0,"messageTokens":0})
    );
    let request = event(
        0,
        "request/header",
        json!({"reason":"test","header":{"tools":[{"name":"read"}]}}),
        None,
    );
    state = (definition.apply)(&state, &request).unwrap();
    let prior = state.clone();
    state = (definition.apply)(&state, &request).unwrap();
    assert!(std::sync::Arc::ptr_eq(&prior, &state));
    assert!(view(&definition, &state)["toolsTokens"].as_u64().unwrap() > 0);
}

#[test]
fn newest_nonempty_system_is_system_and_older_or_cleared_prompts_are_messages() {
    let definition = context_breakdown_projection_definition();
    let header = SessionHeader::new("systems".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    for candidate in [
        event(
            0,
            "system/message",
            message("system", "old", "s0"),
            Some(json!("append")),
        ),
        event(
            1,
            "system/message",
            message("system", "new prompt", "s1"),
            Some(json!("append")),
        ),
    ] {
        state = (definition.apply)(&state, &candidate).unwrap();
    }
    let first = view(&definition, &state);
    assert!(first["systemTokens"].as_u64().unwrap() > 0);
    assert!(first["messageTokens"].as_u64().unwrap() > 0);
    state = (definition.apply)(
        &state,
        &event(
            2,
            "system/message",
            message("system", "", "s2"),
            Some(json!({"op":"replace","startSeq":1,"endSeq":1})),
        ),
    )
    .unwrap();
    let cleared = view(&definition, &state);
    assert!(cleared["systemTokens"].as_u64().unwrap() > 0);
    assert!(cleared["systemTokens"].as_u64().unwrap() < first["systemTokens"].as_u64().unwrap());
}

#[test]
fn metered_compaction_removes_shadowed_systems_and_shrinks_the_message_figure() {
    let definition = context_breakdown_projection_definition();
    let header = SessionHeader::new("compaction".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    for candidate in [
        event(
            0,
            "system/message",
            message("system", "old system", "s"),
            Some(json!("append")),
        ),
        event(
            1,
            "user/message",
            message("user", "a long question", "u"),
            Some(json!("append")),
        ),
    ] {
        state = (definition.apply)(&state, &candidate).unwrap();
    }
    let before = view(&definition, &state);
    let shadowed = serde_json::to_value(state.as_ref()).unwrap()["surfaceTokens"]
        .as_u64()
        .unwrap();
    state = (definition.apply)(
        &state,
        &event(
            2,
            "compaction/summary",
            json!({
                "shadowedRange":{"start":0,"end":1},
                "shadowedTokenCount":shadowed,"shadowedSeqs":[0,1]
            }),
            None,
        ),
    )
    .unwrap();
    state = (definition.apply)(
        &state,
        &event(
            3,
            "user/message",
            message("user", "summary", "sum"),
            Some(json!({"op":"replace","startSeq":0,"endSeq":1})),
        ),
    )
    .unwrap();
    let after = view(&definition, &state);
    assert_eq!(after["systemTokens"], json!(0));
    assert!(after["messageTokens"].as_u64().unwrap() < before["messageTokens"].as_u64().unwrap());
}

#[test]
fn same_price_system_rewrite_retains_wire_identity_but_updates_checkpoint_positions() {
    let definition = context_breakdown_projection_definition();
    let header = SessionHeader::new("identity".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    state = (definition.apply)(
        &state,
        &event(
            0,
            "system/message",
            message("system", "same", "s0"),
            Some(json!("append")),
        ),
    )
    .unwrap();
    let before = view(&definition, &state);
    state = (definition.apply)(
        &state,
        &event(
            1,
            "system/message",
            message("system", "same", "s1"),
            Some(json!({"op":"replace","startSeq":0,"endSeq":0})),
        ),
    )
    .unwrap();
    let after = view(&definition, &state);
    assert!(std::sync::Arc::ptr_eq(&before, &after));
    let encoded = serde_json::to_value(state.as_ref()).unwrap();
    assert_eq!(encoded["systems"][0]["seq"], json!(1));
}

#[test]
fn armed_shadow_price_expires_on_the_next_unrelated_event() {
    let definition = context_breakdown_projection_definition();
    let header = SessionHeader::new("expiry".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    state = (definition.apply)(
        &state,
        &event(
            0,
            "compaction/prune",
            json!({
                "shadowedRange":{"start":1,"end":1},
                "shadowedTokenCount":10,"shadowedSeqs":[1]
            }),
            None,
        ),
    )
    .unwrap();
    state = (definition.apply)(&state, &event(1, "session/end-seed", json!({}), None)).unwrap();
    state = (definition.apply)(
        &state,
        &event(
            2,
            "user/message",
            message("user", "replacement", "u"),
            Some(json!({"op":"replace","startSeq":1,"endSeq":1})),
        ),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(state.as_ref()).unwrap()["surfaceTokens"],
        json!(0)
    );
}

#[test]
fn state_schema_rejects_old_scalar_cache_shapes() {
    let definition = context_breakdown_projection_definition();
    for old in [
        json!({"systemTokens":1,"toolsTokens":2,"messageTokens":3}),
        json!({"surfaceTokens":1,"systemTokens":1,"toolsTokens":2,"messageTokens":0}),
    ] {
        assert!((definition.state_schema)(Some(&old)).is_err());
    }
}
