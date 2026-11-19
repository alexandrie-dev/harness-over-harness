use crate::{test_support::*, *};
use hoh_error::SharedError;
use hoh_kernel::Level;
use hoh_scope::{CreateScopeOptions, carrier_key_of, create_scope};
use hoh_session::SessionEventEmitted;
use hoh_timeout::AbortController;
use serde_json::json;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::oneshot;

#[tokio::test]
async fn throws_before_appending_anything_when_no_turn_has_ever_opened_idle_ask() {
    let h = Harness::mounted().await;
    let a = h.agent(&[]);
    let error = h.service().request(req(a.clone())).await.unwrap_err();
    assert_eq!(error.to_string(), OUTSIDE_TURN);
    assert!(error.downcast_ref::<ApprovalError>().is_some());
    assert!(hoh_error::HarnessError::of(error.as_ref()).is_none());
    assert!(audit(a.session()).is_empty());
    h.close().await;
}
#[tokio::test]
async fn throws_between_turns_a_closed_turn_does_not_satisfy_the_enclosure_precondition() {
    let h = Harness::mounted().await;
    let a = h.agent(&["turn/start", "turn/end"]);
    assert_eq!(
        h.service()
            .request(req(a.clone()))
            .await
            .unwrap_err()
            .to_string(),
        OUTSIDE_TURN
    );
    assert!(audit(a.session()).is_empty());
    h.close().await;
}
#[tokio::test]
async fn fails_closed_to_unavailable_when_nobody_listens_auditing_the_asked_decided_pair() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let mut request = req(a.clone());
    request.call_id = Some("call-1".into());
    request.reason = Some("hook says ask".into());
    assert_eq!(
        h.service().request(request).await.unwrap(),
        ApprovalOutcome::Unavailable
    );
    assert_pair(a.session(), ApprovalOutcome::Unavailable);
    let data = audit(a.session())[0].data().clone();
    assert_eq!(
        data.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["id", "toolName", "callId", "reason"]
    );
    assert_eq!(data["toolName"], "echo");
    assert_eq!(data["callId"], "call-1");
    assert_eq!(data["reason"], "hook says ask");
    h.close().await;
}
#[tokio::test]
async fn omits_absent_optional_fields_from_the_asked_audit_event() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    h.service().request(req(a.clone())).await.unwrap();
    assert_eq!(
        audit(a.session())[0]
            .data()
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["id", "toolName"]
    );
    h.close().await;
}
#[tokio::test]
async fn borrows_the_exact_readonly_request_for_scoped_dispatch_and_audit() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let scoped = create_scope(&h.ctx, a.scope_key(), CreateScopeOptions::default()).unwrap();
    let seen = Arc::new(Mutex::new(None));
    let capture = seen.clone();
    scoped
        .ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |r, _| {
            *lock(&capture) = Some(r);
            async { Ok(ApprovalOutcome::AllowedOnce.into()) }
        })
        .unwrap();
    let mut r = req(a.clone());
    r.tool_name = "scoped-tool".into();
    r.call_id = Some("scoped-call".into());
    r.reason = Some("scoped reason".into());
    r.display_reason = Some(DisplayReason::new("presentation"));
    let r = Arc::new(r);
    assert_eq!(
        h.service().request(r.clone()).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    assert!(Arc::ptr_eq(lock(&seen).as_ref().unwrap(), &r));
    assert_eq!(
        carrier_key_of(&agent_carrier(&r.agent)),
        Some(a.scope_key())
    );
    assert_pair(a.session(), ApprovalOutcome::AllowedOnce);
    let d = audit(a.session())[0].data().clone();
    assert_eq!(d["toolName"], "scoped-tool");
    assert_eq!(d["callId"], "scoped-call");
    assert_eq!(d["reason"], "scoped reason");
    assert!(d.get("displayReason").is_none());
    scoped.dispose().await;
    h.close().await;
}
async fn throwing_observer(event_type: &'static str, outcome: ApprovalOutcome) {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    h.ctx
        .on_contained::<SessionEventEmitted>(move |e| {
            if e.event.event_type() == event_type {
                return Err(format!(
                    "observer failed after {} append",
                    if event_type == "approval/asked" {
                        "asked"
                    } else {
                        "decided"
                    }
                )
                .into());
            }
            Ok(())
        })
        .unwrap();
    answer(&h.ctx, outcome);
    assert_eq!(h.service().request(req(a.clone())).await.unwrap(), outcome);
    assert_pair(a.session(), outcome);
    let expected = format!(
        "session/event listener threw: Error: observer failed after {} append",
        if event_type == "approval/asked" {
            "asked"
        } else {
            "decided"
        }
    );
    assert!(
        h.ctx
            .logger()
            .buffer()
            .iter()
            .any(|r| r.level == Level::Warn && r.message.contains(&expected))
    );
    h.close().await;
}
#[tokio::test]
async fn contains_an_approval_asked_observer_throw_after_append_and_still_completes_the_pair() {
    throwing_observer("approval/asked", ApprovalOutcome::AllowedOnce).await;
}
#[tokio::test]
async fn contains_an_approval_decided_observer_throw_after_append_and_still_resolves() {
    throwing_observer("approval/decided", ApprovalOutcome::Rejected).await;
}
#[tokio::test]
async fn propagates_an_append_failure_that_prevented_audit_log_growth() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let failure = SharedError::new("append failed before log growth");
    let f = failure.clone();
    h.ctx
        .on_dispatch(move |_, _, payload| {
            if let Some(e) = payload.downcast_ref::<SessionEventEmitted>()
                && e.event.event_type() == "approval/asked"
            {
                e.validation.reject(f.clone().into());
            }
        })
        .unwrap();
    let before = a.session().seq();
    let err = h.service().request(req(a.clone())).await.unwrap_err();
    assert!(SharedError::of(err.as_ref()).unwrap().ptr_eq(&failure));
    assert_eq!(a.session().seq(), before);
    h.close().await;
}
#[tokio::test]
async fn returns_the_first_answering_listener_outcome_single_decision_slot() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let ran = Arc::new(AtomicBool::new(false));
    answer(&h.ctx, ApprovalOutcome::AllowedOnce);
    let r = ran.clone();
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, _| {
            r.store(true, Ordering::SeqCst);
            async { Ok(ApprovalOutcome::Rejected.into()) }
        })
        .unwrap();
    assert_eq!(
        h.service().request(req(a)).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    assert!(!ran.load(Ordering::SeqCst));
    h.close().await;
}
#[tokio::test]
async fn lets_a_non_owning_listener_delegate_via_next_down_to_the_fail_closed_default() {
    let h = Harness::mounted().await;
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(|_, next| next.run())
        .unwrap();
    assert_eq!(
        h.service().request(req(h.open_agent())).await.unwrap(),
        ApprovalOutcome::Unavailable
    );
    h.close().await;
}
#[tokio::test]
async fn dispatches_to_global_and_matching_agent_scoped_listeners_never_a_foreign_scope() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let b = h.open_agent();
    let heard = Arc::new(Mutex::new(vec![]));
    let ca = create_scope(&h.ctx, a.scope_key(), Default::default()).unwrap();
    let cb = create_scope(&h.ctx, b.scope_key(), Default::default()).unwrap();
    let ga: Arc<dyn Agent> = a.clone();
    let seen = heard.clone();
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |r, next| {
            lock(&seen).push(if Arc::ptr_eq(&r.agent, &ga) {
                "global:A"
            } else {
                "global:B"
            });
            next.run()
        })
        .unwrap();
    for (scope, label) in [(&ca, "scoped:A"), (&cb, "scoped:B")] {
        let seen = heard.clone();
        scope
            .ctx
            .on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, next| {
                lock(&seen).push(label);
                next.run()
            })
            .unwrap();
    }
    assert_eq!(
        h.service().request(req(a)).await.unwrap(),
        ApprovalOutcome::Unavailable
    );
    assert_eq!(
        h.service().request(req(b)).await.unwrap(),
        ApprovalOutcome::Unavailable
    );
    assert_eq!(
        *lock(&heard),
        vec!["global:A", "scoped:A", "global:B", "scoped:B"]
    );
    ca.dispose().await;
    cb.dispose().await;
    h.close().await;
}
#[tokio::test]
async fn keys_the_scoped_dispatch_carrier_to_the_exact_request_agent() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let scope = create_scope(&h.ctx, a.scope_key(), Default::default()).unwrap();
    let seen = Arc::new(AtomicBool::new(false));
    let capture = seen.clone();
    let exact: Arc<dyn Agent> = a.clone();
    scope
        .ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |r, next| {
            assert!(Arc::ptr_eq(&r.agent, &exact));
            assert_eq!(
                carrier_key_of(&agent_carrier(&r.agent)),
                Some(exact.scope_key())
            );
            capture.store(true, Ordering::SeqCst);
            next.run()
        })
        .unwrap();
    assert_eq!(
        h.service().request(req(a)).await.unwrap(),
        ApprovalOutcome::Unavailable
    );
    assert!(seen.load(Ordering::SeqCst));
    scope.dispose().await;
    h.close().await;
}
#[tokio::test]
async fn contains_a_throwing_answerer_as_unavailable() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(|_, _| async {
            Err("transport died".into())
        })
        .unwrap();
    assert_eq!(
        h.service().request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::Unavailable
    );
    assert_pair(a.session(), ApprovalOutcome::Unavailable);
    h.close().await;
}
#[tokio::test]
async fn normalizes_a_rogue_non_vocabulary_answer_to_unavailable() {
    let h = Harness::mounted().await;
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(|_, _| async { Ok(json!("yolo")) })
        .unwrap();
    assert_eq!(
        h.service().request(req(h.open_agent())).await.unwrap(),
        ApprovalOutcome::Unavailable
    );
    h.close().await;
}
#[tokio::test]
async fn settles_cancelled_immediately_on_an_already_aborted_signal_without_asking_anyone() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let asked = Arc::new(AtomicBool::new(false));
    let seen = asked.clone();
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, _| {
            seen.store(true, Ordering::SeqCst);
            async { Ok(ApprovalOutcome::AllowedOnce.into()) }
        })
        .unwrap();
    let c = AbortController::new();
    c.abort();
    let mut r = req(a.clone());
    r.signal = Some(c.signal());
    assert_eq!(
        h.service().request(r).await.unwrap(),
        ApprovalOutcome::Cancelled
    );
    assert!(!asked.load(Ordering::SeqCst));
    assert_pair(a.session(), ApprovalOutcome::Cancelled);
    h.close().await;
}
async fn late_answer(reject: bool, delegates: usize) {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    for _ in 0..delegates {
        h.ctx
            .on_waterfall::<ApprovalRequestEvent, Value, _>(|_, next| next.run())
            .unwrap();
    }
    let (send, recv) = oneshot::channel::<Result<Value, BoxError>>();
    let slot = Arc::new(Mutex::new(Some(recv)));
    let (entered, ready) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let sequence = Arc::new(Mutex::new(Vec::new()));
    let observed = sequence.clone();
    h.ctx
        .on_contained::<SessionEventEmitted>(move |e| {
            match e.event.event_type() {
                "approval/asked" => lock(&observed).push("asked"),
                "approval/decided" => lock(&observed).push("decided(cancelled)"),
                _ => {}
            }
            Ok(())
        })
        .unwrap();
    let observed = sequence.clone();
    let (finished, done) = oneshot::channel();
    let finished = Arc::new(Mutex::new(Some(finished)));
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, _| {
            let recv = lock(&slot).take().unwrap();
            let entered = lock(&entered).take().unwrap();
            let finished = lock(&finished).take().unwrap();
            lock(&observed).push("handler");
            async move {
                entered.send(()).unwrap();
                let result = recv.await.unwrap();
                let _ = finished.send(());
                result
            }
        })
        .unwrap();
    let c = AbortController::new();
    let mut r = req(a.clone());
    r.signal = Some(c.signal());
    let pending = h.service().request(r);
    c.abort();
    assert_eq!(pending.await.unwrap(), ApprovalOutcome::Cancelled);
    ready.await.unwrap();
    let result = if reject {
        Err("answered too late".into())
    } else {
        Ok(ApprovalOutcome::AllowedOnce.into())
    };
    send.send(result).unwrap();
    done.await.unwrap();
    pump().await;
    assert_pair(a.session(), ApprovalOutcome::Cancelled);
    assert_eq!(
        *lock(&sequence),
        vec!["asked", "handler", "decided(cancelled)"]
    );
    h.close().await;
}
#[tokio::test]
async fn resolves_cancelled_when_the_signal_aborts_mid_question_and_discards_the_late_answer() {
    for delegates in [0, 1, 8] {
        late_answer(false, delegates).await;
    }
}
#[tokio::test]
async fn discards_a_late_rejection_after_abort_without_an_unhandled_rejection() {
    late_answer(true, 0).await;
}
#[tokio::test]
async fn resolves_the_answer_when_the_signal_never_aborts() {
    let h = Harness::mounted().await;
    answer(&h.ctx, ApprovalOutcome::Rejected);
    let c = AbortController::new();
    let mut r = req(h.open_agent());
    r.signal = Some(c.signal());
    assert_eq!(
        h.service().request(r).await.unwrap(),
        ApprovalOutcome::Rejected
    );
    h.close().await;
}
#[tokio::test]
async fn issues_a_fresh_id_per_request() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    for _ in 0..2 {
        h.service().request(req(a.clone())).await.unwrap();
    }
    let ev = audit(a.session());
    assert_eq!(ev.len(), 4);
    assert_ne!(ev[0].data()["id"], ev[2].data()["id"]);
    assert_eq!(ev[0].data()["id"].as_str().unwrap().len(), 36);
    h.close().await;
}
#[tokio::test]
async fn drops_a_disposed_plugin_listener_from_the_chain_hmr_safety() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let f = h
        .ctx
        .inject::<(), _>(|ctx, ()| async move {
            answer(&ctx, ApprovalOutcome::AllowedOnce);
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    assert_eq!(
        h.service().request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    f.dispose().await;
    assert_eq!(
        h.service().request(req(a)).await.unwrap(),
        ApprovalOutcome::Unavailable
    );
    h.close().await;
}
#[tokio::test]
async fn folds_to_the_last_event_or_undefined_without_one() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    assert_eq!(h.service().override_of(a.session()).unwrap(), None);
    set_approval_policy(a.session(), ApprovalPolicy::Never).unwrap();
    set_approval_policy(a.session(), ApprovalPolicy::Ask).unwrap();
    assert_eq!(
        h.service().override_of(a.session()).unwrap(),
        Some(ApprovalPolicy::Ask)
    );
    assert_eq!(
        events(a.session()).last().unwrap().data(),
        &json!({"policy":"ask"})
    );
    h.close().await;
}
#[tokio::test]
async fn rejects_a_policy_outside_the_closed_vocabulary_before_appending() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    let before = a.session().seq();
    let error = ApprovalPolicy::try_from("sometimes")
        .map(|policy| set_approval_policy(a.session(), policy).unwrap())
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "approval policy must be one of \"ask\" or \"never\""
    );
    assert_eq!(error.name(), "TypeError");
    assert_eq!(a.session().seq(), before);
    h.close().await;
}
#[tokio::test]
async fn defaults_an_omitted_policy_to_ask() {
    let h = Harness::mounted().await;
    let service = (*h.service()).clone();
    *service.config() = Config::default();
    answer(&h.ctx, ApprovalOutcome::AllowedOnce);
    assert_eq!(
        service.request(req(h.open_agent())).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    h.close().await;
}
#[tokio::test]
async fn context_and_requests_read_the_same_current_service_configuration() {
    let h = Harness::context().await;
    h.load::<SystemPrompt>(json!({})).await;
    h.load::<ApprovalService>(json!({})).await;
    let a = h.open_agent();
    answer(&h.ctx, ApprovalOutcome::AllowedOnce);
    assert_eq!(
        context_text(&h, Some(a.clone())).await.as_deref(),
        Some(ASK_SENTENCE)
    );
    let service = (*h.service()).clone();
    *service.config() = Config {
        policy: ApprovalPolicy::Never,
    };
    assert_eq!(
        service.request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::Rejected
    );
    assert_eq!(
        context_text(&h, Some(a.clone())).await.as_deref(),
        Some(NEVER_SENTENCE)
    );
    assert_eq!(
        h.service().request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::Rejected
    );
    service.config().policy = ApprovalPolicy::Ask;
    assert_eq!(
        context_text(&h, Some(a.clone())).await.as_deref(),
        Some(ASK_SENTENCE)
    );
    assert_eq!(
        h.service().request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    h.close().await;
}
#[tokio::test]
async fn contains_an_answerer_that_throws_synchronously_as_unavailable() {
    let h = Harness::mounted().await;
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(
            |_, _| -> std::future::Ready<Result<Value, BoxError>> { panic!("sync bug") },
        )
        .unwrap();
    assert_eq!(
        h.service().request(req(h.open_agent())).await.unwrap(),
        ApprovalOutcome::Unavailable
    );
    h.close().await;
}
#[tokio::test]
async fn a_never_config_rejects_deterministically_without_consulting_any_answerer() {
    let h = Harness::context().await;
    h.load::<ApprovalService>(json!({"policy":"never"})).await;
    let consulted = Arc::new(AtomicBool::new(false));
    let seen = consulted.clone();
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, next| {
            seen.store(true, Ordering::SeqCst);
            next.run()
        })
        .unwrap();
    let a = h.open_agent();
    assert_eq!(
        h.service().request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::Rejected
    );
    assert!(!consulted.load(Ordering::SeqCst));
    assert_pair(a.session(), ApprovalOutcome::Rejected);
    h.close().await;
}
#[tokio::test]
async fn the_gate_decides_first_even_against_an_answerer_registered_before_the_service_prepend() {
    let h = Harness::context().await;
    answer(&h.ctx, ApprovalOutcome::AllowedOnce);
    h.load::<ApprovalService>(json!({"policy":"never"})).await;
    assert_eq!(
        h.service().request(req(h.open_agent())).await.unwrap(),
        ApprovalOutcome::Rejected
    );
    h.close().await;
}
#[tokio::test]
async fn never_is_unbypassable_even_by_an_answerer_prepended_after_the_service_mounts() {
    let h = Harness::context().await;
    h.load::<ApprovalService>(json!({"policy":"never"})).await;
    let seen = Arc::new(AtomicBool::new(false));
    let seen2 = seen.clone();
    h.ctx
        .listen()
        .prepend()
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, _| {
            seen2.store(true, Ordering::SeqCst);
            async { Ok(ApprovalOutcome::AllowedOnce.into()) }
        })
        .unwrap();
    let a = h.open_agent();
    assert_eq!(
        h.service().request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::Rejected
    );
    assert!(!seen.load(Ordering::SeqCst));
    assert_pair(a.session(), ApprovalOutcome::Rejected);
    h.close().await;
}
#[tokio::test]
async fn a_session_override_outranks_the_configured_default_in_both_directions() {
    let h = Harness::context().await;
    h.load::<ApprovalService>(json!({"policy":"never"})).await;
    answer(&h.ctx, ApprovalOutcome::AllowedOnce);
    let a = h.open_agent();
    assert_eq!(h.service().override_of(a.session()).unwrap(), None);
    set_approval_policy(a.session(), ApprovalPolicy::Ask).unwrap();
    assert_eq!(
        h.service().override_of(a.session()).unwrap(),
        Some(ApprovalPolicy::Ask)
    );
    assert_eq!(
        h.service().request(req(a.clone())).await.unwrap(),
        ApprovalOutcome::AllowedOnce
    );
    set_approval_policy(a.session(), ApprovalPolicy::Never).unwrap();
    assert_eq!(
        h.service().request(req(a)).await.unwrap(),
        ApprovalOutcome::Rejected
    );
    h.close().await;
}
#[tokio::test]
async fn queues_a_live_policy_switch_for_the_next_model_step() {
    let h = Harness::mounted().await;
    let a = h.open_agent();
    h.service()
        .set_policy(a.as_ref(), ApprovalPolicy::Never)
        .unwrap();
    h.service()
        .set_policy(a.as_ref(), ApprovalPolicy::Never)
        .unwrap();
    assert_eq!(
        h.service().override_of(a.session()).unwrap(),
        Some(ApprovalPolicy::Never)
    );
    assert_eq!(lock(&a.injected).len(), 1);
    let m = serde_json::to_value(&lock(&a.injected)[0]).unwrap();
    assert_eq!(m["source"], json!({"kind":"user-approval"}));
    assert_eq!(
        m["content"],
        json!([{"type":"text","text":"The user changed the approval policy from \"ask the user for approval\" to \"never ask the user for approval\"."}])
    );
    h.service()
        .set_policy(a.as_ref(), ApprovalPolicy::Ask)
        .unwrap();
    let m = serde_json::to_value(&lock(&a.injected)[1]).unwrap();
    assert_eq!(
        m["content"][0]["text"],
        "The user changed the approval policy from \"never ask the user for approval\" to \"ask the user for approval\"."
    );
    h.close().await;
}
#[tokio::test]
async fn contributes_the_complete_current_ask_or_never_policy_as_cache_safe_context() {
    let h = Harness::context().await;
    h.load::<SystemPrompt>(json!({})).await;
    h.load::<ApprovalService>(json!({})).await;
    let a = h.open_agent();
    let b = h.open_agent();
    set_approval_policy(b.session(), ApprovalPolicy::Never).unwrap();
    assert_eq!(
        context_text(&h, Some(a)).await.as_deref(),
        Some(ASK_SENTENCE)
    );
    assert_eq!(
        context_text(&h, Some(b)).await.as_deref(),
        Some(NEVER_SENTENCE)
    );
    assert_eq!(context_text(&h, None).await.as_deref(), Some(""));
    h.close().await;
}
#[tokio::test]
async fn reflects_the_latest_durable_switch_in_cache_safe_context_and_stays_byte_stable_while_unchanged()
 {
    let h = Harness::context().await;
    h.load::<SystemPrompt>(json!({})).await;
    h.load::<ApprovalService>(json!({})).await;
    let a = h.open_agent();
    for _ in 0..2 {
        assert_eq!(
            context_text(&h, Some(a.clone())).await.as_deref(),
            Some(ASK_SENTENCE)
        );
    }
    for p in [
        ApprovalPolicy::Never,
        ApprovalPolicy::Ask,
        ApprovalPolicy::Never,
    ] {
        set_approval_policy(a.session(), p).unwrap();
    }
    for _ in 0..2 {
        assert_eq!(
            context_text(&h, Some(a.clone())).await.as_deref(),
            Some(NEVER_SENTENCE)
        );
    }
    h.close().await;
}
#[tokio::test]
async fn disposes_the_runtime_context_contribution_with_the_service() {
    let h = Harness::context().await;
    h.load::<SystemPrompt>(json!({})).await;
    let f = h.load::<ApprovalService>(json!({})).await;
    let a = h.open_agent();
    assert!(context_text(&h, Some(a.clone())).await.is_some());
    f.dispose().await;
    assert!(context_text(&h, Some(a)).await.is_none());
    h.close().await;
}

#[tokio::test]
async fn fused_dispatch_overwrites_a_foreign_agent_of_the_request_with_its_subject() {
    let h = Harness::mounted().await;
    let (subject, foreign) = (h.open_agent(), h.open_agent());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    h.ctx
        .on_waterfall::<ApprovalRequestEvent, Value, _>(move |r, _| {
            lock(&sink).push(r.agent.id().clone());
            async { Ok(Value::Null) }
        })
        .unwrap();
    let (subject, foreign): (Arc<dyn Agent>, Arc<dyn Agent>) = (subject, foreign);
    hoh_agent::agent_events(&h.ctx, &subject)
        .waterfall::<ApprovalRequestEvent, Value, _>(
            |_| req(foreign.clone()),
            |_| async { Ok(Value::Null) },
        )
        .await
        .unwrap();
    assert_eq!(*lock(&heard), [subject.id().clone()]);
    assert_ne!(subject.id(), foreign.id());
    h.close().await;
}
