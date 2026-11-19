use crate::{test_support::*, *};
use hoh_kernel::{Fiber, FiberState, Level};
use hoh_session_projection::{ProjectionDefinition, SessionProjectionRegistry};
use serde_json::json;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::oneshot;

fn checkpoint(h: &Harness, s: &Session) -> Value {
    serde_json::to_value(h.registry().checkpoint(s).unwrap()).unwrap()
}

// Per-test substitute for the source's register prototype spy. The production
// initializer awaits the same real registration, held only by this caller gate.
struct Delay {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}
struct DelayedApproval;
impl Plugin for DelayedApproval {
    const NAME: &'static str = "ApprovalService";
    type Config = Config;
    type Deps = ApprovalDeps;
    async fn apply(ctx: Context, deps: ApprovalDeps, config: Cfg<Config>) -> Result<(), BoxError> {
        let registration = deps.projections.register(&ctx, state::definition());
        let delay = ctx.tag::<Delay>().unwrap();
        let deferred = async move {
            let handle = registration.await?;
            lock(&delay.entered).take().unwrap().send(()).unwrap();
            let wait = lock(&delay.release).take().unwrap();
            let _ = wait.await;
            Ok(handle)
        };
        if let Some(service) =
            ApprovalService::initialize(ctx.clone(), (*config.get()).clone(), deferred).await?
        {
            ctx.provide::<Approval>(Arc::new(service)).await?;
        }
        Ok(())
    }
}
async fn delayed(h: &Harness) -> (Fiber, oneshot::Sender<()>) {
    let (entered, ready) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    let ctx = h.ctx.with_tag(Delay {
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(Some(wait)),
    });
    let f = ctx.plugin::<DelayedApproval>(json!({})).unwrap();
    ready.await.unwrap();
    (f, release)
}

#[tokio::test]
async fn keeps_only_the_current_policy_and_turn_flag_after_a_long_history() {
    let h = Harness::mounted().await;
    let a = h.agent(&[]);
    let s = a.session();
    set_approval_policy(s, ApprovalPolicy::Ask).unwrap();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    for turn in 1..=1000 {
        set_approval_policy(s, ApprovalPolicy::Never).unwrap();
        s.append(
            "turn/end",
            json!({"turn":turn,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
        s.append("turn/start", json!({"turn":turn+1}), None)
            .unwrap();
        set_approval_policy(s, ApprovalPolicy::Ask).unwrap();
    }
    assert_eq!(
        checkpoint(&h, s)["approvalState"]["val"],
        json!({"policy":"ask","turnOpen":true})
    );
    assert_eq!(
        h.service().override_of(s).unwrap(),
        Some(ApprovalPolicy::Ask)
    );
    h.close().await;
}
#[tokio::test]
async fn waits_for_its_required_registry_before_exposing_the_service() {
    let mut h = Harness::base().await;
    let f = h.ctx.plugin::<ApprovalService>(json!({})).unwrap();
    pump().await;
    assert!(h.ctx.get::<Approval>().is_none());
    h.registry_owner = Some(h.load::<SessionProjectionRegistry>(json!({})).await);
    f.settled().await.unwrap();
    let a = h.agent(&[]);
    let s = a.session();
    set_approval_policy(s, ApprovalPolicy::Ask).unwrap();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    set_approval_policy(s, ApprovalPolicy::Never).unwrap();
    assert_eq!(
        h.service().override_of(s).unwrap(),
        Some(ApprovalPolicy::Never)
    );
    assert_eq!(
        h.service().request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::Rejected
    );
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    assert!(
        h.service()
            .request(req(a.clone()))
            .await
            .unwrap_err()
            .to_string()
            .contains("outside an open turn")
    );
    h.close().await;
}
#[tokio::test]
async fn rebuilds_policy_and_turn_state_before_a_replacement_service_becomes_ready_without_replaying_decisions()
 {
    let h = Harness::context().await;
    let owner = h.load::<ApprovalService>(json!({})).await;
    let a = h.agent(&[]);
    let s = a.session();
    assert_eq!(h.service().override_of(s).unwrap(), None);
    assert!(h.service().request(req(a.clone())).await.is_err());
    for _ in 0..2 {
        s.append("turn/start", json!({"turn":1}), None).unwrap();
    }
    for _ in 0..2 {
        set_approval_policy(s, ApprovalPolicy::Never).unwrap();
    }
    s.append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    assert_eq!(
        checkpoint(&h, s)["approvalState"],
        json!({"ver":1,"seq":s.seq().get()-1,"val":{"policy":"never","turnOpen":true}})
    );
    assert!(
        h.registry()
            .snapshot(s, None)
            .unwrap()
            .values
            .get("approvalState")
            .is_none()
    );
    for _ in 0..2 {
        s.append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    }
    set_approval_policy(s, ApprovalPolicy::Ask).unwrap();
    owner.dispose().await;
    let called = Arc::new(AtomicUsize::new(0));
    let seen = called.clone();
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            async { Ok(ApprovalOutcome::AllowedOnce.into()) }
        })
        .unwrap();
    let before = s.seq();
    h.load::<ApprovalService>(json!({})).await;
    assert_eq!(
        h.service().override_of(s).unwrap(),
        Some(ApprovalPolicy::Ask)
    );
    assert!(h.service().request(req(a.clone())).await.is_err());
    assert_eq!(s.seq(), before);
    assert_eq!(called.load(Ordering::SeqCst), 0);
    s.append("turn/start", json!({"turn":2}), None).unwrap();
    assert_eq!(
        h.service().request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert_pair(s, ApprovalOutcome::AllowedOnce);
    assert_eq!(
        checkpoint(&h, s)["approvalState"]["val"],
        json!({"policy":"ask","turnOpen":true})
    );
    h.close().await;
}
#[tokio::test]
async fn uses_the_retained_reader_for_current_reads_and_releases_the_unit_with_the_service() {
    let h = Harness::context().await;
    let owner = h.load::<ApprovalService>(json!({})).await;
    let a = h.agent(&[]);
    let s = a.session();
    set_approval_policy(s, ApprovalPolicy::Never).unwrap();
    let service = h.service();
    assert_eq!(service.override_of(s).unwrap(), Some(ApprovalPolicy::Never));
    let first = service.approval_projection_handle.state_of(s).unwrap();
    let cut = s.seq();
    for _ in 0..2 {
        assert_eq!(service.override_of(s).unwrap(), Some(ApprovalPolicy::Never));
        assert!(Arc::ptr_eq(
            &first,
            &service.approval_projection_handle.state_of(s).unwrap()
        ));
    }
    assert_eq!(s.seq(), cut);
    let row = checkpoint(&h, s)["approvalState"].clone();
    owner.dispose().await;
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    assert_eq!(checkpoint(&h, s)["approvalState"], row);
    let another = h.agent(&[]);
    assert!(
        checkpoint(&h, another.session())
            .get("approvalState")
            .is_none()
    );
    h.load::<ApprovalService>(json!({})).await;
    assert_eq!(
        h.service().override_of(s).unwrap(),
        Some(ApprovalPolicy::Never)
    );
    assert_eq!(
        checkpoint(&h, s)["approvalState"]["val"],
        json!({"policy":"never","turnOpen":false})
    );
    h.close().await;
}
#[tokio::test]
async fn waits_for_registration_before_activating_its_synchronous_readers_and_prompt() {
    let h = Harness::context().await;
    h.load::<SystemPrompt>(json!({})).await;
    let (f, release) = delayed(&h).await;
    assert_eq!(f.state(), FiberState::Loading);
    let a = h.agent(&[]);
    set_approval_policy(a.session(), ApprovalPolicy::Never).unwrap();
    assert!(context_text(&h, Some(a.clone())).await.is_none());
    release.send(()).unwrap();
    f.settled().await.unwrap();
    let service = h.service();
    let first = service
        .approval_projection_handle
        .state_of(a.session())
        .unwrap();
    assert_eq!(
        service.override_of(a.session()).unwrap(),
        Some(ApprovalPolicy::Never)
    );
    assert_eq!(
        context_text(&h, Some(a.clone())).await.as_deref(),
        Some(NEVER_SENTENCE)
    );
    assert!(Arc::ptr_eq(
        &first,
        &service
            .approval_projection_handle
            .state_of(a.session())
            .unwrap()
    ));
    a.session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    assert_eq!(
        service.request(req(a)).await.unwrap(),
        ApprovalOutcome::Rejected
    );
    h.close().await;
}
#[tokio::test]
async fn fails_activation_when_its_projection_registration_fails() {
    let h = Harness::context().await;
    h.registry()
        .register(
            &h.ctx,
            ProjectionDefinition {
                key: "approvalState".into(),
                state_version: 2,
                state_schema: Arc::new(|_| Ok(Arc::new(String::new()))),
                init: Arc::new(|_, _| Ok(Arc::new(String::new()))),
                apply: Arc::new(|s, _| Ok(s.clone())),
                wire: None,
            },
        )
        .await
        .unwrap();
    let f = h.ctx.plugin::<ApprovalService>(json!({})).unwrap();
    assert!(
        f.settled()
            .await
            .unwrap_err()
            .to_string()
            .contains("stateVersion")
    );
    assert_eq!(f.state(), FiberState::Failed);
    assert!(h.ctx.get::<Approval>().is_none());
    assert!(
        h.ctx
            .logger()
            .buffer()
            .iter()
            .any(|r| r.level == Level::Error)
    );
    h.close().await;
}
#[tokio::test]
async fn provides_the_same_policy_when_the_optional_prompt_service_loads_or_restarts_later() {
    let h = Harness::mounted().await;
    let a = h.agent(&[]);
    set_approval_policy(a.session(), ApprovalPolicy::Never).unwrap();
    let prompt = h.load::<SystemPrompt>(json!({})).await;
    pump().await;
    let first = context_text(&h, Some(a.clone())).await;
    assert_eq!(first.as_deref(), Some(NEVER_SENTENCE));
    prompt.dispose().await;
    h.load::<SystemPrompt>(json!({})).await;
    pump().await;
    assert_eq!(context_text(&h, Some(a)).await, first);
    h.close().await;
}
#[tokio::test]
async fn withdraws_the_service_while_its_required_registry_is_unavailable() {
    let mut h = Harness::context().await;
    let approval = h.load::<ApprovalService>(json!({})).await;
    let a = h.agent(&[]);
    set_approval_policy(a.session(), ApprovalPolicy::Never).unwrap();
    assert_eq!(
        h.service().override_of(a.session()).unwrap(),
        Some(ApprovalPolicy::Never)
    );
    h.registry_owner.take().unwrap().dispose().await;
    assert!(h.ctx.get::<Approval>().is_none());
    h.registry_owner = Some(h.load::<SessionProjectionRegistry>(json!({})).await);
    approval.settled().await.unwrap();
    let next = h.agent(&[]);
    set_approval_policy(next.session(), ApprovalPolicy::Ask).unwrap();
    assert_eq!(
        h.service().override_of(next.session()).unwrap(),
        Some(ApprovalPolicy::Ask)
    );
    assert_eq!(
        checkpoint(&h, next.session())["approvalState"]["val"],
        json!({"policy":"ask","turnOpen":false})
    );
    h.close().await;
}
#[tokio::test]
async fn does_not_publish_a_handle_delivered_after_its_owning_service_is_disposed() {
    let h = Harness::context().await;
    h.load::<SystemPrompt>(json!({})).await;
    let (fiber, release) = delayed(&h).await;
    let a = h.agent(&[]);
    set_approval_policy(a.session(), ApprovalPolicy::Never).unwrap();
    let row = checkpoint(&h, a.session())["approvalState"].clone();
    let unloading = fiber.dispose();
    let _ = release.send(());
    unloading.await;
    a.session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    assert_eq!(checkpoint(&h, a.session())["approvalState"], row);
    let next = h.agent(&[]);
    assert!(
        checkpoint(&h, next.session())
            .get("approvalState")
            .is_none()
    );
    assert!(h.ctx.get::<Approval>().is_none());
    assert!(context_text(&h, Some(a)).await.is_none());
    assert!(
        !h.ctx
            .logger()
            .buffer()
            .iter()
            .any(|r| r.level == Level::Error)
    );
    h.close().await;
}

#[test]
fn stored_state_matches_zod_nullable_required_fields_and_strips_unknown_keys() {
    for policy in [json!(null), json!("ask"), json!("never")] {
        let state = state::parse(Some(
            &json!({"policy":policy,"turnOpen":true,"extra":"stripped"}),
        ))
        .unwrap();
        assert_eq!(
            serde_json::to_value(state.as_ref()).unwrap(),
            json!({"policy":policy,"turnOpen":true})
        );
    }
    for value in [
        json!(null),
        json!([]),
        json!({}),
        json!({"turnOpen":false}),
        json!({"policy":null}),
        json!({"policy":"sometimes","turnOpen":false}),
        json!({"policy":null,"turnOpen":null}),
        json!({"policy":null,"turnOpen":0}),
    ] {
        assert!(state::parse(Some(&value)).is_err(), "{value}");
    }
    assert!(state::parse(None).is_err());
}
#[tokio::test]
async fn failed_decided_append_propagates_exact_cause_after_asked_commits() {
    use hoh_error::SharedError;
    use hoh_session::SessionEventEmitted;
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let failure = SharedError::new("decided append failed before growth");
    let exact = failure.clone();
    h.ctx
        .on_dispatch(move |_, _, payload| {
            if let Some(e) = payload.downcast_ref::<SessionEventEmitted>()
                && e.event.event_type() == "approval/decided"
            {
                e.validation.reject(exact.clone().into());
            }
        })
        .unwrap();
    let error = h.service().request(req(a.clone())).await.unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&failure));
    let a = audit(a.session());
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].event_type(), "approval/asked");
    h.close().await;
}
#[tokio::test]
async fn dropping_the_result_future_does_not_withdraw_a_started_request() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    answer(&h.ctx, ApprovalOutcome::AllowedOnce);
    let (done, wait) = oneshot::channel();
    let slot = Arc::new(Mutex::new(Some(done)));
    let slot2 = slot.clone();
    h.ctx
        .on_contained::<hoh_session::SessionEventEmitted>(move |e| {
            if e.event.event_type() == "approval/decided" {
                let _ = lock(&slot2).take().unwrap().send(());
            }
            Ok(())
        })
        .unwrap();
    drop(h.service().request(req(a.clone())));
    wait.await.unwrap();
    assert_pair(a.session(), ApprovalOutcome::AllowedOnce);
    h.close().await;
}

#[tokio::test]
async fn explicit_null_policy_defaults_to_ask_like_schemastery() {
    let h = Harness::context().await;
    let owner = h.load::<ApprovalService>(json!({"policy":null})).await;
    assert_eq!(owner.effective_config().unwrap()["policy"], json!("ask"));
    assert_eq!(h.service().config().policy, ApprovalPolicy::Ask);
    answer(&h.ctx, ApprovalOutcome::AllowedOnce);
    assert_eq!(
        h.service().request(req(h.open_agent())).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    let invalid = h
        .ctx
        .plugin::<ApprovalService>(json!({"policy":"sometimes"}))
        .unwrap();
    assert!(invalid.settled().await.is_err());
    h.close().await;
}
