mod common;

use common::stub_agent_on;
use hoh_agent::{
    Agent, AgentModel, AgentOptions, AgentPreStep, Live, Model, ModelSelection,
    ModelSelectionProjectionState, PreStepDecision, agent_events, install_model_selection,
    model_selection_projection_definition, model_selection_view,
};
use hoh_kernel::{Context, Kernel};
use hoh_llm::{
    ContentBlock, MessageSource, NewUserMessage, ReasoningEffortId, UserMessage,
    create_user_message,
};
use hoh_session::{Session, SessionId};
use hoh_session_projection::{ProjectionHandle, SessionProjectionRegistry};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn selection(provider: &str, model: &str) -> ModelSelection {
    ModelSelection {
        provider: provider.into(),
        model: model.into(),
        reasoning_effort: None,
    }
}
fn with_effort(mut base: ModelSelection, effort: &str) -> ModelSelection {
    base.reasoning_effort = Some(ReasoningEffortId::from(effort));
    base
}
fn initial() -> ModelSelection {
    selection("alpha", "a0")
}
fn input() -> UserMessage {
    static INPUT: std::sync::OnceLock<UserMessage> = std::sync::OnceLock::new();
    INPUT
        .get_or_init(|| {
            create_user_message(NewUserMessage {
                content: vec![ContentBlock::text("continue")],
                source: MessageSource::new("user"),
            })
        })
        .clone()
}

type Options = Arc<Mutex<(Option<String>, Option<String>)>>;

struct Harness {
    kernel: Kernel,
    ctx: Context,
    agent: Arc<dyn Agent>,
    session: Session,
    model: AgentModel,
    options: Options,
    projection: Arc<ProjectionHandle<ModelSelectionProjectionState>>,
}

async fn harness(start: Option<(&str, &str)>) -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let projection = Arc::new(
        SessionProjectionRegistry::new(&ctx)
            .unwrap()
            .register(&ctx, model_selection_projection_definition())
            .await
            .unwrap(),
    );
    let session =
        Session::create(SessionId::from("model-selection"), None, None, None, None).unwrap();
    let options: Options = Arc::new(Mutex::new((
        start.map(|(p, _)| p.to_owned()),
        start.map(|(_, m)| m.to_owned()),
    )));
    let (p, m) = (options.clone(), options.clone());
    let agent_options = AgentOptions {
        provider: Some(Live::Accessor(Arc::new(move || {
            p.lock().unwrap().0.clone()
        }))),
        model: Some(Live::Accessor(Arc::new(move || {
            m.lock().unwrap().1.clone()
        }))),
        ..AgentOptions::default()
    };
    let (handle, s) = (projection.clone(), session.clone());
    let model = AgentModel::new(
        Arc::new(move || handle.state_of(&s).unwrap()),
        session.clone(),
        agent_options,
    );
    let agent = stub_agent_on(&ctx, "model-selection", session.clone());
    Harness {
        kernel,
        ctx,
        agent,
        session,
        model,
        options,
        projection,
    }
}

impl Harness {
    fn current(&self) -> Option<ModelSelection> {
        self.model.current()
    }
    fn record(&self, selection: &ModelSelection) {
        self.model.select(selection).unwrap();
    }
    fn header(&self, config: &ModelSelection, adapter_defaults: Option<Value>) {
        let mut header = json!({ "config": serde_json::to_value(config).unwrap() });
        if let Some(defaults) = adapter_defaults {
            header["adapterDefaults"] = defaults;
        }
        self.session
            .append(
                "request/header",
                json!({ "header": header, "reason": "initial" }),
                None,
            )
            .unwrap();
    }
    fn state(&self) -> Arc<ModelSelectionProjectionState> {
        self.projection.state_of(&self.session).unwrap()
    }
    fn event_count(&self) -> usize {
        self.session.snapshot_events(None, None).len()
    }
    async fn pre_step(
        &self,
        selected: &ModelSelection,
        messages: Vec<UserMessage>,
        offered: Vec<UserMessage>,
        step: u64,
        signal: AbortSignal,
    ) -> PreStepDecision {
        let selected = selected.clone();
        let entering = messages;
        agent_events(&self.ctx, &self.agent)
            .waterfall::<AgentPreStep, PreStepDecision, _>(
                move |agent| AgentPreStep {
                    agent,
                    messages: offered,
                    turn: 1,
                    step,
                    signal,
                    model_selection: selected,
                },
                move |_| async move {
                    Ok(PreStepDecision::Enter {
                        messages: entering,
                        starts_request_series: true,
                    })
                },
            )
            .await
            .unwrap()
    }
    async fn simple(&self, selected: &ModelSelection) -> Vec<UserMessage> {
        let signal = AbortController::new().signal();
        match self
            .pre_step(selected, vec![input()], vec![input()], 1, signal)
            .await
        {
            PreStepDecision::Enter { messages, .. } => messages,
            PreStepDecision::Reject => panic!("unexpected rejection"),
        }
    }
}

fn expected_notice(from: &str, to: &str) -> (Vec<ContentBlock>, MessageSource) {
    let mut source = MessageSource::new("model-selection");
    source.data.insert("form".into(), "notice".into());
    source
        .data
        .insert("summary".into(), format!("{from} → {to}").into());
    (
        vec![ContentBlock::text(format!(
            "[model changed: assistant turns above this point were generated by {from}; the session continues with {to}]"
        ))],
        source,
    )
}

fn is_notice(message: &UserMessage, from: &str, to: &str) -> bool {
    let (content, source) = expected_notice(from, to);
    message.content == content && message.source == source
}

#[tokio::test]
async fn uses_initial_options_only_until_a_selection_or_request_is_recorded_and_returns_detached_values()
 {
    let h = harness(None).await;
    assert_eq!(h.current(), None);
    h.options.lock().unwrap().0 = Some("alpha".into());
    assert_eq!(h.current(), None);
    h.options.lock().unwrap().1 = Some("a0".into());
    assert_eq!(h.current(), Some(initial()));
    let wire = model_selection_view(&h.state());
    assert_eq!(
        serde_json::to_value(wire).unwrap(),
        json!({ "lastUsed": null, "next": null })
    );

    h.record(&initial());
    let events = h.session.snapshot_events(None, None);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type(), "model/selection");
    assert_eq!(
        events[0].data(),
        &json!({ "provider": "alpha", "model": "a0" })
    );
    let mut selected = with_effort(selection("beta", "b1"), "high");
    h.record(&selected);
    selected.model = "mutated".into();
    let mut read = h.current().unwrap();
    read.model = "also mutated".into();
    assert_eq!(
        h.current(),
        Some(with_effort(selection("beta", "b1"), "high"))
    );
    h.record(&h.current().unwrap());
    assert_eq!(h.event_count(), 2);
    h.options.lock().unwrap().1 = Some("other initial model".into());
    assert_eq!(h.current().unwrap().model, "b1");
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn retires_only_a_matching_pending_selection_and_reconstructs_it_from_the_log() {
    let h = harness(Some(("alpha", "a0"))).await;
    h.header(&initial(), None);
    let selected = with_effort(selection("beta", "b1"), "high");
    h.record(&selected);
    for config in [
        ModelSelection {
            provider: "other".into(),
            ..selected.clone()
        },
        ModelSelection {
            model: "other".into(),
            ..selected.clone()
        },
        with_effort(selection("beta", "b1"), "low"),
    ] {
        h.header(&config, None);
        assert_eq!(h.state().pending, Some(selected.clone()));
    }
    let restored = Session::create(
        SessionId::from("restored"),
        Some(&h.session.snapshot_events(None, None)),
        None,
        None,
        None,
    )
    .unwrap();
    let handle = h.projection.clone();
    let restored_model = AgentModel::new(
        Arc::new({
            let restored = restored.clone();
            move || handle.state_of(&restored).unwrap()
        }),
        restored,
        AgentOptions::default(),
    );
    assert_eq!(restored_model.current(), Some(selected.clone()));
    h.header(&selected, None);
    assert_eq!(
        *h.state(),
        ModelSelectionProjectionState {
            last_used: Some(selected.clone()),
            pending: None
        }
    );
    assert_eq!(h.current(), Some(selected.clone()));
    let state = h.state();
    h.header(&selected, None);
    assert!(Arc::ptr_eq(&state, &h.state()) || *state == *h.state());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn leaves_adapter_defaulted_reasoning_unselected_while_retaining_explicit_effort() {
    let h = harness(Some(("alpha", "a0"))).await;
    let selected = selection("beta", "b1");
    h.record(&selected);
    h.header(
        &with_effort(selected.clone(), "high"),
        Some(json!({ "reasoningEffort": true })),
    );
    assert_eq!(
        *h.state(),
        ModelSelectionProjectionState {
            last_used: Some(selected.clone()),
            pending: None
        }
    );
    assert_eq!(h.current(), Some(selected.clone()));
    assert_eq!(
        serde_json::to_value(model_selection_view(&h.state())).unwrap(),
        json!({ "lastUsed": { "provider": "beta", "model": "b1" }, "next": { "provider": "beta", "model": "b1" } })
    );
    h.header(&with_effort(selected.clone(), "low"), None);
    assert_eq!(h.current(), Some(with_effort(selected, "low")));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn reuses_fold_state_for_duplicate_recorded_selections_and_unrelated_events() {
    let h = harness(Some(("alpha", "a0"))).await;
    let selected = selection("beta", "b1");
    h.record(&selected);
    let state = h.state();
    h.session
        .append(
            "model/selection",
            serde_json::to_value(&selected).unwrap(),
            None,
        )
        .unwrap();
    h.session
        .append(
            "user/message",
            serde_json::to_value(input()).unwrap(),
            Some(hoh_session::SurfaceIntent::append()),
        )
        .unwrap();
    assert_eq!(*h.state(), *state);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn replaces_the_complete_selection_and_clears_an_omitted_explicit_effort() {
    let h = harness(Some(("alpha", "a0"))).await;
    h.record(&with_effort(initial(), "high"));
    h.record(&initial());
    assert_eq!(h.current(), Some(initial()));
    assert_eq!(h.event_count(), 2);
    h.record(&initial());
    assert_eq!(h.event_count(), 2);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn announces_the_fixed_step_route_preserves_request_series_declarations_and_repeats_before_a_header_commits()
 {
    let h = harness(Some(("alpha", "a0"))).await;
    h.header(&initial(), None);
    let _listener = install_model_selection(&h.ctx).unwrap();
    h.record(&selection("alpha", "future"));
    let selected = selection("beta", "b1");
    for _ in 0..2 {
        let signal = AbortController::new().signal();
        let decision = h
            .pre_step(&selected, vec![input()], vec![input()], 1, signal)
            .await;
        let PreStepDecision::Enter {
            messages,
            starts_request_series,
        } = decision
        else {
            panic!("rejected")
        };
        assert!(starts_request_series);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0], input());
        assert!(is_notice(&messages[1], "alpha/a0", "beta/b1"));
    }
    h.header(&selected, None);
    assert_eq!(h.simple(&selected).await, vec![input()]);
    let messages = h.simple(&selection("beta", "b2")).await;
    assert_eq!(messages.len(), 2);
    assert!(is_notice(&messages[1], "b1", "b2"));
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn does_not_announce_initial_same_route_effort_only_rejected_aborted_or_disposed_steps() {
    let h = harness(Some(("alpha", "a0"))).await;
    let dispose = install_model_selection(&h.ctx).unwrap();
    assert_eq!(h.simple(&initial()).await, vec![input()]);
    h.header(&initial(), None);
    assert_eq!(h.simple(&initial()).await, vec![input()]);
    assert_eq!(
        h.simple(&with_effort(initial(), "high")).await,
        vec![input()]
    );
    let selected = selection("alpha", "a1");
    let rejected = agent_events(&h.ctx, &h.agent)
        .waterfall::<AgentPreStep, PreStepDecision, _>(
            {
                let selected = selected.clone();
                move |agent| AgentPreStep {
                    agent,
                    messages: vec![],
                    turn: 1,
                    step: 1,
                    signal: AbortController::new().signal(),
                    model_selection: selected,
                }
            },
            |_| async { Ok(PreStepDecision::Reject) },
        )
        .await
        .unwrap();
    assert_eq!(rejected, PreStepDecision::Reject);
    let aborted = AbortController::new();
    aborted.abort();
    let decision = h
        .pre_step(&selected, vec![input()], vec![input()], 1, aborted.signal())
        .await;
    assert_eq!(
        decision,
        PreStepDecision::Enter {
            messages: vec![input()],
            starts_request_series: true
        }
    );
    dispose.dispose().await;
    assert_eq!(h.simple(&selected).await, vec![input()]);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_empty_no_call_decisions_and_announces_an_empty_tool_continuation() {
    let h = harness(Some(("alpha", "a0"))).await;
    h.header(&initial(), None);
    let _listener = install_model_selection(&h.ctx).unwrap();
    let selected = selection("alpha", "a1");
    let signal = || AbortController::new().signal();
    let PreStepDecision::Enter { messages, .. } = h
        .pre_step(&selected, vec![], vec![input()], 1, signal())
        .await
    else {
        panic!()
    };
    assert!(messages.is_empty());
    let PreStepDecision::Enter { messages, .. } = h
        .pre_step(&selected, vec![], vec![input()], 2, signal())
        .await
    else {
        panic!()
    };
    assert!(messages.is_empty());
    let PreStepDecision::Enter { messages, .. } =
        h.pre_step(&selected, vec![], vec![], 2, signal()).await
    else {
        panic!()
    };
    assert_eq!(messages.len(), 1);
    assert!(is_notice(&messages[0], "a0", "a1"));
    h.kernel.shutdown().await;
}

// ---- persisted state schema ----------------------------------------------------------------

/// A checkpoint row of the real projection with the given persisted state, restored over an
/// empty log. `Err` is the registry's refusal to restore.
async fn restore_row(val: Value) -> Result<(), String> {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = SessionProjectionRegistry::new(&ctx).unwrap();
    registry
        .register(&ctx, model_selection_projection_definition())
        .await
        .unwrap();
    let checkpoint = [(
        "modelSelection".to_owned(),
        hoh_session_projection::ProjectionCheckpointRow {
            ver: 3,
            seq: hoh_session::SessionSeqCursor::BeforeFirst,
            val: Some(val),
        },
    )]
    .into_iter()
    .collect();
    let result = registry
        .restore(
            &checkpoint,
            &[],
            hoh_session::SessionLogOffset::new(0).unwrap(),
            &hoh_session::SessionHeader::new("restore-schema".into(), 0),
            hoh_session::SessionLogOffset::new(0).unwrap(),
        )
        .map(|_| ())
        .map_err(|error| error.to_string());
    kernel.shutdown().await;
    result
}

#[tokio::test]
async fn restore_rejects_a_checkpoint_state_without_its_required_keys() {
    for missing in [
        json!({}),
        json!({ "lastUsed": null }),
        json!({ "pending": null }),
        json!({ "lastUsed": { "provider": "alpha", "model": "a0" } }),
    ] {
        let error = restore_row(missing.clone()).await.unwrap_err();
        assert!(error.contains("modelSelection"), "{missing}: {error}");
    }
}

#[tokio::test]
async fn restore_rejects_a_malformed_selection_in_a_checkpoint_state() {
    let valid = json!({ "provider": "alpha", "model": "a0" });
    for malformed in [
        json!({ "lastUsed": 1, "pending": null }),
        json!({ "lastUsed": [], "pending": null }),
        json!({ "lastUsed": { "provider": "", "model": "a0" }, "pending": null }),
        json!({ "lastUsed": { "provider": "alpha" }, "pending": null }),
        json!({ "lastUsed": valid, "pending": { "provider": "alpha", "model": 3 } }),
        json!({ "lastUsed": null,
                "pending": { "provider": "alpha", "model": "a0", "reasoningEffort": "" } }),
        json!({ "lastUsed": null,
                "pending": { "provider": "alpha", "model": "a0", "reasoningEffort": null } }),
    ] {
        let error = restore_row(malformed.clone()).await.unwrap_err();
        assert!(error.contains("modelSelection"), "{malformed}: {error}");
    }
}

#[tokio::test]
async fn restore_accepts_null_and_complete_selections_in_a_checkpoint_state() {
    restore_row(json!({ "lastUsed": null, "pending": null }))
        .await
        .unwrap();
    restore_row(json!({
        "lastUsed": { "provider": "alpha", "model": "a0", "reasoningEffort": "high" },
        "pending": { "provider": "beta", "model": "b0" },
    }))
    .await
    .unwrap();
}

#[test]
fn the_wire_view_schema_requires_both_keys_too() {
    let view = model_selection_projection_definition()
        .wire
        .unwrap()
        .view_schema;
    assert!(view(Some(&json!({ "lastUsed": null, "next": null }))).is_ok());
    assert!(view(Some(&json!({ "lastUsed": null }))).is_err());
    assert!(
        view(Some(
            &json!({ "next": { "provider": "alpha", "model": "" }, "lastUsed": null })
        ))
        .is_err()
    );
    assert!(view(None).is_err());
}
