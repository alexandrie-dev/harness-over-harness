mod support;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use hoh_session::{Session, SessionEventEmitted};
use hoh_session_title::{
    SessionTitle, SessionTitleAutomaticMode, SessionTitleModelIdentity, SessionTitleProviderResult,
    SessionTitleSource,
};
use hoh_timeout::AbortController;

use support::*;

fn request_seqs(
    request: &hoh_session_title::SessionTitleProviderRequest,
) -> Vec<hoh_session::SessionSeq> {
    request.messages.iter().map(|message| message.seq).collect()
}

#[tokio::test]
async fn configuration_requires_positive_integral_compatible_limits() {
    for invalid in [
        serde_json::json!({"fallbackMaxWords":0,"fallbackMaxBytes":40,"maxTitleBytes":80}),
        serde_json::json!({"fallbackMaxWords":1.5,"fallbackMaxBytes":40,"maxTitleBytes":80}),
        serde_json::json!({"fallbackMaxWords":5,"fallbackMaxBytes":81,"maxTitleBytes":80}),
    ] {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<hoh_session::SessionPlugin>(serde_json::json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_session_projection::SessionProjectionRegistry>(serde_json::json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_agent::AgentRegistryPlugin>(serde_json::json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let error = ctx
            .plugin::<hoh_session_title::SessionTitlePlugin>(invalid)
            .unwrap()
            .settled()
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("positive integer")
                || error.to_string().contains("must not exceed")
        );
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn empty_input_and_detached_or_preaborted_refresh_boundaries() {
    let env = setup().await;
    let empty = session(&env, "empty");
    assert!(env.titles.refresh(&empty, None).await.unwrap().is_none());
    let detached = Session::create("detached".into(), None, None, None, None).unwrap();
    assert!(
        env.titles
            .refresh(&detached, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not live")
    );
    let controller = AbortController::new();
    controller.abort_with("already cancelled");
    assert!(
        env.titles
            .refresh(&empty, Some(controller.signal()))
            .await
            .unwrap_err()
            .to_string()
            .contains("already cancelled")
    );
    env.shutdown().await;
}

#[tokio::test]
async fn explicit_generation_receives_absent_route_and_live_caller_signal() {
    let env = setup().await;
    let observed = Arc::new(Mutex::new(None));
    let seen = observed.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "explicit",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    *seen.lock().unwrap() = Some((request.route.clone(), request.signal.aborted()));
                    async move { Ok(result("Explicit", request_seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "explicit");
    append_human(&session, "Prompt before route");
    settle().await;
    env.titles
        .refresh(&session, Some(AbortController::new().signal()))
        .await
        .unwrap();
    assert_eq!(*observed.lock().unwrap(), Some((None, false)));
    env.shutdown().await;
}

#[tokio::test]
async fn scheduling_reads_only_seed_and_origin_from_lossless_header() {
    let env = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "lossless-header",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(result("Scheduled", request_seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("lossless-header".into()),
            hoh_session::CreateSessionOptions {
                // hoh-session accepts the integral JSON number losslessly, but
                // deserializing the whole header into a u64-backed Rust DTO does not.
                meta: Some(serde_json::json!({"delegationDepth": 1.0})),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        serde_json::from_value::<hoh_session::SessionHeader>((*session.header()).clone()).is_err()
    );
    append_human(&session, "Schedule from raw header fields");
    settle().await;
    append_route(&session, "initial");
    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(env.titles.get(&session).unwrap().title, "Scheduled");
    env.shutdown().await;
}

#[tokio::test]
async fn caller_cancellation_reaches_active_provider_work() {
    let env = setup().await;
    let aborted = Arc::new(AtomicBool::new(false));
    let seen = aborted.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "cancel",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    let seen = seen.clone();
                    async move {
                        request.signal.cancelled().await;
                        seen.store(true, Ordering::SeqCst);
                        Ok(result("ignored", request_seqs(&request)))
                    }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "cancel");
    append_human(&session, "Cancel me");
    settle().await;
    let controller = AbortController::new();
    let titles = env.titles.clone();
    let refresh_session = session.clone();
    let signal = controller.signal();
    let refresh = tokio::spawn(async move { titles.refresh(&refresh_session, Some(signal)).await });
    settle().await;
    controller.abort_with("caller cancelled");
    assert!(
        refresh
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("caller cancelled")
    );
    assert!(aborted.load(Ordering::SeqCst));
    env.shutdown().await;

    let env = setup().await;
    let disposed_signal = Arc::new(AtomicBool::new(false));
    let seen = disposed_signal.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "dispose",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    let seen = seen.clone();
                    async move {
                        request.signal.cancelled().await;
                        seen.store(true, Ordering::SeqCst);
                        Ok(result("ignored", request_seqs(&request)))
                    }
                },
            ),
        )
        .unwrap();
    let disposed = env
        .sessions
        .prepare(Some("disposed".into()), Default::default())
        .unwrap();
    let detach = env.sessions.enter(&env.ctx, disposed.clone()).unwrap();
    env.sessions.announce(&disposed).unwrap();
    append_human(&disposed, "Dispose me");
    settle().await;
    let titles = env.titles.clone();
    let refresh_session = disposed.clone();
    let refresh = tokio::spawn(async move { titles.refresh(&refresh_session, None).await });
    settle().await;
    detach.detach();
    assert!(
        refresh
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("disposed")
    );
    assert!(disposed_signal.load(Ordering::SeqCst));
    env.shutdown().await;
}

#[tokio::test]
async fn dropping_refresh_clears_active_work_without_later_spurious_abort() {
    let env = setup().await;
    let started = Arc::new(tokio::sync::Notify::new());
    let provider_started = started.clone();
    let signal = Arc::new(Mutex::new(None));
    let observed = signal.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "drop-safe",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    let started = provider_started.clone();
                    let observed = observed.clone();
                    async move {
                        *observed.lock().unwrap() = Some(request.signal.clone());
                        started.notify_one();
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "drop-safe");
    append_human(&session, "Drop refresh future");
    settle().await;
    let titles = env.titles.clone();
    let target = session.clone();
    let refresh = tokio::spawn(async move { titles.refresh(&target, None).await });
    started.notified().await;
    refresh.abort();
    assert!(refresh.await.unwrap_err().is_cancelled());
    env.titles.rename(&session, "User title").unwrap();
    assert!(!signal.lock().unwrap().as_ref().unwrap().aborted());
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refresh_racing_session_disposal_never_panics() {
    let env = setup().await;
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "dispose-race",
                SessionTitleAutomaticMode::FirstPrompt,
                |request| async move { Ok(result("Title", request_seqs(&request))) },
            ),
        )
        .unwrap();
    for index in 0..256 {
        let session = env
            .sessions
            .prepare(
                Some(format!("dispose-race-{index}").into()),
                Default::default(),
            )
            .unwrap();
        let detach = env.sessions.enter(&env.ctx, session.clone()).unwrap();
        env.sessions.announce(&session).unwrap();
        append_human(&session, "Dispose concurrently");
        settle().await;
        let gate = Arc::new(tokio::sync::Barrier::new(3));
        let titles = env.titles.clone();
        let target = session.clone();
        let refresh_gate = gate.clone();
        let refresh = tokio::spawn(async move {
            refresh_gate.wait().await;
            titles.refresh(&target, None).await
        });
        let detach_gate = gate.clone();
        let disposal = tokio::spawn(async move {
            detach_gate.wait().await;
            detach.detach();
        });
        gate.wait().await;
        let outcome = refresh.await;
        assert!(outcome.is_ok(), "refresh panicked at iteration {index}");
        let _ = outcome.unwrap();
        disposal.await.unwrap();
    }
    env.shutdown().await;
}

#[tokio::test]
async fn concurrent_refreshes_share_one_fallback() {
    let env = setup().await;
    let seed = Session::create("fallback-seed".into(), None, None, None, None).unwrap();
    seed.append("turn/start", serde_json::json!({"turn":1}), None)
        .unwrap();
    let source = append_human(&seed, "Exactly one fallback");
    seed.append(
        "turn/end",
        serde_json::json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("fallback-concurrency".into()),
            hoh_session::CreateSessionOptions {
                seed: Some(seed.snapshot_events(None, None).to_vec()),
                ..Default::default()
            },
        )
        .unwrap();
    let (left, right) = tokio::join!(
        env.titles.refresh(&session, None),
        env.titles.refresh(&session, None)
    );
    assert_eq!(left.unwrap(), right.unwrap());
    assert_eq!(
        session
            .snapshot_events(None, None)
            .iter()
            .filter(|event| event.event_type() == "session/title")
            .count(),
        1
    );
    assert_eq!(env.titles.get(&session).unwrap().message_seqs, vec![source]);
    env.shutdown().await;
}

#[tokio::test]
async fn queued_fallback_reuses_an_already_accepted_title() {
    let env = setup().await;
    let session = session(&env, "fallback-race");
    let seq = append_human(&session, "Fallback race");
    let titles = env.titles.clone();
    let refresh_session = session.clone();
    let refresh = tokio::spawn(async move { titles.refresh(&refresh_session, None).await });
    session.append("session/title", serde_json::json!({"title":"Already accepted","messageSeqs":[seq],"source":{"kind":"fallback"}}), None).unwrap();
    assert_eq!(
        refresh.await.unwrap().unwrap().unwrap().title,
        "Already accepted"
    );
    assert_eq!(
        session
            .snapshot_events(None, None)
            .iter()
            .filter(|event| event.event_type() == "session/title")
            .count(),
        1
    );
    env.shutdown().await;
}

#[tokio::test]
async fn newest_overlapping_explicit_refresh_wins() {
    let env = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let first_started = Arc::new(tokio::sync::Notify::new());
    let started = first_started.clone();
    let first_release = Arc::new(tokio::sync::Notify::new());
    let release = first_release.clone();
    let first_signal = Arc::new(Mutex::new(None));
    let signal = first_signal.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "newest",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    let call = seen.fetch_add(1, Ordering::SeqCst);
                    let started = started.clone();
                    let release = release.clone();
                    let signal = signal.clone();
                    async move {
                        if call == 0 {
                            *signal.lock().unwrap() = Some(request.signal.clone());
                            started.notify_one();
                            release.notified().await;
                            Ok(result("Obsolete title", request_seqs(&request)))
                        } else {
                            Ok(result("Newest explicit title", request_seqs(&request)))
                        }
                    }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "newest");
    append_human(&session, "Newest wins");
    settle().await;
    let titles = env.titles.clone();
    let older_session = session.clone();
    let older = tokio::spawn(async move { titles.refresh(&older_session, None).await });
    first_started.notified().await;
    let titles = env.titles.clone();
    let newer_session = session.clone();
    let newer = tokio::spawn(async move { titles.refresh(&newer_session, None).await });
    settle().await;
    assert!(first_signal.lock().unwrap().as_ref().unwrap().aborted());
    assert_eq!(
        newer.await.unwrap().unwrap().unwrap().title,
        "Newest explicit title"
    );
    first_release.notify_one();
    assert!(older.await.unwrap().is_err());
    assert_eq!(
        env.titles.get(&session).unwrap().title,
        "Newest explicit title"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn inactive_service_rejects_refresh_after_shutdown_begins() {
    let env = setup_base().await;
    let fiber = env.mount_titles(config()).await;
    let service = (*env.ctx.get::<SessionTitle>().unwrap()).clone();
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("inactive".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    let disposal = tokio::spawn(async move { fiber.dispose().await });
    tokio::task::yield_now().await;
    append_human(&session, "Ignore reentrant disposal prompt");
    append_route(&session, "initial");
    assert!(
        service
            .refresh(&session, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("disposed")
    );
    disposal.await.unwrap();
    assert!(service.get(&session).is_none());
    env.shutdown().await;
}

#[tokio::test]
async fn service_shutdown_suppresses_queued_fallback_publication() {
    let env = setup_base().await;
    let fiber = env.mount_titles(config()).await;
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let captured = warnings.clone();
    let _exporter = env
        .ctx
        .logger()
        .exporter(Default::default(), move |record| {
            if record.level == hoh_kernel::Level::Warn {
                captured.lock().unwrap().push(record.message.clone());
            }
        })
        .unwrap();
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("shutdown-fallback".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    let disposal = tokio::spawn(async move { fiber.dispose().await });
    tokio::task::yield_now().await;
    append_human(&session, "Do not publish after service disposal");
    disposal.await.unwrap();
    settle().await;
    assert!(
        session
            .snapshot_events(None, None)
            .iter()
            .all(|event| event.event_type() != "session/title")
    );
    assert!(warnings.lock().unwrap().is_empty());
    env.shutdown().await;
}

#[tokio::test]
async fn service_shutdown_aborts_provider_lifetime_signal() {
    let env = setup_base().await;
    let fiber = env.mount_titles(config()).await;
    let titles = (*env.ctx.get::<SessionTitle>().unwrap()).clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let provider_started = started.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let provider_release = release.clone();
    let signal = Arc::new(Mutex::new(None));
    let observed = signal.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen_calls = calls.clone();
    let old_registration = titles
        .register(
            &env.ctx,
            TestProvider::new(
                "shutdown",
                SessionTitleAutomaticMode::AllPrompts,
                move |request| {
                    seen_calls.fetch_add(1, Ordering::SeqCst);
                    let started = provider_started.clone();
                    let release = provider_release.clone();
                    let observed = observed.clone();
                    async move {
                        *observed.lock().unwrap() = Some(request.signal.clone());
                        started.notify_one();
                        release.notified().await;
                        Ok(result("Ignored service abort", request_seqs(&request)))
                    }
                },
            ),
        )
        .unwrap();
    let active = env
        .sessions
        .create(
            &env.ctx,
            Some("shutdown-active".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    append_human(&active, "Active work");
    settle().await;
    let service = titles.clone();
    let refresh_session = active.clone();
    let task = tokio::spawn(async move { service.refresh(&refresh_session, None).await });
    started.notified().await;
    let old_disposal = tokio::spawn(async move { old_registration.dispose().await });
    settle().await;
    assert!(signal.lock().unwrap().as_ref().unwrap().aborted());
    titles
        .register(
            &env.ctx,
            TestProvider::new(
                "shutdown-replacement",
                SessionTitleAutomaticMode::AllPrompts,
                |request| async move {
                    Ok(result(
                        "Replacement should stay pending",
                        request_seqs(&request),
                    ))
                },
            ),
        )
        .unwrap();
    let pending = env
        .sessions
        .create(
            &env.ctx,
            Some("shutdown-pending".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    append_human(&pending, "Pending work");
    settle().await;
    let disposed = Arc::new(AtomicBool::new(false));
    let completed = disposed.clone();
    let disposal = tokio::spawn(async move {
        fiber.dispose().await;
        completed.store(true, Ordering::SeqCst);
    });
    settle().await;
    assert!(signal.lock().unwrap().as_ref().unwrap().aborted());
    assert!(!disposed.load(Ordering::SeqCst));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.notify_one();
    disposal.await.unwrap();
    old_disposal.await.unwrap();
    assert!(disposed.load(Ordering::SeqCst));
    assert!(task.await.unwrap().is_err());
    assert!(
        titles
            .get(&active)
            .is_some_and(|title| !matches!(title.source, SessionTitleSource::Provider { .. }))
    );
    assert!(
        titles
            .get(&pending)
            .is_some_and(|title| matches!(title.source, SessionTitleSource::Fallback))
    );
    env.shutdown().await;
}

#[tokio::test]
async fn detached_session_cannot_publish_fallback() {
    let env = setup_base().await;
    let detached = env
        .sessions
        .prepare(Some("detached-fallback".into()), Default::default())
        .unwrap();
    let detach = env.sessions.enter(&env.ctx, detached.clone()).unwrap();
    env.sessions.announce(&detached).unwrap();
    let target = detached.clone();
    env.ctx
        .on_contained::<SessionEventEmitted>(move |event| {
            if event.session == target && event.event.event_type() == "user/message" {
                detach.detach();
            }
            Ok(())
        })
        .unwrap();
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let captured = warnings.clone();
    let _exporter = env
        .ctx
        .logger()
        .exporter(Default::default(), move |record| {
            if record.level == hoh_kernel::Level::Warn {
                captured.lock().unwrap().push(record.message.clone());
            }
        })
        .unwrap();
    env.mount_titles(config()).await;
    append_human(&detached, "Detach before fallback publication");
    settle().await;
    assert!(
        warnings
            .lock()
            .unwrap()
            .iter()
            .any(|message| message.contains("fallback title update failed"))
    );
    assert!(hoh_session_title::fold_session_title(&detached.snapshot_events(None, None)).is_none());
    env.shutdown().await;
}

#[tokio::test]
async fn byte_cap_smaller_than_first_code_point_leaves_title_absent() {
    let env = setup_with(
        serde_json::json!({"fallbackMaxWords":5,"fallbackMaxBytes":1,"maxTitleBytes":2}),
    )
    .await;
    let session = session(&env, "no-code-point");
    append_human(&session, "😀");
    settle().await;
    assert!(env.titles.get(&session).is_none());
    assert!(env.titles.refresh(&session, None).await.unwrap().is_none());
    env.shutdown().await;
}

#[tokio::test]
async fn malformed_provider_registration_is_rejected_before_publication() {
    let env = setup().await;
    let invalid = env.titles.register(
        &env.ctx,
        TestProvider::new(
            "",
            SessionTitleAutomaticMode::FirstPrompt,
            |request| async move { Ok(result("unused", request_seqs(&request))) },
        ),
    );
    assert!(invalid.err().unwrap().to_string().contains("non-empty"));
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "valid",
                SessionTitleAutomaticMode::FirstPrompt,
                |request| async move { Ok(result("valid", request_seqs(&request))) },
            ),
        )
        .unwrap();
    env.shutdown().await;
}

#[tokio::test]
async fn disposed_provider_drops_queued_automatic_work() {
    let env = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let registration = env
        .titles
        .register(
            &env.ctx,
            TestProvider::new(
                "queued",
                SessionTitleAutomaticMode::AllPrompts,
                move |request| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(result("too late", request_seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "queued");
    append_human(&session, "Queue work");
    settle().await;
    registration.dispose().await;
    append_route(&session, "initial");
    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    env.shutdown().await;
}

#[tokio::test]
async fn disposal_started_after_route_prevents_deferred_provider_entry() {
    let env = setup().await;
    for iteration in 0..32 {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let registration = env
            .titles
            .register(
                &env.ctx,
                TestProvider::new(
                    "deferred-close",
                    SessionTitleAutomaticMode::AllPrompts,
                    move |request| {
                        seen.fetch_add(1, Ordering::SeqCst);
                        async move { Ok(result("too late", request_seqs(&request))) }
                    },
                ),
            )
            .unwrap();
        let session = session(&env, &format!("deferred-close-{iteration}"));
        append_human(&session, "Queue automatic work");
        settle().await;
        append_route(&session, "initial");
        registration.dispose().await;
        settle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "closed provider entered at iteration {iteration}"
        );
    }
    env.shutdown().await;
}

#[tokio::test]
async fn malformed_provider_results_do_not_replace_fallback() {
    let env = setup().await;
    let mode = Arc::new(AtomicUsize::new(0));
    let selected = mode.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "invalid",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    let case = selected.fetch_add(1, Ordering::SeqCst);
                    async move {
                        Ok(match case {
                            0 => SessionTitleProviderResult {
                                title: "\u{1b}[31m".into(),
                                message_seqs: request_seqs(&request),
                                model: None,
                            },
                            1 => SessionTitleProviderResult {
                                title: "valid".into(),
                                message_seqs: vec![],
                                model: None,
                            },
                            2 => SessionTitleProviderResult {
                                title: "valid".into(),
                                message_seqs: vec![hoh_session::SessionSeq::new(999).unwrap()],
                                model: None,
                            },
                            _ => SessionTitleProviderResult {
                                title: "valid".into(),
                                message_seqs: request_seqs(&request),
                                model: Some(SessionTitleModelIdentity {
                                    provider: "".into(),
                                    model: "m".into(),
                                }),
                            },
                        })
                    }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "invalid-results");
    append_human(&session, "Fallback source");
    settle().await;
    for expected in [
        "session-title provider returned an empty title",
        "session-title provider must identify at least one source message seq",
        "session-title provider messageSeqs must be unique, ordered seqs from the request",
        "session-title provider result model must contain non-empty provider and model strings",
    ] {
        assert_eq!(
            env.titles
                .refresh(&session, None)
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
        assert!(matches!(
            env.titles.get(&session).unwrap().source,
            SessionTitleSource::Fallback
        ));
    }
    env.shutdown().await;
}

#[tokio::test]
async fn stale_service_rejects_registration_after_unload() {
    let env = setup_base().await;
    let fiber = env.mount_titles(config()).await;
    let titles = (*env.ctx.get::<SessionTitle>().unwrap()).clone();
    fiber.dispose().await;
    assert_eq!(
        titles
            .register(
                &env.ctx,
                TestProvider::new(
                    "stale",
                    SessionTitleAutomaticMode::FirstPrompt,
                    |request| async move { Ok(result("unused", request_seqs(&request))) },
                ),
            )
            .err()
            .unwrap()
            .to_string(),
        "session-title service disposed"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn provider_result_validation_rejects_all_representable_order_and_model_failures() {
    let env = setup().await;
    let mode = Arc::new(AtomicUsize::new(0));
    let selected = mode.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "validation",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    let case = selected.fetch_add(1, Ordering::SeqCst);
                    async move {
                        let first = request.messages[0].seq;
                        let second = request.messages[1].seq;
                        Ok(match case {
                            0 => SessionTitleProviderResult {
                                title: "valid".into(),
                                message_seqs: vec![first, first],
                                model: None,
                            },
                            1 => SessionTitleProviderResult {
                                title: "valid".into(),
                                message_seqs: vec![second, first],
                                model: None,
                            },
                            _ => SessionTitleProviderResult {
                                title: "valid".into(),
                                message_seqs: vec![first, second],
                                model: Some(SessionTitleModelIdentity {
                                    provider: "provider".into(),
                                    model: "".into(),
                                }),
                            },
                        })
                    }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "validation");
    append_human(&session, "First");
    append_human(&session, "Second");
    settle().await;
    for expected in [
        "messageSeqs must be unique, ordered seqs from the request",
        "messageSeqs must be unique, ordered seqs from the request",
        "result model must contain non-empty provider and model strings",
    ] {
        assert!(
            env.titles
                .refresh(&session, None)
                .await
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
    }
    env.shutdown().await;
}

#[tokio::test]
async fn concurrent_registration_publishes_exactly_one_provider() {
    let env = setup().await;
    for iteration in 0..128 {
        let gate = Arc::new(std::sync::Barrier::new(3));
        let left_gate = gate.clone();
        let left_titles = env.titles.clone();
        let left_ctx = env.ctx.clone();
        let left = std::thread::spawn(move || {
            left_gate.wait();
            left_titles.register(
                &left_ctx,
                TestProvider::new(
                    "left",
                    SessionTitleAutomaticMode::FirstPrompt,
                    |request| async move { Ok(result("left", request_seqs(&request))) },
                ),
            )
        });
        let right_gate = gate.clone();
        let right_titles = env.titles.clone();
        let right_ctx = env.ctx.clone();
        let right = std::thread::spawn(move || {
            right_gate.wait();
            right_titles.register(
                &right_ctx,
                TestProvider::new(
                    "right",
                    SessionTitleAutomaticMode::FirstPrompt,
                    |request| async move { Ok(result("right", request_seqs(&request))) },
                ),
            )
        });
        gate.wait();
        let outcomes = [left.join().unwrap(), right.join().unwrap()];
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
            1,
            "registration race admitted two providers at iteration {iteration}"
        );
        for disposer in outcomes.into_iter().flatten() {
            disposer.dispose().await;
        }
    }
    env.shutdown().await;
}
