mod support;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use hoh_session::Session;
use hoh_session_title::{SessionTitleAutomaticMode, SessionTitleSource};

use support::*;

#[tokio::test]
async fn appends_a_normalized_user_source_title() {
    let env = setup().await;
    let s = session(&env, "rename");
    append_human(&s, "Original prompt");
    settle().await;
    let accepted = env.titles.rename(&s, "  Hand\tpicked   name  ").unwrap();
    assert_eq!(accepted.title, "Hand picked name");
    assert!(accepted.message_seqs.is_empty());
    assert!(matches!(accepted.source, SessionTitleSource::User));
    env.shutdown().await;
}

#[tokio::test]
async fn rejects_empty_titles_and_dead_sessions() {
    let env = setup().await;
    let s = session(&env, "rename-reject");
    assert_eq!(
        env.titles
            .rename(&s, "  \u{1b}[31m  ")
            .unwrap_err()
            .to_string(),
        "session title must contain visible characters"
    );
    let detached = Session::create("detached".into(), None, None, None, None).unwrap();
    assert_eq!(
        env.titles
            .rename(&detached, "name")
            .unwrap_err()
            .to_string(),
        "session \"detached\" is not live in this store"
    );
    assert_eq!(
        env.titles
            .refresh(&detached, None)
            .await
            .unwrap_err()
            .to_string(),
        "session \"detached\" is not live in this store"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn pins_later_messages_and_refresh_unpins() {
    let env = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let provider = TestProvider::new(
        "pin",
        SessionTitleAutomaticMode::AllPrompts,
        move |request| {
            seen.fetch_add(1, Ordering::SeqCst);
            let seqs = request.messages.iter().map(|m| m.seq).collect();
            async move { Ok(result("Provider title", seqs)) }
        },
    );
    env.titles.register(&env.ctx, provider).unwrap();
    let s = session(&env, "pin");
    append_human(&s, "First");
    settle().await;
    env.titles.rename(&s, "Pinned by hand").unwrap();
    append_human(&s, "Second");
    append_route(&s, "change");
    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(env.titles.get(&s).unwrap().title, "Pinned by hand");
    assert_eq!(
        env.titles.refresh(&s, None).await.unwrap().unwrap().title,
        "Provider title"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn fallback_only_refresh_unpins_to_a_rederived_fallback() {
    let env = setup().await;
    let s = session(&env, "fallback-unpin");
    append_human(&s, "Derivable prompt words");
    settle().await;
    env.titles.rename(&s, "Pinned").unwrap();
    let refreshed = env.titles.refresh(&s, None).await.unwrap().unwrap();
    assert_eq!(refreshed.title, "Derivable prompt words");
    assert!(matches!(refreshed.source, SessionTitleSource::Fallback));
    env.shutdown().await;
}

#[tokio::test]
async fn rename_supersedes_an_in_flight_provider() {
    let env = setup().await;
    let signal = Arc::new(Mutex::new(None));
    let observed = signal.clone();
    let gate = Arc::new(tokio::sync::Notify::new());
    let release = gate.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let provider_started = started.clone();
    let provider = TestProvider::new(
        "slow",
        SessionTitleAutomaticMode::AllPrompts,
        move |request| {
            let gate = gate.clone();
            let observed = observed.clone();
            let started = provider_started.clone();
            async move {
                *observed.lock().unwrap() = Some(request.signal.clone());
                started.notify_one();
                gate.notified().await;
                Ok(result(
                    "Late",
                    request.messages.iter().map(|m| m.seq).collect(),
                ))
            }
        },
    );
    env.titles.register(&env.ctx, provider).unwrap();
    let s = session(&env, "slow");
    append_human(&s, "Prompt");
    settle().await;
    append_route(&s, "initial");
    started.notified().await;
    env.titles.rename(&s, "User wins").unwrap();
    assert!(signal.lock().unwrap().as_ref().unwrap().aborted());
    release.notify_waiters();
    settle().await;
    assert_eq!(env.titles.get(&s).unwrap().title, "User wins");
    env.shutdown().await;
}

#[tokio::test]
async fn fallback_refresh_keeps_pin_when_no_code_point_fits() {
    let env = setup_with(
        serde_json::json!({"fallbackMaxWords":5,"fallbackMaxBytes":3,"maxTitleBytes":40}),
    )
    .await;
    let s = session(&env, "emoji");
    append_human(&s, "😀😀");
    settle().await;
    env.titles.rename(&s, "Sticky").unwrap();
    assert_eq!(
        env.titles.refresh(&s, None).await.unwrap().unwrap().title,
        "Sticky"
    );
    env.shutdown().await;
}
