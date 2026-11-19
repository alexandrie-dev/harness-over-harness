mod common;

use common::*;
use futures_util::future::BoxFuture;
use hoh_compaction::*;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Kernel, NoConfig, Plugin};
use hoh_llm::{ContentBlock, MessageSource};
use hoh_session::{Session, SessionSeq, SurfaceIntent};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct StubCompactionEngine {
    last_signal: Mutex<Option<AbortSignal>>,
}

impl StubCompactionEngine {
    fn remember(&self, signal: AbortSignal) {
        *self.last_signal.lock().unwrap() = Some(signal);
    }
}

impl CompactionEngine for StubCompactionEngine {
    fn compact_if_needed<'a>(
        &'a self,
        _agent: &'a CompactionAgentContext,
        _trigger: CompactionTrigger,
        signal: AbortSignal,
    ) -> BoxFuture<'a, Result<Option<CompactionResult>, BoxError>> {
        self.remember(signal);
        Box::pin(async { Ok(None) })
    }

    fn compact_now<'a>(
        &'a self,
        _agent: &'a ManualCompactAgentContext,
        signal: AbortSignal,
        _source_command_id: Option<hoh_commands::CommandId>,
    ) -> BoxFuture<'a, Result<Option<CompactionResult>, BoxError>> {
        self.remember(signal);
        Box::pin(async { Ok(None) })
    }

    fn compact_region<'a>(
        &'a self,
        start: SessionSeq,
        end: SessionSeq,
        agent: &'a CompactionAgentContext,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<CompactionResult, BoxError>> {
        if let Some(signal) = signal {
            self.remember(signal);
        }
        Box::pin(async move {
            let surface = agent.session.surface()?.nodes()?;
            let start_index = surface
                .iter()
                .position(|seq| *seq == start)
                .ok_or_else(|| BoxError::from("stub compact range is invalid"))?;
            let end_index = surface
                .iter()
                .position(|seq| *seq == end)
                .filter(|index| *index >= start_index)
                .ok_or_else(|| BoxError::from("stub compact range is invalid"))?;
            let shadowed_seqs = surface[start_index..=end_index].to_vec();
            let compaction_id = CompactionId::from("stub-compaction");
            let start_event = CompactionStartEvent {
                compaction_id: compaction_id.clone(),
                source_command_id: None,
                turn: Some(0),
            };
            let start_event = agent.session.append(
                CompactionStartEvent::TYPE,
                serde_json::to_value(start_event)?,
                None,
            )?;
            let summary = vec![ContentBlock::text("stub")];
            let summary_event = CompactionSummaryEvent {
                compaction_id: compaction_id.clone(),
                source_command_id: None,
                summary: summary.clone(),
                output: CompactionSummaryOutput::other(None),
                shadowed_range: ShadowedRange { start, end },
                shadowed_seqs: shadowed_seqs.clone(),
                shadowed_token_count: 0,
                provider: "mock".into(),
                model: "stub".into(),
                max_tokens: None,
                usage: None,
            };
            let summary_event = agent.session.append(
                CompactionSummaryEvent::TYPE,
                serde_json::to_value(summary_event)?,
                None,
            )?;
            let source = compact_checkpoint_source(compaction_id.clone(), None);
            agent.session.append(
                "user/message",
                json!({
                    "content": summary,
                    "source": source,
                    "role": "user",
                    "id": "compaction-checkpoint",
                }),
                Some(SurfaceIntent::replace(
                    start,
                    end,
                    [
                        vec![start_event.seq(), summary_event.seq()],
                        shadowed_seqs.clone(),
                    ]
                    .concat(),
                )),
            )?;
            let end_event = CompactionEndEvent {
                compaction_id: compaction_id.clone(),
                source_command_id: None,
                turn: Some(0),
                error: None,
            };
            let end_event = agent.session.append(
                CompactionEndEvent::TYPE,
                serde_json::to_value(end_event)?,
                None,
            )?;
            Ok(CompactionResult {
                compaction_id,
                source_command_id: None,
                start_seq: start_event.seq(),
                summary_seq: summary_event.seq(),
                end_seq: end_event.seq(),
                summary,
                shadowed_range: ShadowedRange { start, end },
                shadowed_seqs,
                shadowed_token_count: 0,
            })
        })
    }
}

struct StubCompactionPlugin;

impl Plugin for StubCompactionPlugin {
    const NAME: &'static str = "stub-compaction";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(ctx: Context, _deps: (), _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        provide_compaction(&ctx, Arc::new(StubCompactionEngine::default())).await?;
        Ok(())
    }
}

fn agent(session: Session, model: Option<&str>) -> CompactionAgentContext {
    CompactionAgentContext {
        session,
        options: CompactionAgentOptions {
            provider: None,
            model: model.map(str::to_owned),
        },
    }
}

fn manual_agent(session: Session) -> ManualCompactAgentContext {
    ManualCompactAgentContext::new(
        session,
        CompactionAgentOptions::default(),
        Arc::new(|task| Ok(task(AbortController::new().signal()))),
    )
}

#[tokio::test]
async fn registers_as_ctx_compaction() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let disposer = provide_compaction(&ctx, Arc::new(StubCompactionEngine::default()))
        .await
        .unwrap();
    assert!(ctx.get::<Compaction>().is_some());
    disposer.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn disposing_the_owner_unregisters_ctx_compaction_for_hmr_safety() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<StubCompactionPlugin>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    assert!(ctx.get::<Compaction>().is_some());
    fiber.dispose().await;
    assert!(ctx.get::<Compaction>().is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn exposes_the_abstract_contract_methods() {
    let engine = StubCompactionEngine::default();
    let session = session("contract");
    assert!(
        engine
            .compact_if_needed(
                &agent(session.clone(), None),
                CompactionTrigger::Pressure,
                AbortController::new().signal(),
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        engine
            .compact_now(
                &manual_agent(session),
                AbortController::new().signal(),
                None,
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn compaction_events_are_log_only_and_checkpoint_is_correlated() {
    let engine = StubCompactionEngine::default();
    let session = session("events");
    let original = append_user(&session, "original");
    let result = engine
        .compact_region(
            original.seq(),
            original.seq(),
            &agent(session.clone(), Some("m")),
            None,
        )
        .await
        .unwrap();
    let events = session.snapshot_events(None, None);
    let start = events
        .iter()
        .find(|event| event.event_type() == CompactionStartEvent::TYPE)
        .unwrap();
    assert!(start.surface_op().is_none());
    assert_eq!(result.summary, vec![ContentBlock::text("stub")]);
    assert!(result.summary_seq > result.start_seq);
    assert!(result.end_seq > result.summary_seq);
    assert_eq!(
        result.shadowed_range,
        ShadowedRange {
            start: original.seq(),
            end: original.seq()
        }
    );
    assert_eq!(result.shadowed_seqs, vec![original.seq()]);
    let checkpoint = events
        .iter()
        .find(|event| {
            event.event_type() == "user/message"
                && event.data()["source"]["kind"] == "compact-checkpoint"
        })
        .unwrap();
    let source: MessageSource =
        serde_json::from_value(checkpoint.data()["source"].clone()).unwrap();
    assert!(is_compact_checkpoint_source(&source));
    assert_eq!(
        source.data["compactionId"],
        serde_json::Value::String(result.compaction_id.to_string())
    );
    assert!(!is_compact_checkpoint_source(&MessageSource::new("other")));
    assert!(!is_compact_checkpoint_source(&MessageSource::new("user")));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type().starts_with("compaction/"))
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        ["compaction/start", "compaction/summary", "compaction/end"]
    );
}

#[tokio::test]
async fn threads_the_cancellation_signal_through_to_the_backend() {
    let engine = StubCompactionEngine::default();
    let session = session("signal");
    let original = append_user(&session, "original");
    let controller = AbortController::new();
    engine
        .compact_region(
            original.seq(),
            original.seq(),
            &agent(session.clone(), Some("m")),
            Some(controller.signal()),
        )
        .await
        .unwrap();
    controller.abort();
    assert!(
        engine
            .last_signal
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .aborted()
    );

    let controller = AbortController::new();
    engine
        .compact_if_needed(
            &agent(session, None),
            CompactionTrigger::ContextOverflow,
            controller.signal(),
        )
        .await
        .unwrap();
    controller.abort();
    assert!(
        engine
            .last_signal
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .aborted()
    );
}

#[test]
fn serializes_event_payloads_with_source_key_order_and_optional_absence() {
    let value = serde_json::to_value(CompactionStartEvent {
        compaction_id: CompactionId::from("c"),
        source_command_id: None,
        turn: None,
    })
    .unwrap();
    assert_eq!(value, json!({"compactionId":"c","turn":null}));
    assert_eq!(
        value.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["compactionId", "turn"]
    );

    let checkpoint = serde_json::to_value(compact_checkpoint_source(
        CompactionId::from("c"),
        Some(hoh_commands::CommandId::from("cmd")),
    ))
    .unwrap();
    assert_eq!(
        checkpoint.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["kind", "compactionId", "sourceCommandId"]
    );

    let summary = CompactionSummaryEvent {
        compaction_id: CompactionId::from("c"),
        source_command_id: None,
        summary: vec![ContentBlock::text("summary")],
        output: CompactionSummaryOutput::llm_stream(vec![ContentBlock::text("raw")]),
        shadowed_range: ShadowedRange {
            start: seq(4),
            end: seq(7),
        },
        shadowed_seqs: vec![seq(4), seq(7)],
        shadowed_token_count: 9,
        provider: "provider".into(),
        model: "model".into(),
        max_tokens: None,
        usage: None,
    };
    let summary = serde_json::to_value(summary).unwrap();
    assert_eq!(
        summary.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "compactionId",
            "summary",
            "rawOutput",
            "llmStreamCall",
            "shadowedRange",
            "shadowedSeqs",
            "shadowedTokenCount",
            "provider",
            "model",
        ]
    );
    assert_eq!(summary["llmStreamCall"], true);

    let unmarked = serde_json::to_value(CompactionSummaryEvent {
        compaction_id: CompactionId::from("c"),
        source_command_id: None,
        summary: vec![],
        output: CompactionSummaryOutput::other(Some(vec![ContentBlock::text("raw")])),
        shadowed_range: ShadowedRange {
            start: seq(4),
            end: seq(7),
        },
        shadowed_seqs: vec![seq(4), seq(7)],
        shadowed_token_count: 9,
        provider: "provider".into(),
        model: "model".into(),
        max_tokens: None,
        usage: None,
    })
    .unwrap();
    assert_eq!(
        unmarked.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "compactionId",
            "summary",
            "rawOutput",
            "shadowedRange",
            "shadowedSeqs",
            "shadowedTokenCount",
            "provider",
            "model",
        ]
    );
    assert!(unmarked.get("llmStreamCall").is_none());
}
