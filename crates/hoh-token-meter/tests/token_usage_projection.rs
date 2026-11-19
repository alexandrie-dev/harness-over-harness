use hoh_session::{SessionEvent, SessionHeader, SessionLogOffset};
use hoh_token_meter::{context_pressure_projection_definition, token_usage_projection_definition};
use serde_json::{Value, json};

fn event(seq: u64, kind: &str, data: Value, surface_op: Option<Value>) -> SessionEvent {
    let mut value = json!({"seq":seq,"time":seq,"type":kind,"data":data});
    if let Some(surface_op) = surface_op {
        value["surfaceOp"] = surface_op;
    }
    SessionEvent::from_value(value)
}

fn usage(input: u64, output: u64, read: u64, write: u64) -> Value {
    json!({
        "inputTokens":input,"outputTokens":output,
        "cacheReadTokens":read,"cacheWriteTokens":write
    })
}

fn attempt(seq: u64, usage: Value) -> SessionEvent {
    event(
        seq,
        "assistant/attempt",
        json!({
            "turn":1,"step":1,
            "stream":[{"type":"chunk","time":seq,"chunk":{"type":"usage","usage":usage}}]
        }),
        None,
    )
}

fn assistant(seq: u64, usage: Value) -> SessionEvent {
    event(
        seq,
        "assistant/message",
        json!({
            "turn":1,"step":1,"stream":[],"usage":usage,
            "message":{"role":"assistant","content":[],"source":{"kind":"model","provider":"p","model":"m"},"id":"a"}
        }),
        Some(json!("append")),
    )
}

#[test]
fn serves_zero_buckets_and_replaces_an_identical_same_attempt_sample() {
    let definition = token_usage_projection_definition();
    let header = SessionHeader::new("usage".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    let wire = definition.wire.as_ref().unwrap();
    assert_eq!(
        (wire.view)(&state).unwrap().unwrap().as_ref(),
        &json!({
            "uncachedInputTokens":0,"outputTokens":0,
            "cacheReadTokens":0,"cacheWriteTokens":0
        })
    );
    state = (definition.apply)(&state, &attempt(0, usage(10, 2, 3, 1))).unwrap();
    let identity = state.clone();
    state = (definition.apply)(&state, &attempt(1, usage(10, 2, 3, 1))).unwrap();
    assert!(std::sync::Arc::ptr_eq(&identity, &state));
    assert_eq!(
        (wire.view)(&state).unwrap().unwrap().as_ref(),
        &json!({
            "uncachedInputTokens":10,"outputTokens":2,
            "cacheReadTokens":3,"cacheWriteTokens":1
        })
    );
}

#[test]
fn retry_started_closes_replacement_scope_and_accumulates_the_next_attempt() {
    let definition = token_usage_projection_definition();
    let header = SessionHeader::new("retry".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    for event in [
        attempt(0, usage(10, 2, 3, 0)),
        event(
            1,
            "llm/retry-started",
            json!({"turn":1,"step":1,"retry":1}),
            None,
        ),
        attempt(2, usage(14, 5, 8, 1)),
    ] {
        state = (definition.apply)(&state, &event).unwrap();
    }
    let view = (definition.wire.unwrap().view)(&state).unwrap().unwrap();
    assert_eq!(
        view.as_ref(),
        &json!({
            "uncachedInputTokens":24,"outputTokens":7,
            "cacheReadTokens":11,"cacheWriteTokens":1
        })
    );
}

#[test]
fn final_message_replaces_the_earlier_same_step_stream_sample() {
    let definition = token_usage_projection_definition();
    let header = SessionHeader::new("replace".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    state = (definition.apply)(&state, &attempt(0, usage(100, 20, 50, 0))).unwrap();
    state = (definition.apply)(&state, &assistant(1, usage(30, 5, 10, 0))).unwrap();
    let view = (definition.wire.unwrap().view)(&state).unwrap().unwrap();
    assert_eq!(
        view.as_ref(),
        &json!({
            "uncachedInputTokens":30,"outputTokens":5,
            "cacheReadTokens":10,"cacheWriteTokens":0
        })
    );
}

#[test]
fn malformed_checkpoint_replacement_returns_an_error_instead_of_panicking_or_wrapping() {
    let definition = token_usage_projection_definition();
    let checkpoint = json!({
        "totals": {
            "uncachedInputTokens": 0,
            "outputTokens": 5,
            "cacheReadTokens": 0,
            "cacheWriteTokens": 0
        },
        "last": {
            "turn": 1,
            "step": 1,
            "buckets": {
                "uncachedInputTokens": 0,
                "outputTokens": 9,
                "cacheReadTokens": 0,
                "cacheWriteTokens": 0
            }
        }
    });
    let state = (definition.state_schema)(Some(&checkpoint)).unwrap();
    let error = (definition.apply)(&state, &assistant(0, usage(0, 1, 0, 0)))
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "token usage: replacement produced a negative or overflowing bucket"
    );
}

#[test]
fn visible_surface_replacements_do_not_erase_historical_billing() {
    let definition = token_usage_projection_definition();
    let header = SessionHeader::new("durable-billing".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    state = (definition.apply)(&state, &attempt(0, usage(10, 2, 3, 1))).unwrap();
    let before = (definition.wire.as_ref().unwrap().view)(&state)
        .unwrap()
        .unwrap();
    state = (definition.apply)(
        &state,
        &event(
            1,
            "user/message",
            json!({
                "role":"user","content":[{"type":"text","text":"summary"}],
                "source":{"kind":"test"},"id":"s"
            }),
            Some(json!({"op":"replace","startSeq":7,"endSeq":8})),
        ),
    )
    .unwrap();
    let after = (definition.wire.as_ref().unwrap().view)(&state)
        .unwrap()
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn newest_context_capacity_replaces_or_removes_the_older_value() {
    let definition = context_pressure_projection_definition();
    let header = SessionHeader::new("capacity".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    for (seq, data) in [
        (
            0,
            json!({"provider":"p","model":"small","contextWindow":64000}),
        ),
        (
            1,
            json!({"provider":"p","model":"large","contextWindow":256000}),
        ),
        (2, json!({"provider":"p","model":"unknown"})),
    ] {
        state = (definition.apply)(&state, &event(seq, "request/context", data, None)).unwrap();
    }
    let view = (definition.wire.unwrap().view)(&state).unwrap().unwrap();
    assert_eq!(view.as_ref(), &json!({}));
}

#[test]
fn capacity_without_usage_does_not_synthesize_zero_pressure_and_noops_retain_identity() {
    let definition = context_pressure_projection_definition();
    let header = SessionHeader::new("no-sample".into(), 0);
    let state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    let capacity = event(
        0,
        "request/context",
        json!({"provider":"p","model":"m","contextWindow":64000}),
        None,
    );
    let state = (definition.apply)(&state, &capacity).unwrap();
    let view = (definition.wire.as_ref().unwrap().view)(&state)
        .unwrap()
        .unwrap();
    assert_eq!(view.as_ref(), &json!({"contextWindow":64000}));
    let repeated = (definition.apply)(&state, &capacity).unwrap();
    assert!(std::sync::Arc::ptr_eq(&state, &repeated));
    let unrelated =
        (definition.apply)(&state, &event(1, "session/end-seed", json!({}), None)).unwrap();
    assert!(std::sync::Arc::ptr_eq(&state, &unrelated));
}

#[test]
fn pressure_keeps_signed_internal_surface_and_clamps_only_the_wire_projection() {
    let definition = context_pressure_projection_definition();
    let header = SessionHeader::new("signed".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    let user = event(
        0,
        "user/message",
        json!({
            "role":"user","content":[{"type":"text","text":"long prompt"}],
            "source":{"kind":"user"},"id":"u"
        }),
        Some(json!("append")),
    );
    state = (definition.apply)(&state, &user).unwrap();
    state = (definition.apply)(&state, &attempt(1, usage(3, 1, 0, 0))).unwrap();
    let claim = event(
        2,
        "compaction/summary",
        json!({
            "shadowedRange":{"start":0,"end":0},
            "shadowedTokenCount":100,"shadowedSeqs":[0]
        }),
        None,
    );
    state = (definition.apply)(&state, &claim).unwrap();
    let replacement = event(
        3,
        "user/message",
        json!({
            "role":"user","content":[{"type":"text","text":"."}],
            "source":{"kind":"test"},"id":"s"
        }),
        Some(json!({"op":"replace","startSeq":0,"endSeq":0})),
    );
    state = (definition.apply)(&state, &replacement).unwrap();
    assert!(state.surface_tokens < 0);
    let view = (definition.wire.unwrap().view)(&state).unwrap().unwrap();
    assert_eq!(view["projectedTokens"], json!(0));
}

#[test]
fn mismatched_shadow_endpoint_errors_and_an_unclaimed_replacement_is_zero_delta() {
    let definition = context_pressure_projection_definition();
    let header = SessionHeader::new("claims".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    state = (definition.apply)(
        &state,
        &event(
            0,
            "compaction/summary",
            json!({
                "shadowedRange":{"start":1,"end":2},
                "shadowedTokenCount":7,"shadowedSeqs":[1,2]
            }),
            None,
        ),
    )
    .unwrap();
    let replacement = event(
        1,
        "user/message",
        json!({
            "role":"user","content":[{"type":"text","text":"summary"}],
            "source":{"kind":"test"},"id":"s"
        }),
        Some(json!({"op":"replace","startSeq":1,"endSeq":1})),
    );
    assert!(
        (definition.apply)(&state, &replacement)
            .unwrap_err()
            .to_string()
            .contains("armed claim covers 1-2")
    );
    let fresh = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    let unchanged = (definition.apply)(&fresh, &replacement).unwrap();
    assert_eq!(unchanged.surface_tokens, 0);
}
