use hoh_session::SessionEvent;
use hoh_token_meter::{TurnTokenUsage, TurnTokenUsageRoute, derive_turn_token_usage};
use serde_json::{Value, json};

fn event(seq: u64, event_type: &str, data: Value) -> SessionEvent {
    SessionEvent::from_value(json!({"seq":seq,"time":seq,"type":event_type,"data":data}))
}

fn usage(overrides: Value) -> Value {
    let mut value = json!({
        "inputTokens":100,
        "outputTokens":20,
        "totalTokens":170,
        "cacheReadTokens":50
    });
    for (key, replacement) in overrides.as_object().unwrap() {
        if replacement.is_null() {
            value.as_object_mut().unwrap().shift_remove(key);
        } else {
            value[key] = replacement.clone();
        }
    }
    value
}

fn assistant(
    seq: u64,
    usage: Option<Value>,
    provider: &str,
    model: &str,
    step: u64,
) -> SessionEvent {
    let mut data = json!({
        "turn":1,
        "step":step,
        "stream":[],
        "message":{
            "role":"assistant",
            "content":[{"type":"text","text":"done"}],
            "source":{"kind":"model","provider":provider,"model":model},
            "id":format!("message-{seq}")
        }
    });
    if let Some(usage) = usage {
        data["usage"] = usage;
    }
    event(seq, "assistant/message", data)
}

fn attempt(seq: u64, step: u64, chunks: Value) -> SessionEvent {
    event(
        seq,
        "assistant/attempt",
        json!({"turn":1,"step":step,"stream":chunks}),
    )
}

fn complete(middle: impl IntoIterator<Item = SessionEvent>) -> Vec<SessionEvent> {
    let mut events = vec![
        event(1, "turn/start", json!({"turn":1})),
        event(2, "step/start", json!({"turn":1,"step":1})),
    ];
    events.extend(middle);
    events.push(event(90, "step/end", json!({"turn":1,"step":1})));
    events.push(event(
        91,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
    ));
    events
}

#[test]
fn preserves_authoritative_totals_optional_buckets_and_route() {
    let result = derive_turn_token_usage(&complete([assistant(
        3,
        Some(usage(json!({"cacheWriteTokens":0,"reasoningTokens":8}))),
        "deepseek",
        "deepseek-chat",
        1,
    )]));
    assert_eq!(
        result,
        Some(TurnTokenUsage {
            uncached_input_tokens: 100,
            output_tokens: 20,
            total_tokens: 170,
            cache_read_tokens: Some(50),
            cache_write_tokens: Some(0),
            reasoning_tokens: Some(8),
            routes: Some(vec![TurnTokenUsageRoute {
                provider: "deepseek".into(),
                model: "deepseek-chat".into(),
            }]),
        })
    );
}

#[test]
fn derives_total_only_when_both_cache_buckets_are_present() {
    let derived = usage(json!({
        "totalTokens":null,"inputTokens":10,"outputTokens":4,
        "cacheReadTokens":2,"cacheWriteTokens":1
    }));
    assert_eq!(
        derive_turn_token_usage(&complete([assistant(3, Some(derived), "p", "m", 1)]))
            .unwrap()
            .total_tokens,
        17
    );
    let missing = usage(json!({"totalTokens":null,"cacheWriteTokens":null}));
    assert!(
        derive_turn_token_usage(&complete([assistant(3, Some(missing), "p", "m", 1)])).is_none()
    );
}

#[test]
fn final_message_usage_replaces_the_latest_stream_sample() {
    let mut message = assistant(
        4,
        Some(usage(
            json!({"inputTokens":30,"outputTokens":5,"totalTokens":45,"cacheReadTokens":10}),
        )),
        "p",
        "m",
        1,
    );
    let mut value = message.as_value().clone();
    value["data"]["stream"] = json!([{
        "type":"chunk","time":3,
        "chunk":{"type":"usage","usage":usage(json!({}))}
    }]);
    message = SessionEvent::from_value(value);
    let result = derive_turn_token_usage(&complete([message])).unwrap();
    assert_eq!(result.uncached_input_tokens, 30);
    assert_eq!(result.output_tokens, 5);
    assert_eq!(result.total_tokens, 45);
}

#[test]
fn stream_sample_survives_when_final_message_omits_usage() {
    let mut message = assistant(4, None, "p", "m", 1);
    let mut value = message.as_value().clone();
    value["data"]["stream"] = json!([{
        "type":"chunk","time":3,
        "chunk":{"type":"usage","usage":usage(json!({}))}
    }]);
    message = SessionEvent::from_value(value);
    assert_eq!(
        derive_turn_token_usage(&complete([message]))
            .unwrap()
            .total_tokens,
        170
    );
}

#[test]
fn counts_an_error_finished_attempt_once_across_retry() {
    let first = event(
        3,
        "assistant/attempt",
        json!({
            "turn":1,"step":1,
            "stream":[
                {"type":"chunk","time":3,"chunk":{"type":"usage","usage":usage(json!({}))}},
                {"type":"chunk","time":4,"chunk":{"type":"finish","reason":{"kind":"error"}}}
            ]
        }),
    );
    let events = complete([
        first,
        event(5, "llm/retry", json!({"turn":1,"step":1})),
        event(6, "llm/retry-started", json!({"turn":1,"step":1,"retry":1})),
        assistant(
            7,
            Some(usage(
                json!({"inputTokens":40,"outputTokens":10,"totalTokens":70,"cacheReadTokens":20}),
            )),
            "p",
            "m",
            1,
        ),
    ]);
    let result = derive_turn_token_usage(&events).unwrap();
    assert_eq!(result.uncached_input_tokens, 140);
    assert_eq!(result.output_tokens, 30);
    assert_eq!(result.total_tokens, 240);
    assert_eq!(result.cache_read_tokens, Some(70));
}

#[test]
fn scheduled_retry_that_never_started_does_not_invent_an_attempt() {
    let attempt = event(
        3,
        "assistant/attempt",
        json!({
            "turn":1,"step":1,
            "stream":[{"type":"chunk","time":3,"chunk":{"type":"usage","usage":usage(json!({}))}}]
        }),
    );
    let result = derive_turn_token_usage(&complete([
        attempt,
        event(4, "llm/retry", json!({"turn":1,"step":1})),
    ]))
    .unwrap();
    assert_eq!(result.total_tokens, 170);
}

#[test]
fn rejects_invalid_or_contradictory_usage_shapes() {
    let invalid = [
        usage(json!({"inputTokens":-1})),
        usage(json!({"outputTokens":1.5})),
        usage(json!({"totalTokens":9_007_199_254_740_992_u64})),
        usage(json!({"cacheReadTokens":-1})),
        usage(json!({"cacheWriteTokens":1.5})),
        usage(json!({"outputTokens":20,"totalTokens":10,"cacheReadTokens":null})),
        usage(json!({"totalTokens":160})),
        usage(json!({"totalTokens":171,"cacheWriteTokens":0})),
        usage(json!({"reasoningTokens":21})),
        usage(json!({
            "inputTokens":9_007_199_254_740_991_u64,"outputTokens":0,
            "totalTokens":9_007_199_254_740_991_u64,"cacheReadTokens":1
        })),
        usage(json!({
            "inputTokens":9_007_199_254_740_991_u64,"outputTokens":1,
            "totalTokens":null,"cacheReadTokens":0,"cacheWriteTokens":0
        })),
    ];
    for usage in invalid {
        assert!(
            derive_turn_token_usage(&complete([assistant(3, Some(usage), "p", "m", 1)])).is_none()
        );
    }
}

#[test]
fn optional_aggregates_and_routes_require_every_attempt() {
    let events = vec![
        event(1, "turn/start", json!({"turn":1})),
        event(2, "step/start", json!({"turn":1,"step":1})),
        assistant(
            3,
            Some(usage(
                json!({"cacheWriteTokens":5,"reasoningTokens":2,"totalTokens":175}),
            )),
            "p",
            "m",
            1,
        ),
        event(4, "step/end", json!({"turn":1,"step":1})),
        event(5, "step/start", json!({"turn":1,"step":2})),
        assistant(
            6,
            Some(usage(
                json!({"cacheReadTokens":null,"cacheWriteTokens":null,"reasoningTokens":null}),
            )),
            "",
            "",
            2,
        ),
        event(7, "step/end", json!({"turn":1,"step":2})),
        event(
            8,
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    let result = derive_turn_token_usage(&events).unwrap();
    assert_eq!(result.uncached_input_tokens, 200);
    assert_eq!(result.output_tokens, 40);
    assert_eq!(result.total_tokens, 345);
    assert_eq!(result.cache_read_tokens, None);
    assert_eq!(result.cache_write_tokens, None);
    assert_eq!(result.reasoning_tokens, None);
    assert_eq!(result.routes, None);
}

#[test]
fn sums_steps_and_deduplicates_distinct_routes_in_first_seen_order() {
    let events = vec![
        event(1, "turn/start", json!({"turn":1})),
        event(2, "step/start", json!({"turn":1,"step":1})),
        assistant(3, Some(usage(json!({}))), "deepseek", "chat", 1),
        event(4, "step/end", json!({"turn":1,"step":1})),
        event(5, "step/start", json!({"turn":1,"step":2})),
        assistant(6, Some(usage(json!({}))), "openai", "gpt", 2),
        event(7, "step/end", json!({"turn":1,"step":2})),
        event(
            8,
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    let result = derive_turn_token_usage(&events).unwrap();
    assert_eq!(result.total_tokens, 340);
    assert_eq!(
        result.routes.unwrap(),
        vec![
            TurnTokenUsageRoute {
                provider: "deepseek".into(),
                model: "chat".into()
            },
            TurnTokenUsageRoute {
                provider: "openai".into(),
                model: "gpt".into()
            },
        ]
    );
}

#[test]
fn fails_closed_for_missing_lifecycle_and_unsafe_aggregate() {
    assert!(
        derive_turn_token_usage(&[assistant(1, Some(usage(json!({}))), "p", "m", 1)]).is_none()
    );
    let half = 9_007_199_254_740_991_u64 / 2 + 1;
    let value =
        usage(json!({"inputTokens":0,"outputTokens":0,"cacheReadTokens":null,"totalTokens":half}));
    let events = vec![
        event(1, "turn/start", json!({"turn":1})),
        event(2, "step/start", json!({"turn":1,"step":1})),
        assistant(3, Some(value.clone()), "p", "m", 1),
        event(4, "step/end", json!({"turn":1,"step":1})),
        event(5, "step/start", json!({"turn":1,"step":2})),
        assistant(6, Some(value), "p", "m", 2),
        event(7, "step/end", json!({"turn":1,"step":2})),
        event(
            8,
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    assert!(derive_turn_token_usage(&events).is_none());

    for value in [
        usage(json!({
            "inputTokens":half,"outputTokens":0,"cacheReadTokens":null,"totalTokens":half
        })),
        usage(json!({
            "inputTokens":0,"outputTokens":half,"cacheReadTokens":null,"totalTokens":half
        })),
    ] {
        let events = vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            assistant(3, Some(value.clone()), "p", "m", 1),
            event(4, "step/end", json!({"turn":1,"step":1})),
            event(5, "step/start", json!({"turn":1,"step":1})),
            assistant(6, Some(value), "p", "m", 1),
            event(7, "step/end", json!({"turn":1,"step":1})),
            event(
                8,
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
        ];
        assert!(derive_turn_token_usage(&events).is_none());
    }
}

#[test]
fn sampled_open_attempt_can_close_at_step_end() {
    let events = complete([event(
        3,
        "assistant/attempt",
        json!({
            "turn":1,"step":1,
            "stream":[{"type":"chunk","time":3,"chunk":{"type":"usage","usage":usage(json!({}))}}]
        }),
    )]);
    assert_eq!(derive_turn_token_usage(&events).unwrap().total_tokens, 170);
}

#[test]
fn accepts_an_aborted_finish_after_observing_usage() {
    let sampled = attempt(
        3,
        1,
        json!([
            {"type":"chunk","time":3,"chunk":{"type":"usage","usage":usage(json!({}))}},
            {"type":"chunk","time":4,"chunk":{"type":"finish","reason":{"kind":"aborted","failure":{"message":"aborted","code":"ABORTED"}}}}
        ]),
    );
    assert_eq!(
        derive_turn_token_usage(&complete([sampled]))
            .unwrap()
            .total_tokens,
        170
    );
}

#[test]
fn fails_closed_for_every_invalid_attempt_lifecycle() {
    let sampled_attempt = || {
        attempt(
            3,
            1,
            json!([
                {"type":"chunk","time":3,"chunk":{"type":"usage","usage":usage(json!({}))}},
                {"type":"chunk","time":4,"chunk":{"type":"finish","reason":{"kind":"error","failure":{"code":"HTTP","message":"failed"}}}}
            ]),
        )
    };
    let cases = vec![
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(
                2,
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "turn/start", json!({"turn":1})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(
                2,
                "turn/end",
                json!({"turn":2,"reason":{"kind":"completed"}}),
            ),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            event(
                3,
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(
                2,
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
            event(
                3,
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(
                2,
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
            event(3, "step/start", json!({"turn":1,"step":1})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":2,"step":1})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            event(3, "step/start", json!({"turn":1,"step":2})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "llm/retry-started", json!({"turn":1,"step":1,"retry":1})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            assistant(3, Some(usage(json!({}))), "p", "m", 1),
            event(4, "llm/retry-started", json!({"turn":1,"step":1,"retry":1})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            sampled_attempt(),
            event(4, "llm/retry", json!({"turn":1,"step":1})),
            event(5, "llm/retry-started", json!({"turn":1,"step":2,"retry":1})),
        ],
        vec![event(1, "turn/start", json!({"turn":1})), sampled_attempt()],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            attempt(
                3,
                2,
                json!([{"type":"chunk","time":3,"chunk":{"type":"usage","usage":usage(json!({}))}}]),
            ),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            attempt(
                3,
                1,
                json!([{"type":"chunk","time":3,"chunk":{"type":"finish","reason":{"kind":"error","failure":{"code":"HTTP","message":"failed"}}}}]),
            ),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "llm/retry", json!({"turn":1,"step":1})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            sampled_attempt(),
            event(4, "llm/retry", json!({"turn":1,"step":2})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            assistant(3, Some(usage(json!({}))), "p", "m", 1),
            event(4, "llm/retry", json!({"turn":1,"step":1})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            event(3, "llm/retry", json!({"turn":1,"step":1})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/end", json!({"turn":1,"step":1})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            event(3, "step/end", json!({"turn":1,"step":2})),
        ],
        vec![
            event(1, "turn/start", json!({"turn":1})),
            event(2, "step/start", json!({"turn":1,"step":1})),
            event(3, "step/end", json!({"turn":1,"step":1})),
        ],
    ];
    for events in cases {
        assert!(derive_turn_token_usage(&events).is_none());
    }
}

#[test]
fn requires_the_complete_turn_window() {
    let events = complete([assistant(3, Some(usage(json!({}))), "p", "m", 1)]);
    assert!(derive_turn_token_usage(&events[1..]).is_none());
    assert!(derive_turn_token_usage(&events[..events.len() - 1]).is_none());
}
