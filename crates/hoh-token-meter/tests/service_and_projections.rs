use hoh_kernel::{Context, Fiber, Kernel, PluginDef, Realm};
use hoh_llm::{
    GenerateOptions, ImageBlock, Llm, LlmAdapter, LlmImageRequestPrice, LlmImageRequestPricing,
    LlmRuntime, LlmStream,
};
use hoh_session::{
    CreateSessionOptions, MessageProjections, Session, SessionEvent, SessionEventEmitted,
    SessionHeader, SessionLogOffset, SessionStore, Sessions, SurfaceIntent,
};
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use hoh_token_meter::{
    MeasurementState, TokenMeasurementBaseline, TokenMeter, TokenMeterPlugin, TokenMeterRuntime,
    measurement_projection_definition,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct CallerPricing;

impl LlmImageRequestPricing for CallerPricing {
    fn price_images(&self, images: &[ImageBlock]) -> Vec<LlmImageRequestPrice> {
        images
            .iter()
            .map(|image| LlmImageRequestPrice {
                visual_tokens: if image.offloaded == Some(true) {
                    0
                } else {
                    333
                },
                text: if image.offloaded == Some(true) {
                    "[offloaded]".into()
                } else {
                    String::new()
                },
            })
            .collect()
    }
}

struct CallerAdapter;

impl LlmAdapter for CallerAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, hoh_error::BoxError> {
        Ok(LlmStream::chunks([]))
    }

    fn image_request_pricing(
        &self,
        provider: &str,
        model: &str,
    ) -> Option<Arc<dyn LlmImageRequestPricing>> {
        (provider == "scoped" && model == "vision")
            .then(|| Arc::new(CallerPricing) as Arc<dyn LlmImageRequestPricing>)
    }
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    session: Session,
    meter_fiber: Fiber,
}

impl Harness {
    async fn new() -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let sessions = Arc::new(SessionStore::new());
        ctx.provide::<Sessions>(sessions.clone()).await.unwrap();
        ctx.plugin::<SessionProjectionRegistry>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let meter_fiber = ctx.plugin::<TokenMeterPlugin>(json!({})).unwrap();
        meter_fiber.settled().await.unwrap();
        let session = sessions
            .create(
                &ctx,
                Some("token-meter-test".into()),
                CreateSessionOptions::default(),
            )
            .unwrap();
        Self {
            kernel,
            ctx,
            session,
            meter_fiber,
        }
    }

    fn meter(&self) -> hoh_kernel::Service<TokenMeter> {
        self.ctx.get::<TokenMeter>().unwrap()
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

fn user(text: &str, id: &str) -> Value {
    json!({
        "role":"user",
        "content":[{"type":"text","text":text}],
        "source":{"kind":"user"},
        "id":id
    })
}

fn assistant(text: &str, id: &str) -> Value {
    json!({
        "role":"assistant",
        "content": if text.is_empty() { vec![] } else { vec![json!({"type":"text","text":text})] },
        "source":{"kind":"model","provider":"test","model":"model"},
        "id":id
    })
}

fn system(text: &str, id: &str) -> Value {
    json!({
        "role":"system",
        "content": if text.is_empty() { vec![] } else { vec![json!({"type":"text","text":text})] },
        "source":{"kind":"system-prompt"},
        "id":id
    })
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Value {
    json!({
        "inputTokens":input,"outputTokens":output,
        "cacheReadTokens":cache_read,"cacheWriteTokens":cache_write
    })
}

fn snapshot_value(ctx: &Context, session: &Session, key: &str) -> Value {
    ctx.get::<SessionProjections>()
        .unwrap()
        .snapshot(session, None)
        .unwrap()
        .values[key]
        .as_ref()
        .clone()
}

#[tokio::test]
async fn config_schema_is_an_empty_object_and_rejects_unknown_keys() {
    let schema = PluginDef::of::<TokenMeterPlugin>();
    assert_eq!(schema.name(), "TokenMeter");
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.provide::<Sessions>(Arc::new(SessionStore::new()))
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    for key in ["models", "contextWindow", "contextWidow", "a\"b"] {
        let mut config = serde_json::Map::new();
        config.insert(key.into(), json!([]));
        let fiber = ctx
            .plugin::<TokenMeterPlugin>(Value::Object(config))
            .unwrap();
        let error = fiber.settled().await.unwrap_err().to_string();
        assert_eq!(
            error,
            format!("TokenMeterConfig: unknown key \"{key}\" (no settings are supported)")
        );
    }
    let fiber = ctx
        .plugin::<TokenMeterPlugin>(json!({"b":1,"1":2}))
        .unwrap();
    assert_eq!(
        fiber.settled().await.unwrap_err().to_string(),
        "TokenMeterConfig: unknown key \"1\" (no settings are supported)"
    );
    kernel.shutdown().await;
}

#[test]
fn measurement_projection_preserves_arbitrary_diagnostic_coordinates_and_usage_fields() {
    let definition = measurement_projection_definition();
    let header = SessionHeader::new("measurement".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    let events = [
        SessionEvent::from_value(json!({
            "seq":0,"time":0,"type":"step/start",
            "data":{"turn":-1.5,"step":0.5,"diagnostic":"not state"}
        })),
        SessionEvent::from_value(json!({
            "seq":1,"time":1,"type":"request/header",
            "data":{"reason":"test","header":{}}
        })),
        SessionEvent::from_value(json!({
            "seq":2,"time":2,"type":"assistant/message","surfaceOp":"append",
            "data":{
                "turn":-1.5,"step":0.5,"stream":[],
                "usage":{"inputTokens":1000,"outputTokens":-1,"providerDetail":{"total":7}},
                "message":{"role":"assistant","content":[],"source":{"kind":"model","provider":"p","model":"m"},"id":"a"}
            }
        })),
    ];
    for event in &events {
        state = (definition.apply)(&state, event).unwrap();
    }
    assert_eq!(state.step_start.as_ref().unwrap().turn, -1.5);
    assert_eq!(state.step_start.as_ref().unwrap().step, 0.5);
    assert_eq!(
        state.anchor.as_ref().unwrap().usage.as_ref().unwrap(),
        &json!({"inputTokens":1000,"outputTokens":-1,"providerDetail":{"total":7}})
    );
    let encoded = serde_json::to_value(state.as_ref()).unwrap();
    assert_eq!(
        (definition.state_schema)(Some(&encoded)).unwrap().as_ref(),
        state.as_ref()
    );
}

#[test]
fn measurement_projection_does_not_anchor_usage_without_a_request_header() {
    let definition = measurement_projection_definition();
    let header = SessionHeader::new("headerless-usage".into(), 0);
    let mut state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    for event in [
        SessionEvent::from_value(json!({
            "seq":0,"time":0,"type":"step/start","data":{"turn":1,"step":1}
        })),
        SessionEvent::from_value(json!({
            "seq":1,"time":1,"type":"assistant/message","surfaceOp":"append",
            "data":{
                "turn":1,"step":1,"stream":[],"usage":usage(100,20,0,0),
                "message":assistant("durable output", "a")
            }
        })),
    ] {
        state = (definition.apply)(&state, &event).unwrap();
    }
    let anchor = state.anchor.as_ref().unwrap();
    assert!(anchor.usage.is_none());
    assert!(anchor.assistant_tokens > 0.0);
}

#[test]
fn measurement_projection_records_the_first_invalid_position_without_blocking_other_units() {
    let definition = measurement_projection_definition();
    let header = SessionHeader::new("invalid".into(), 0);
    let state = (definition.init)(&header, SessionLogOffset::default()).unwrap();
    let event = SessionEvent::from_value(json!({
        "seq":0,"time":0,"type":"assistant/message","surfaceOp":"append",
        "data":{"turn":1,"step":1,"stream":[],"message":assistant("", "a")}
    }));
    let state = (definition.apply)(&state, &event).unwrap();
    assert_eq!(state.invalid_seq.unwrap().get(), 0);
}

#[tokio::test]
async fn empty_and_surface_only_measurements_are_detached_and_current() {
    let h = Harness::new().await;
    let empty = h.meter().measure(&h.ctx, &h.session, None).unwrap();
    assert_eq!(empty.log_revision.get(), 0);
    assert_eq!(empty.total_tokens, 0.0);
    assert_eq!(empty.nodes, vec![]);
    assert_eq!(
        empty.baseline,
        TokenMeasurementBaseline::None { tokens: 0.0 }
    );
    h.session
        .append(
            "user/message",
            user("question", "u1"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let after = h.meter().measure(&h.ctx, &h.session, None).unwrap();
    assert_eq!(after.log_revision, h.session.seq());
    assert_eq!(after.nodes.len(), 1);
    assert_eq!(after.surface_tokens, after.nodes[0].tokens);
    assert_eq!(after.total_tokens, after.surface_tokens as f64);
    assert_eq!(empty.nodes, vec![]);
    h.close().await;
}

#[tokio::test]
async fn header_override_changes_pricing_envelope_but_not_the_surface() {
    let h = Harness::new().await;
    h.session
        .append(
            "user/message",
            user("question", "u1"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let base = h.meter().measure(&h.ctx, &h.session, None).unwrap();
    let override_header = json!({
        "config":{"provider":"test","model":"other"},
        "tools":[{"name":"read","description":"read"}]
    });
    let overridden = h
        .meter()
        .measure(&h.ctx, &h.session, Some(&override_header))
        .unwrap();
    assert_eq!(overridden.nodes, base.nodes);
    assert!(overridden.total_tokens > base.total_tokens);
    h.close().await;
}

#[tokio::test]
async fn measure_resolves_scoped_llm_from_the_callers_context() {
    let h = Harness::new().await;
    let caller = h
        .ctx
        .isolate("llm", Realm::new())
        .child("token-meter-caller")
        .unwrap();
    caller.fiber().settled().await.unwrap();
    let llm = Arc::new(LlmRuntime::new());
    llm.register_adapter(&caller, vec!["scoped".into()], Arc::new(CallerAdapter))
        .unwrap();
    caller.provide::<Llm>(llm).await.unwrap();
    h.session
        .append(
            "request/header",
            json!({
                "reason":"test",
                "header":{"config":{"provider":"scoped","model":"vision"}}
            }),
            None,
        )
        .unwrap();
    h.session
        .append(
            "user/message",
            json!({
                "role":"user",
                "content":[{"type":"image","attachment":{
                    "attachmentId":"image","mediaType":"image/png",
                    "bytes":8,"width":1,"height":1
                }}],
                "source":{"kind":"user"},"id":"u-image"
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let root = h.meter().measure(&h.ctx, &h.session, None).unwrap();
    let scoped = h.meter().measure(&caller, &h.session, None).unwrap();
    assert_eq!(root.nodes[0].seq, scoped.nodes[0].seq);
    assert_eq!(
        root.nodes[0].heuristic_tokens,
        scoped.nodes[0].heuristic_tokens
    );
    assert_ne!(root.nodes[0].tokens, scoped.nodes[0].tokens);
    assert_eq!(scoped.nodes[0].tokens, 341);
    h.close().await;
}

#[tokio::test]
async fn matching_successful_usage_anchors_signed_later_surface_growth() {
    let h = Harness::new().await;
    h.session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    h.session
        .append(
            "request/header",
            json!({"reason":"initial","header":{"config":{"provider":"test","model":"model"}}}),
            None,
        )
        .unwrap();
    h.session
        .append(
            "user/message",
            user("question", "u1"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    h.session
        .append(
            "assistant/message",
            json!({
                "turn":1,"step":1,
                "stream":[{"type":"chunk","time":0,"chunk":{"type":"usage","usage":usage(80,20,0,0)}}],
                "usage":usage(80,20,0,0),
                "message":assistant("answer", "a1")
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let measured = h.meter().measure(&h.ctx, &h.session, None).unwrap();
    assert!(matches!(
        measured.baseline,
        TokenMeasurementBaseline::Usage { tokens: 100.0, .. }
    ));
    assert!(measured.surface_delta_tokens > 0.0);
    assert_eq!(measured.total_tokens, 100.0 + measured.surface_delta_tokens);
    h.close().await;
}

#[tokio::test]
async fn undercut_provider_usage_uses_the_full_estimated_anchor() {
    let h = Harness::new().await;
    h.session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    h.session
        .append(
            "request/header",
            json!({"reason":"initial","header":{"config":{"provider":"test","model":"model"}}}),
            None,
        )
        .unwrap();
    h.session
        .append(
            "user/message",
            user(&"x".repeat(100), "u1"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    h.session.append(
        "assistant/message",
        json!({"turn":1,"step":1,"stream":[],"usage":usage(1,0,0,0),"message":assistant("", "a1")}),
        Some(SurfaceIntent::append()),
    ).unwrap();
    assert!(matches!(
        h.meter()
            .measure(&h.ctx, &h.session, None)
            .unwrap()
            .baseline,
        TokenMeasurementBaseline::Estimated { .. }
    ));
    h.close().await;
}

#[tokio::test]
async fn latest_successful_request_anchor_wins_and_envelope_changes_invalidate_usage() {
    let h = Harness::new().await;
    for (turn, model, input) in [(1, "first", 80), (2, "second", 180)] {
        h.session
            .append("step/start", json!({"turn":turn,"step":1}), None)
            .unwrap();
        h.session
            .append(
                "request/header",
                json!({
                    "reason":"test",
                    "header":{"config":{"provider":"test","model":model}}
                }),
                None,
            )
            .unwrap();
        h.session
            .append(
                "assistant/message",
                json!({
                    "turn":turn,"step":1,"stream":[],
                    "usage":usage(input,20,0,0),"message":assistant("", &format!("a{turn}"))
                }),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
        h.session
            .append("step/end", json!({"turn":turn,"step":1}), None)
            .unwrap();
    }
    let current = h.meter().measure(&h.ctx, &h.session, None).unwrap();
    assert!(matches!(
        current.baseline,
        TokenMeasurementBaseline::Usage { tokens: 200.0, .. }
    ));
    let override_header = json!({"config":{"provider":"test","model":"first"}});
    let overridden = h
        .meter()
        .measure(&h.ctx, &h.session, Some(&override_header))
        .unwrap();
    assert!(matches!(
        overridden.baseline,
        TokenMeasurementBaseline::Estimated { .. }
    ));
    h.close().await;
}

#[tokio::test]
async fn empty_assistant_surface_anchor_is_priced_as_zero() {
    let h = Harness::new().await;
    h.session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    h.session
        .append(
            "request/header",
            json!({"reason":"initial","header":{"config":{"provider":"test","model":"m"}}}),
            None,
        )
        .unwrap();
    let assistant_event = h
        .session
        .append(
            "assistant/message",
            json!({
                "turn":1,"step":1,"stream":[],"usage":usage(20,0,0,0),
                "message":assistant("", "empty")
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let measured = h.meter().measure(&h.ctx, &h.session, None).unwrap();
    assert_eq!(measured.nodes.len(), 1);
    assert_eq!(measured.nodes[0].seq, assistant_event.seq());
    assert_eq!(measured.nodes[0].tokens, 0);
    assert_eq!(measured.surface_tokens, 0);
    h.close().await;
}

#[tokio::test]
async fn seeded_replacement_has_signed_deltas_and_does_not_mutate_an_earlier_snapshot() {
    let h = Harness::new().await;
    h.session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    h.session
        .append(
            "request/header",
            json!({"reason":"initial","header":{"config":{"provider":"test","model":"m"}}}),
            None,
        )
        .unwrap();
    let head = h
        .session
        .append(
            "user/message",
            user(&"large anchored prompt ".repeat(100), "head"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    h.session
        .append(
            "assistant/message",
            json!({
                "turn":1,"step":1,"stream":[],"usage":usage(2000,10,0,0),
                "message":assistant("", "anchor")
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    h.session
        .append("step/end", json!({"turn":1,"step":1}), None)
        .unwrap();
    h.session
        .append(
            "user/message",
            user("new tail", "tail"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let seed = h.session.snapshot_events(None, None);
    let seeded = Session::create("surface-seeded".into(), Some(&seed), None, None, None).unwrap();
    let before = h.meter().measure(&h.ctx, &seeded, None).unwrap();
    assert!(before.surface_delta_tokens > 0.0);
    let replacement = seeded
        .append(
            "user/message",
            user(".", "replacement"),
            Some(SurfaceIntent::replace(
                head.seq(),
                head.seq(),
                vec![head.seq()],
            )),
        )
        .unwrap();
    let after = h.meter().measure(&h.ctx, &seeded, None).unwrap();
    assert!(after.surface_delta_tokens < 0.0);
    assert_eq!(after.nodes[0].seq, replacement.seq());
    assert_eq!(before.nodes[0].seq, head.seq());
    assert!(before.surface_delta_tokens > 0.0);
    assert!(after.log_revision > before.log_revision);
    h.close().await;
}

#[tokio::test]
async fn logged_offload_reprices_one_duplicate_without_rewriting_the_usage_anchor() {
    let h = Harness::new().await;
    let message_projections: MessageProjections = Arc::new(Mutex::new(vec![
        hoh_compaction_image_offload::image_offload_projection(),
    ]));
    let session = Session::create(
        "offload-session".into(),
        None,
        None,
        None,
        Some(message_projections.clone()),
    )
    .unwrap();
    let attachment = json!({
        "attachmentId":"same","mediaType":"image/png","bytes":8,"width":1,"height":1
    });
    let source = session
        .append(
            "user/message",
            json!({
                "role":"user","content":[
                    {"type":"image","attachment":attachment.clone()},
                    {"type":"image","attachment":attachment}
                ],
                "source":{"kind":"user"},"id":"images"
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    session
        .append(
            "request/header",
            json!({"reason":"initial","header":{"config":{"provider":"scoped","model":"vision"}}}),
            None,
        )
        .unwrap();
    session
        .append(
            "assistant/message",
            json!({
                "turn":1,"step":1,"stream":[],"usage":usage(5000,10,0,0),
                "message":assistant("", "anchor")
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let caller = h
        .ctx
        .isolate("offload", Realm::new())
        .child("offload-caller")
        .unwrap();
    caller.fiber().settled().await.unwrap();
    let llm = Arc::new(LlmRuntime::new());
    llm.register_adapter(&caller, vec!["scoped".into()], Arc::new(CallerAdapter))
        .unwrap();
    caller.provide::<Llm>(llm).await.unwrap();
    let before = h.meter().measure(&caller, &session, None).unwrap();
    session
        .append(
            "image/offload",
            json!({"targets":[{"seq":source.seq(),"imageIndexes":[0]}]}),
            None,
        )
        .unwrap();
    let after = h.meter().measure(&caller, &session, None).unwrap();
    assert_eq!(after.baseline, before.baseline);
    assert!(after.total_tokens < before.total_tokens);
    assert!(after.nodes[0].tokens > 333);
    assert_eq!(after.nodes[0].seq, before.nodes[0].seq);
    assert_eq!(
        after.nodes[0].heuristic_tokens,
        before.nodes[0].heuristic_tokens
    );
    let replay = Session::create(
        "offload-replay".into(),
        Some(&session.snapshot_events(None, None)),
        None,
        None,
        Some(message_projections),
    )
    .unwrap();
    assert_eq!(
        h.meter()
            .measure(&caller, &replay, None)
            .unwrap()
            .surface_tokens,
        after.surface_tokens
    );
    h.close().await;
}

#[tokio::test]
async fn token_usage_pressure_and_breakdown_projections_follow_the_log() {
    let h = Harness::new().await;
    h.session
        .append("request/context", json!({"contextWindow":4096}), None)
        .unwrap();
    h.session
        .append(
            "request/header",
            json!({"reason":"initial","header":{"tools":[{"name":"read"}]}}),
            None,
        )
        .unwrap();
    h.session
        .append(
            "system/message",
            json!({"message":system("You are terse.", "s1")}),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    h.session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    h.session
        .append(
            "assistant/message",
            json!({
                "turn":1,"step":1,"stream":[],"usage":usage(10,4,7,2),
                "message":assistant("done", "a1")
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    assert_eq!(
        snapshot_value(&h.ctx, &h.session, "tokenUsage"),
        json!({
            "uncachedInputTokens":10,"outputTokens":4,"cacheReadTokens":7,"cacheWriteTokens":2
        })
    );
    assert_eq!(
        snapshot_value(&h.ctx, &h.session, "contextPressure"),
        json!({
            "pressureTokens":19,"projectedTokens":28,"contextWindow":4096
        })
    );
    let breakdown = snapshot_value(&h.ctx, &h.session, "contextBreakdown");
    assert_eq!(breakdown["systemTokens"], 8);
    assert!(breakdown["toolsTokens"].as_u64().unwrap() > 0);
    assert!(breakdown["messageTokens"].as_u64().unwrap() > 0);
    assert_eq!(
        h.meter().context_pressure(&h.session).unwrap(),
        hoh_token_meter::ContextPressureProjection {
            pressure_tokens: Some(19),
            projected_tokens: None,
            context_window: Some(4096),
        }
    );
    h.close().await;
}

#[tokio::test]
async fn usage_samples_replace_within_attempt_and_accumulate_after_retry_started() {
    let h = Harness::new().await;
    h.session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    let attempt = |usage: Value| {
        json!({
            "turn":1,"step":1,
            "stream":[{"type":"chunk","time":0,"chunk":{"type":"usage","usage":usage}}]
        })
    };
    h.session
        .append("assistant/attempt", attempt(usage(10, 2, 3, 0)), None)
        .unwrap();
    h.session
        .append(
            "llm/retry-started",
            json!({"turn":1,"step":1,"retry":1,"retryId":"r"}),
            None,
        )
        .unwrap();
    h.session
        .append("assistant/attempt", attempt(usage(14, 5, 8, 1)), None)
        .unwrap();
    assert_eq!(
        snapshot_value(&h.ctx, &h.session, "tokenUsage"),
        json!({
            "uncachedInputTokens":24,"outputTokens":7,"cacheReadTokens":11,"cacheWriteTokens":1
        })
    );
    h.close().await;
}

#[tokio::test]
async fn invalid_measurement_history_keeps_projection_snapshots_available_but_measure_rethrows() {
    let h = Harness::new().await;
    h.meter().measure(&h.ctx, &h.session, None).unwrap();
    h.session
        .append(
            "assistant/message",
            json!({"turn":1,"step":1,"stream":[],"message":assistant("", "a")}),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    assert!(
        h.ctx
            .get::<SessionProjections>()
            .unwrap()
            .snapshot(&h.session, None)
            .is_ok()
    );
    let error = h
        .meter()
        .measure(&h.ctx, &h.session, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no matching step/start event"));
    let repeated = h
        .meter()
        .measure(&h.ctx, &h.session, None)
        .unwrap_err()
        .to_string();
    assert_eq!(repeated, error);
    h.close().await;
}

#[tokio::test]
async fn completed_step_boundaries_reject_overlapping_late_and_mismatched_events_repeatedly() {
    let h = Harness::new().await;
    h.session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    h.session
        .append("step/start", json!({"turn":1,"step":2}), None)
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            h.meter()
                .measure(&h.ctx, &h.session, None)
                .unwrap_err()
                .to_string(),
            "token meter: step/start at seq 1 arrived before turn 1/step 1 ended"
        );
    }

    let late = Session::create("late-assistant".into(), None, None, None, None).unwrap();
    late.append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    late.append("step/end", json!({"turn":1,"step":1}), None)
        .unwrap();
    late.append(
        "assistant/message",
        json!({"turn":1,"step":1,"stream":[],"message":assistant("", "late")}),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            h.meter()
                .measure(&h.ctx, &late, None)
                .unwrap_err()
                .to_string(),
            "token meter: assistant/message at seq 2 has no matching step/start event"
        );
    }

    let mismatched = Session::create("mismatched-end".into(), None, None, None, None).unwrap();
    mismatched
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    mismatched
        .append("step/end", json!({"turn":1,"step":2}), None)
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            h.meter()
                .measure(&h.ctx, &mismatched, None)
                .unwrap_err()
                .to_string(),
            "token meter: step/end at seq 1 has no matching step/start event"
        );
    }
    h.close().await;
}

#[tokio::test]
async fn eager_observation_catches_up_and_service_reload_replays_the_same_revision() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let sessions = Arc::new(SessionStore::new());
    ctx.provide::<Sessions>(sessions.clone()).await.unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let active = Arc::new(Mutex::new(None::<hoh_kernel::Service<TokenMeter>>));
    let revisions = Arc::new(Mutex::new(Vec::new()));
    let listener_active = active.clone();
    let listener_revisions = revisions.clone();
    let listener_ctx = ctx.clone();
    ctx.on_contained::<SessionEventEmitted>(move |event| {
        if let Some(meter) = listener_active.lock().unwrap().clone() {
            listener_revisions.lock().unwrap().push(
                meter
                    .measure(&listener_ctx, &event.session, None)?
                    .log_revision
                    .get(),
            );
        }
        Ok(())
    })
    .unwrap();
    let first = ctx.plugin::<TokenMeterPlugin>(json!({})).unwrap();
    first.settled().await.unwrap();
    *active.lock().unwrap() = ctx.get::<TokenMeter>();
    let seed = Arc::new(SessionEvent::from_value(
        json!({"seq":0,"time":1,"type":"turn/start","data":{"turn":1}}),
    ));
    let session = sessions
        .create(
            &ctx,
            Some("listener-order".into()),
            CreateSessionOptions {
                seed: Some(vec![seed]),
                ..Default::default()
            },
        )
        .unwrap();
    active
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .measure(&ctx, &session, None)
        .unwrap();
    session
        .append(
            "user/message",
            user("one", "u"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    assert_eq!(*revisions.lock().unwrap(), vec![3]);
    first.dispose().await;
    let second = ctx.plugin::<TokenMeterPlugin>(json!({})).unwrap();
    second.settled().await.unwrap();
    let reloaded = ctx.get::<TokenMeter>().unwrap();
    assert_eq!(
        reloaded
            .measure(&ctx, &session, None)
            .unwrap()
            .log_revision
            .get(),
        3
    );
    second.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn checkpoint_round_trip_and_plugin_disposal_remove_all_four_units() {
    let h = Harness::new().await;
    h.session
        .append(
            "user/message",
            user("hello", "u1"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let projections = h.ctx.get::<SessionProjections>().unwrap();
    let checkpoint = projections.checkpoint(&h.session).unwrap();
    for key in [
        "tokenMeasurement",
        "tokenUsage",
        "contextPressure",
        "contextBreakdown",
    ] {
        assert!(checkpoint.contains_key(key));
    }
    let encoded = serde_json::to_value(&checkpoint).unwrap();
    let decoded = serde_json::from_value(encoded).unwrap();
    let header: SessionHeader = serde_json::from_value((*h.session.header()).clone()).unwrap();
    assert!(
        projections
            .restore(
                &decoded,
                &[],
                h.session.seq(),
                &header,
                h.session.inherited_event_count()
            )
            .is_ok()
    );
    h.meter_fiber.dispose().await;
    assert!(h.ctx.get::<TokenMeter>().is_none());
    let values = projections.snapshot(&h.session, None).unwrap().values;
    for key in [
        "tokenMeasurement",
        "tokenUsage",
        "contextPressure",
        "contextBreakdown",
    ] {
        assert!(!values.contains_key(key));
    }
    let reloaded = h.ctx.plugin::<TokenMeterPlugin>(json!({})).unwrap();
    reloaded.settled().await.unwrap();
    assert!(h.ctx.get::<TokenMeter>().is_some());
    assert!(
        h.ctx
            .get::<TokenMeter>()
            .unwrap()
            .measure(&h.ctx, &h.session, None)
            .is_ok()
    );
    h.kernel.shutdown().await;
}

#[test]
fn measurement_state_schema_rejects_cache_fields_and_negative_references() {
    let definition = measurement_projection_definition();
    let init = serde_json::to_value(MeasurementState::default()).unwrap();
    let mut cache = init.clone();
    cache["surface"] = json!([]);
    assert!((definition.state_schema)(Some(&cache)).is_err());
    let mut negative = init;
    negative["headerSeq"] = json!(-1);
    assert!((definition.state_schema)(Some(&negative)).is_err());

    let mut malformed_usage = serde_json::to_value(MeasurementState::default()).unwrap();
    malformed_usage["anchor"] = json!({
        "headerSeq":null,"assistantTokens":0,
        "usage":{"inputTokens":1,"outputTokens":2,"cacheReadTokens":"bad"}
    });
    assert!((definition.state_schema)(Some(&malformed_usage)).is_err());
}

#[test]
fn measurement_json_uses_javascript_integer_spelling() {
    let value = serde_json::to_value(hoh_token_meter::TokenMeasurement {
        log_revision: SessionLogOffset::default(),
        baseline: TokenMeasurementBaseline::None { tokens: 0.0 },
        surface_delta_tokens: -2.0,
        total_tokens: 0.0,
        surface_tokens: 0,
        nodes: vec![],
    })
    .unwrap();
    assert_eq!(value["baseline"]["tokens"], json!(0));
    assert_eq!(value["surfaceDeltaTokens"], json!(-2));
    assert_eq!(value["totalTokens"], json!(0));
}

#[tokio::test]
async fn standalone_service_measures_synchronously_before_projection_activation() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let meter = TokenMeterRuntime::new(ctx);
    let session = Session::create("standalone-meter".into(), None, None, None, None).unwrap();
    session
        .append(
            "user/message",
            user("immediate", "u1"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let measurement = meter.measure(&kernel.root(), &session, None).unwrap();
    assert_eq!(measurement.log_revision, session.seq());
    assert_eq!(measurement.nodes.len(), 1);
}
