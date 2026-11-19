mod support;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use hoh_session::SessionEventEmitted;
use hoh_session_title::{
    SessionTitle, SessionTitleAutomaticMode, SessionTitlePlugin, SessionTitleSource,
};

use support::*;

#[tokio::test]
async fn retains_first_message_while_title_registration_is_pending() {
    let env = setup_base().await;
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("pending-title".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    let first = append_human(&session, "First during activation");
    append_human(&session, "Second during activation");
    env.mount_titles(config()).await;
    let titles = env.ctx.get::<SessionTitle>().unwrap();
    let accepted = titles.refresh(&session, None).await.unwrap().unwrap();
    assert_eq!(accepted.title, "First during activation");
    assert_eq!(accepted.message_seqs, vec![first]);
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
async fn retains_first_message_while_title_input_registration_is_pending() {
    let env = setup_base().await;
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("pending-input".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    append_human(&session, "Input survives registration");
    append_human(&session, "Second input survives registration");
    env.mount_titles(config()).await;
    assert_eq!(
        env.projections.checkpoint(&session).unwrap()["titleInput"]
            .val
            .as_ref()
            .unwrap()["count"],
        2
    );
    env.shutdown().await;
}

#[tokio::test]
async fn uses_fallback_for_input_received_before_provider_registers() {
    let env = setup().await;
    let session = session(&env, "late-provider");
    append_human(&session, "Fallback wins first");
    settle().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "late",
                SessionTitleAutomaticMode::AllPrompts,
                move |request| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async move {
                        Ok(result(
                            "Late",
                            request.messages.iter().map(|m| m.seq).collect(),
                        ))
                    }
                },
            ),
        )
        .unwrap();
    append_route(&session, "initial");
    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        env.titles.get(&session).unwrap().source,
        SessionTitleSource::Fallback
    ));
    append_human(&session, "Later prompt uses provider");
    append_route(&session, "change");
    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        env.titles.get(&session).unwrap().source,
        SessionTitleSource::Provider { .. }
    ));
    env.shutdown().await;
}

#[tokio::test]
async fn does_not_title_a_session_disposed_before_activation() {
    let env = setup_base().await;
    let detached = env
        .sessions
        .prepare(
            Some("disposed-before-activation".into()),
            Default::default(),
        )
        .unwrap();
    let detach = env.sessions.enter(&env.ctx, detached.clone()).unwrap();
    env.sessions.announce(&detached).unwrap();
    append_human(&detached, "Removed during activation");
    detach.detach();
    let live = env
        .sessions
        .create(
            &env.ctx,
            Some("live-during-activation".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    append_human(&live, "Still live");
    env.mount_titles(config()).await;
    let titles = env.ctx.get::<SessionTitle>().unwrap();
    assert!(
        titles
            .refresh(&detached, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not live")
    );
    assert!(titles.get(&detached).is_none());
    assert_eq!(
        titles.refresh(&live, None).await.unwrap().unwrap().title,
        "Still live"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn does_not_write_a_title_after_service_activation_is_disposed() {
    let env = setup_base().await;
    let fiber = env.mount_titles(config()).await;
    let service = (*env.ctx.get::<SessionTitle>().unwrap()).clone();
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("activation-disposed".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    let disposal = tokio::spawn(async move { fiber.dispose().await });
    tokio::task::yield_now().await;
    append_human(&session, "Never commit this title");
    disposal.await.unwrap();
    assert!(service.refresh(&session, None).await.is_err());
    assert!(service.get(&session).is_none());
    env.shutdown().await;
}

#[tokio::test]
async fn rejected_activation_never_publishes_a_title_service() {
    let env = setup_base().await;
    let mut conflict = hoh_session_title::title_projection_definition();
    conflict.key = "titleInput".into();
    env.projections.register(&env.ctx, conflict).await.unwrap();
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("activation-rejected".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    let activation = env.ctx.plugin::<SessionTitlePlugin>(config()).unwrap();
    assert!(activation.settled().await.is_err());
    assert!(env.ctx.get::<SessionTitle>().is_none());
    assert!(
        !env.projections
            .snapshot(&session, None)
            .unwrap()
            .values
            .contains_key("title")
    );
    env.shutdown().await;
}

#[tokio::test]
async fn one_deferred_failure_does_not_prevent_another_session_title() {
    let env = setup_base().await;
    let invalid = env
        .sessions
        .prepare(Some("invalid-first".into()), Default::default())
        .unwrap();
    let detach = env.sessions.enter(&env.ctx, invalid.clone()).unwrap();
    env.sessions.announce(&invalid).unwrap();
    let target = invalid.clone();
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
    append_human(&invalid, "Failed title publication");
    let valid = env
        .sessions
        .create(
            &env.ctx,
            Some("valid-second".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    append_human(&valid, "Independent fallback");
    settle().await;
    let titles = env.ctx.get::<SessionTitle>().unwrap();
    assert!(titles.get(&invalid).is_none());
    assert!(
        warnings
            .lock()
            .unwrap()
            .iter()
            .any(|message| message.contains("fallback title update failed"))
    );
    assert_eq!(titles.get(&valid).unwrap().title, "Independent fallback");
    env.shutdown().await;
}
