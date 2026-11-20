//! Port of tool-result-pruner.spec.ts.

use hoh_compaction_image_offload::image_offload_projection;
use hoh_compaction_tool_result_pruner::{
    DEFAULTS, PRUNE_MARKER, ToolResultPruneConfig, ToolResultPruner, ToolResultPrunerPlugin,
    code_point_length, resolve_config,
};
use hoh_kernel::{Kernel, Service};
use hoh_llm::{
    AssistantProviderMetadata, ContentBlock, Message, NewAssistantMessage, ToolCallId,
    ToolResultMessageInput, create_assistant_message, create_tool_result_message,
};
use hoh_session::{Session, SessionId, SessionSeq, SurfaceIntent};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_token_meter::{TokenMeter, TokenMeterPlugin};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn small() -> Value {
    json!({ "thresholdChars": 50, "headChars": 4, "tailChars": 3 })
}

struct World {
    kernel: Kernel,
    pruner: Service<ToolResultPruner>,
    meter: Service<TokenMeter>,
}

async fn world(config: Value) -> World {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<TokenMeterPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolResultPrunerPlugin>(config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    World {
        pruner: ctx.get::<ToolResultPruner>().unwrap(),
        meter: ctx.get::<TokenMeter>().unwrap(),
        kernel,
    }
}

fn cfg(value: Value) -> ToolResultPruneConfig {
    serde_json::from_value(value).unwrap()
}

fn text(text: &str) -> ContentBlock {
    ContentBlock::text(text)
}

fn block(value: Value) -> ContentBlock {
    serde_json::from_value(value).unwrap()
}

fn session(id: &str) -> Session {
    Session::create(SessionId::from(id), None, None, None, None).unwrap()
}

/// `appendToolStep`: one complete turn ending in a tool result; returns the result seq.
fn append_tool_step(
    session: &Session,
    turn: u64,
    call: &str,
    content: Vec<ContentBlock>,
    extra: Value,
) -> SessionSeq {
    let call_id = ToolCallId::from(call);
    session
        .append("turn/start", json!({ "turn": turn }), None)
        .unwrap();
    session
        .append("step/start", json!({ "turn": turn, "step": 1 }), None)
        .unwrap();
    let assistant = create_assistant_message(NewAssistantMessage {
        content: vec![block(
            json!({ "type": "tool-call", "id": call, "name": "bash", "arguments": "{}" }),
        )],
        source: AssistantProviderMetadata {
            provider: "test-model".into(),
            model: "test-model".into(),
            replay_state: None,
        },
    });
    session
        .append(
            "assistant/message",
            json!({
                "stream": [], "turn": turn, "step": 1,
                "message": serde_json::to_value(assistant).unwrap(),
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append(
            "tool/call",
            json!({
                "turn": turn, "step": 1, "callId": call, "name": "bash", "arguments": "{}"
            }),
            None,
        )
        .unwrap();
    let message = create_tool_result_message(ToolResultMessageInput {
        call_id,
        content,
        is_error: extra.get("error").is_some(),
    });
    let mut data = json!({
        "turn": turn, "step": 1, "message": serde_json::to_value(message).unwrap(),
    });
    for (key, value) in extra.as_object().into_iter().flatten() {
        data[key] = value.clone();
    }
    let result = session
        .append("tool/result", data, Some(SurfaceIntent::append()))
        .unwrap();
    session
        .append("step/end", json!({ "turn": turn, "step": 1 }), None)
        .unwrap();
    session
        .append(
            "turn/end",
            json!({ "turn": turn, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();
    result.seq()
}

fn start_turn(session: &Session, turn: u64) {
    session
        .append("turn/start", json!({ "turn": turn }), None)
        .unwrap();
}

fn events(session: &Session) -> Vec<Value> {
    session
        .snapshot_events(None, None)
        .iter()
        .map(|event| event.as_value().clone())
        .collect()
}

fn text_of(value: &ContentBlock) -> String {
    value.text_value().unwrap().to_owned()
}

#[tokio::test]
async fn preserves_a_logged_image_offload_when_pruning_the_same_result_later() {
    let world = world(small()).await;
    let projections = Arc::new(Mutex::new(vec![image_offload_projection()]));
    let session = Session::create(
        SessionId::from("prune-offloaded"),
        None,
        None,
        None,
        Some(projections.clone()),
    )
    .unwrap();
    let seq = append_tool_step(
        &session,
        1,
        "shot",
        vec![
            text(&"x".repeat(200)),
            block(json!({ "type": "image", "attachment": {
                "attachmentId": format!("sha256:{}", "a".repeat(64)),
                "mediaType": "image/png", "bytes": 1, "width": 1, "height": 1,
            } })),
        ],
        json!({}),
    );
    session
        .append(
            "image/offload",
            json!({ "targets": [{ "seq": seq, "imageIndexes": [0] }] }),
            None,
        )
        .unwrap();
    let pruned = world.pruner.prune_session(&session).unwrap();
    assert_eq!(pruned.pruned.len(), 1);
    let snapshot = session.snapshot_events(None, None);
    let replacement = snapshot.last().unwrap();
    assert_eq!(replacement.event_type(), "tool/result");
    let derived = session.derive_event_message(replacement).unwrap();
    assert!(
        serde_json::to_string(&derived)
            .unwrap()
            .contains("\"offloaded\":true")
    );
    let restored = Session::create(
        SessionId::from("restored-offloaded"),
        Some(&snapshot),
        None,
        None,
        Some(projections),
    )
    .unwrap();
    assert!(
        serde_json::to_string(&restored.derive_messages().unwrap())
            .unwrap()
            .contains("\"offloaded\":true")
    );
    assert!(
        !serde_json::to_string(&session.event_at(seq).unwrap())
            .unwrap()
            .contains("offloaded")
    );
    world.kernel.shutdown().await;
}

#[test]
fn resolves_detached_immutable_defaults_and_partial_overrides() {
    let mut raw = cfg(json!({ "thresholdChars": 100, "headChars": 20, "tailChars": 10 }));
    let resolved = resolve_config(&raw).unwrap();
    raw.0.insert("headChars".into(), json!(1));
    assert_eq!(
        serde_json::to_value(resolved).unwrap(),
        json!({ "thresholdChars": 100, "headChars": 20, "tailChars": 10 })
    );
    assert_eq!(
        serde_json::to_value(DEFAULTS).unwrap(),
        json!({ "thresholdChars": 8192, "headChars": 4096, "tailChars": 1024 })
    );
    assert_eq!(resolve_config(&cfg(json!({}))).unwrap(), DEFAULTS);
}

#[test]
fn rejects_stale_keys_invalid_scalars_and_an_output_budget_above_threshold() {
    let bad = [
        (
            json!({ "thresholdChars": 0 }),
            "ToolResultPruneConfig: thresholdChars (0) must be a positive integer",
        ),
        (
            json!({ "headChars": -1 }),
            "ToolResultPruneConfig: headChars (-1) must be a non-negative integer",
        ),
        (
            json!({ "tailChars": 1.5 }),
            "ToolResultPruneConfig: tailChars (1.5) must be a non-negative integer",
        ),
        (
            json!({ "thresholdChars": 50, "headChars": 20, "tailChars": 20 }),
            "ToolResultPruneConfig: headChars + marker + tailChars (79) must be at most thresholdChars (50)",
        ),
        (
            json!({ "threshold": 10 }),
            "ToolResultPruneConfig: unknown key \"threshold\" (allowed: thresholdChars, headChars, tailChars)",
        ),
    ];
    for (config, message) in bad {
        assert_eq!(
            resolve_config(&cfg(config)).unwrap_err().to_string(),
            message
        );
    }
}

#[tokio::test]
async fn measures_text_code_points_only_and_skips_content_within_threshold() {
    let world = world(small()).await;
    let blocks = [
        text("a😀b"),
        block(json!({ "type": "reasoning", "text": "not measured" })),
    ];
    assert_eq!(world.pruner.measure_content(&blocks), 3);
    assert!(world.pruner.prune_content(&blocks).unwrap().is_none());
    assert_eq!(code_point_length("a😀b"), 3);
    world.kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_configured_head_and_tail_without_splitting_surrogate_pairs() {
    let world = world(small()).await;
    let result = world
        .pruner
        .prune_content(&[text(&"😀".repeat(60))])
        .unwrap()
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(
        text_of(&result[0]),
        format!("{}{PRUNE_MARKER}{}", "😀".repeat(4), "😀".repeat(3))
    );
    assert!(world.pruner.measure_content(&result) <= 50);
    assert!(!text_of(&result[0]).contains('\u{FFFD}'));
    world.kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_non_text_blocks_and_their_relative_ordering_across_removed_text() {
    let world = world(small()).await;
    let reasoning = block(json!({ "type": "reasoning", "text": "private-rich-block" }));
    let call =
        block(json!({ "type": "tool-call", "id": "nested", "name": "nested", "arguments": "{}" }));
    let result = world
        .pruner
        .prune_content(&[
            text(&"A".repeat(40)),
            reasoning.clone(),
            text(&"B".repeat(30)),
            call.clone(),
            text(&"C".repeat(30)),
        ])
        .unwrap()
        .unwrap();
    assert_eq!(
        result,
        vec![
            text(&format!("AAAA{PRUNE_MARKER}")),
            reasoning,
            call,
            text("CCC"),
        ]
    );
    assert!(world.pruner.measure_content(&result) <= 50);
    world.kernel.shutdown().await;
}

#[tokio::test]
async fn supports_zero_sized_head_and_tail_while_still_shrinking() {
    let world = world(json!({
        "thresholdChars": code_point_length(PRUNE_MARKER),
        "headChars": 0,
        "tailChars": 0,
    }))
    .await;
    let result = world
        .pruner
        .prune_content(&[text(&"x".repeat(100))])
        .unwrap()
        .unwrap();
    assert_eq!(result, vec![text(PRUNE_MARKER)]);
    assert_eq!(
        world.pruner.measure_content(&result),
        world.pruner.config.threshold_chars
    );
    world.kernel.shutdown().await;
}

#[tokio::test]
async fn prunes_a_stable_snapshot_preserves_all_data_and_cites_the_replaced_result() {
    let world = world(small()).await;
    let session = session("preserve");
    let original_seq = append_tool_step(
        &session,
        1,
        "one",
        vec![text(&"x".repeat(100))],
        json!({
            "isError": true,
            "error": { "name": "ExitError", "code": "EXIT_1" },
            "meta": { "diff": ["a", "b"] },
            "futureField": { "nested": true },
        }),
    );
    start_turn(&session, 2);

    let result = world.pruner.prune_session(&session).unwrap();
    assert_eq!(result.pruned.len(), 1);
    assert!(result.chars_removed > 0);
    let entry = &result.pruned[0];
    assert_eq!(entry.original_seq, original_seq);
    assert_eq!(entry.call_id, ToolCallId::from("one"));
    assert_eq!(entry.chars_before, 100);
    assert!(entry.chars_after <= 50);

    let all = events(&session);
    let original = &all[u64::from(original_seq) as usize];
    let replacement = &all[u64::from(entry.replacement_seq) as usize];
    assert_eq!(original["type"], "tool/result");
    assert_eq!(
        original["data"]["message"]["content"],
        json!([{ "type": "text", "text": "x".repeat(100) }])
    );
    assert_eq!(replacement["type"], "tool/result");
    let data = &replacement["data"];
    assert_eq!(data["turn"], 1);
    assert_eq!(data["step"], 1);
    assert_eq!(data["isError"], true);
    assert_eq!(data["message"]["source"]["kind"], "tool");
    assert_eq!(data["message"]["source"]["callId"], "one");
    assert_eq!(data["message"]["role"], "tool");
    assert_eq!(data["message"]["toolCallId"], "one");
    assert_eq!(data["message"]["isError"], true);
    assert_eq!(
        data["error"],
        json!({ "name": "ExitError", "code": "EXIT_1" })
    );
    assert_eq!(data["meta"], json!({ "diff": ["a", "b"] }));
    assert_eq!(data["futureField"], json!({ "nested": true }));
    assert_eq!(
        replacement["surfaceOp"],
        json!({ "op": "replace", "startSeq": original_seq, "endSeq": original_seq })
    );
    assert_eq!(replacement["sourceEventSeqs"], json!([original_seq]));
    assert!(
        !session
            .surface()
            .unwrap()
            .nodes()
            .unwrap()
            .contains(&original_seq)
    );

    // Shadow-price protocol: the metering event sits directly before the
    // replacement and prices the shadowed node with the shared estimator.
    let message: Message = serde_json::from_value(original["data"]["message"].clone()).unwrap();
    let prune = &all[u64::from(entry.replacement_seq) as usize - 1];
    assert_eq!(prune["type"], "compaction/prune");
    assert_eq!(
        prune["data"],
        json!({
            "shadowedRange": { "start": original_seq, "end": original_seq },
            "shadowedSeqs": [original_seq],
            "shadowedTokenCount": world.meter.estimate_message(&message),
        })
    );
    world.kernel.shutdown().await;
}

#[tokio::test]
async fn prunes_multiple_results_skips_short_ones_and_converges_in_one_pass() {
    let world = world(small()).await;
    let session = session("multiple");
    append_tool_step(&session, 1, "a", vec![text(&"A".repeat(100))], json!({}));
    append_tool_step(&session, 2, "b", vec![text("short")], json!({}));
    append_tool_step(&session, 3, "c", vec![text(&"C".repeat(80))], json!({}));
    start_turn(&session, 4);
    let first = world.pruner.prune_session(&session).unwrap();
    let second = world.pruner.prune_session(&session).unwrap();
    assert_eq!(
        first
            .pruned
            .iter()
            .map(|e| e.call_id.clone())
            .collect::<Vec<_>>(),
        vec![ToolCallId::from("a"), ToolCallId::from("c")]
    );
    assert_eq!(
        first.chars_removed,
        first
            .pruned
            .iter()
            .map(|e| e.chars_before - e.chars_after)
            .sum::<u64>()
    );
    assert_eq!(second, Default::default());
    world.kernel.shutdown().await;
}

#[tokio::test]
async fn replays_to_the_identical_pruned_model_messages() {
    let world = world(small()).await;
    let session = session("replay");
    append_tool_step(&session, 1, "a", vec![text(&"A".repeat(100))], json!({}));
    start_turn(&session, 2);
    world.pruner.prune_session(&session).unwrap();
    let replay = Session::create(
        session.id().clone(),
        Some(&session.snapshot_events(None, None)),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        replay.derive_messages().unwrap(),
        session.derive_messages().unwrap()
    );
    assert_eq!(
        replay.surface().unwrap().replace_generation().unwrap(),
        session.surface().unwrap().replace_generation().unwrap()
    );
    world.kernel.shutdown().await;
}
