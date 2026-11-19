mod support;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use futures_util::StreamExt;
use hoh_llm::{GenerateOptions, LlmRuntime, mark_agent_loop_request};
use hoh_session::{CreateSessionOptions, SessionForkSource};
use hoh_session_title::{SessionTitleAutomaticMode, SessionTitleModelIdentity, SessionTitleSource};

use support::*;

fn seqs(request: &hoh_session_title::SessionTitleProviderRequest) -> Vec<hoh_session::SessionSeq> {
    request.messages.iter().map(|message| message.seq).collect()
}

#[tokio::test]
async fn empty_fork_does_not_schedule_first_prompt_generation() {
    let env = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "fork",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(result("unused", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let source = session(&env, "empty-fork-source");
    let child = env
        .sessions
        .fork(
            &env.ctx,
            SessionForkSource::Session(source),
            None,
            Some("empty-fork-child".into()),
        )
        .unwrap();
    assert_eq!(child.header()["isSeeded"], true);
    append_human(&child, "First own prompt");
    append_route(&child, "initial");
    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    env.shutdown().await;
}

#[tokio::test]
async fn empty_subagent_does_not_schedule_first_prompt_generation() {
    let env = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "subagent",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(result("unused", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let source = session(&env, "empty-subagent-source");
    let empty = env
        .sessions
        .create(
            &env.ctx,
            Some("empty-subagent".into()),
            CreateSessionOptions {
                meta: Some(serde_json::json!({
                    "origin": "subagent",
                    "parentSession": source.id(),
                })),
                ..Default::default()
            },
        )
        .unwrap();
    append_human(&empty, "First own prompt");
    append_route(&empty, "initial");
    settle().await;
    assert_eq!(empty.header()["origin"], "subagent");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    env.shutdown().await;
}

#[tokio::test]
async fn inherited_title_is_kept_until_all_prompts_updates_it() {
    let env = setup().await;
    let parent = session(&env, "parent");
    parent
        .append("turn/start", serde_json::json!({"turn":1}), None)
        .unwrap();
    let inherited = append_human(&parent, "Inherited title prompt");
    settle().await;
    parent
        .append(
            "turn/end",
            serde_json::json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    let child = env
        .sessions
        .fork(
            &env.ctx,
            SessionForkSource::Session(parent.clone()),
            None,
            Some("child".into()),
        )
        .unwrap();
    assert_eq!(env.titles.get(&child), env.titles.get(&parent));

    let first_calls = Arc::new(AtomicUsize::new(0));
    let seen = first_calls.clone();
    let first = env
        .titles
        .register(
            &env.ctx,
            TestProvider::new(
                "fork-first",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(result("Should not run", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    child
        .append("turn/start", serde_json::json!({"turn":2}), None)
        .unwrap();
    let child_message = append_human(&child, "Child follow-up prompt");
    settle().await;
    append_route(&child, "initial");
    settle().await;
    assert_eq!(first_calls.load(Ordering::SeqCst), 0);
    first.dispose().await;

    let all_calls = Arc::new(AtomicUsize::new(0));
    let seen = all_calls.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "fork-all",
                SessionTitleAutomaticMode::AllPrompts,
                move |request| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(result("Fork all prompts", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    child
        .append("turn/start", serde_json::json!({"turn":3}), None)
        .unwrap();
    let latest = append_human(&child, "Retitle the fork now");
    settle().await;
    append_route(&child, "change");
    settle().await;
    assert_eq!(all_calls.load(Ordering::SeqCst), 1);
    let title = env.titles.get(&child).unwrap();
    assert_eq!(title.title, "Fork all prompts");
    assert_eq!(title.message_seqs, vec![inherited, child_message, latest]);
    assert_eq!(
        env.titles.get(&parent).unwrap().title,
        "Inherited title prompt"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn first_prompt_provider_runs_once_and_refresh_retries() {
    let env = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "first",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    let number = seen.fetch_add(1, Ordering::SeqCst) + 1;
                    async move { Ok(result(&format!("Generated {number}"), seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "first-once");
    append_human(&session, "First prompt");
    settle().await;
    append_route(&session, "initial");
    settle().await;
    append_human(&session, "Second prompt");
    append_route(&session, "change");
    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        env.titles
            .refresh(&session, None)
            .await
            .unwrap()
            .unwrap()
            .title,
        "Generated 2"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn multibyte_underivable_fallback_is_absent_from_provider_request() {
    let env = setup_with(
        serde_json::json!({"fallbackMaxWords":5,"fallbackMaxBytes":1,"maxTitleBytes":80}),
    )
    .await;
    let observed = Arc::new(Mutex::new(None));
    let output = observed.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "bytes",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    *output.lock().unwrap() = Some(request.current_title.clone());
                    async move { Ok(result("Emoji title", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "emoji-provider");
    append_human(&session, "😀");
    settle().await;
    env.titles.refresh(&session, None).await.unwrap();
    assert!(observed.lock().unwrap().as_ref().unwrap().is_none());
    env.shutdown().await;
}

#[tokio::test]
async fn latest_accepted_title_is_passed_to_each_revision() {
    let env = setup().await;
    let titles = Arc::new(Mutex::new(Vec::new()));
    let seen = titles.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "current",
                SessionTitleAutomaticMode::AllPrompts,
                move |request| {
                    seen.lock().unwrap().push(
                        request
                            .current_title
                            .as_ref()
                            .map(|title| title.title.clone()),
                    );
                    async move { Ok(result("Provider revision", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "current-title");
    append_human(&session, "Initial prompt");
    settle().await;
    env.titles.refresh(&session, None).await.unwrap();
    env.titles.refresh(&session, None).await.unwrap();
    {
        let titles = titles.lock().unwrap();
        assert_eq!(titles[0].as_deref(), Some("Initial prompt"));
        assert_eq!(titles[1].as_deref(), Some("Provider revision"));
    }
    env.shutdown().await;
}

#[tokio::test]
async fn provider_input_preserves_message_order() {
    let env = setup().await;
    let texts = Arc::new(Mutex::new(Vec::new()));
    let seen = texts.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "order",
                SessionTitleAutomaticMode::AllPrompts,
                move |request| {
                    *seen.lock().unwrap() = request
                        .messages
                        .iter()
                        .map(|message| message.text.clone())
                        .collect();
                    async move { Ok(result("Ordered", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "order");
    for index in 0..70 {
        append_human(&session, &format!("Prompt {index}"));
    }
    settle().await;
    env.titles.refresh(&session, None).await.unwrap();
    assert_eq!(
        *texts.lock().unwrap(),
        (0..70)
            .map(|index| format!("Prompt {index}"))
            .collect::<Vec<_>>(),
    );
    env.shutdown().await;
}

#[tokio::test]
async fn scheduled_watermark_excludes_a_newer_prompt() {
    let env = setup().await;
    let messages = Arc::new(Mutex::new(Vec::new()));
    let seen = messages.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "watermark",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    *seen.lock().unwrap() = request.messages.clone();
                    async move { Ok(result("Watermark", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "watermark");
    let first = append_human(&session, "first");
    settle().await;
    append_human(&session, "newer");
    append_route(&session, "initial");
    settle().await;
    {
        let messages = messages.lock().unwrap();
        assert_eq!(
            messages.as_slice(),
            [hoh_session_title::SessionTitleUserMessage {
                seq: first,
                text: "first".into(),
            }]
        );
    }
    assert_eq!(env.titles.get(&session).unwrap().message_seqs, vec![first]);
    env.shutdown().await;
}

#[tokio::test]
async fn rejects_second_provider_and_allows_replacement_after_disposal() {
    let env = setup().await;
    let first = env
        .titles
        .register(
            &env.ctx,
            TestProvider::new(
                "first",
                SessionTitleAutomaticMode::FirstPrompt,
                |request| async move { Ok(result("First", seqs(&request))) },
            ),
        )
        .unwrap();
    let second = env.titles.register(
        &env.ctx,
        TestProvider::new(
            "second",
            SessionTitleAutomaticMode::FirstPrompt,
            |request| async move { Ok(result("Second", seqs(&request))) },
        ),
    );
    assert!(
        second
            .err()
            .unwrap()
            .to_string()
            .contains("already registered")
    );
    first.dispose().await;
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "replacement",
                SessionTitleAutomaticMode::FirstPrompt,
                |request| async move { Ok(result("Replacement", seqs(&request))) },
            ),
        )
        .unwrap();
    env.shutdown().await;
}

#[tokio::test]
async fn replacement_discards_previous_provider_late_result() {
    let env = setup().await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let observed = cancelled.clone();
    let first = env
        .titles
        .register(
            &env.ctx,
            TestProvider::new(
                "old",
                SessionTitleAutomaticMode::FirstPrompt,
                move |request| {
                    let observed = observed.clone();
                    async move {
                        request.signal.cancelled().await;
                        observed.store(true, Ordering::SeqCst);
                        Ok(result("Late old", seqs(&request)))
                    }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "replace-late");
    append_human(&session, "prompt");
    settle().await;
    let titles = env.titles.clone();
    let refresh_session = session.clone();
    let refresh = tokio::spawn(async move { titles.refresh(&refresh_session, None).await });
    settle().await;
    let disposal = tokio::spawn(async move { first.dispose().await });
    settle().await;
    assert!(cancelled.load(Ordering::SeqCst));
    let _ = refresh.await.unwrap();
    disposal.await.unwrap();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "new",
                SessionTitleAutomaticMode::FirstPrompt,
                |request| async move { Ok(result("New", seqs(&request))) },
            ),
        )
        .unwrap();
    assert_ne!(env.titles.get(&session).unwrap().title, "Late old");
    env.shutdown().await;
}

#[tokio::test]
async fn newer_all_messages_revision_supersedes_older_work() {
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
                "revisions",
                SessionTitleAutomaticMode::AllPrompts,
                move |request| {
                    let number = seen.fetch_add(1, Ordering::SeqCst);
                    let started = started.clone();
                    let release = release.clone();
                    let signal = signal.clone();
                    async move {
                        if number == 0 {
                            *signal.lock().unwrap() = Some(request.signal.clone());
                            started.notify_one();
                            release.notified().await;
                            Ok(result("Old ignored result", seqs(&request)))
                        } else {
                            Ok(result("Newest complete title", seqs(&request)))
                        }
                    }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "revisions");
    let first = append_human(&session, "one");
    settle().await;
    append_route(&session, "initial");
    first_started.notified().await;
    let second = append_human(&session, "two");
    assert!(first_signal.lock().unwrap().as_ref().unwrap().aborted());
    append_route(&session, "change");
    settle().await;
    let accepted = env.titles.get(&session).unwrap();
    assert_eq!(accepted.title, "Newest complete title");
    assert_eq!(accepted.message_seqs, vec![first, second]);
    first_release.notify_one();
    settle().await;
    assert_eq!(
        env.titles.get(&session).unwrap().title,
        "Newest complete title"
    );
    env.shutdown().await;
}

#[tokio::test]
async fn all_messages_revision_can_reuse_logged_route() {
    let env = setup().await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "route",
                SessionTitleAutomaticMode::AllPrompts,
                move |request| {
                    seen.lock()
                        .unwrap()
                        .push((request.messages.clone(), request.route.clone()));
                    async move { Ok(result("Routed", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let session = session(&env, "route");
    session
        .append("turn/start", serde_json::json!({"turn":1}), None)
        .unwrap();
    session
        .append("step/start", serde_json::json!({"turn":1,"step":1}), None)
        .unwrap();
    let first = append_human(&session, "First routed prompt");
    settle().await;
    append_route(&session, "initial");
    settle().await;
    session
        .append("step/end", serde_json::json!({"turn":1,"step":1}), None)
        .unwrap();
    session
        .append(
            "turn/end",
            serde_json::json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    session
        .append("turn/start", serde_json::json!({"turn":2}), None)
        .unwrap();
    session
        .append("step/start", serde_json::json!({"turn":2,"step":1}), None)
        .unwrap();
    let second = append_human(&session, "Second prompt on same route");
    settle().await;
    let mut options = GenerateOptions {
        provider: "main-route".into(),
        model: "chat-model".into(),
        session_id: Some(session.id().clone()),
        ..Default::default()
    };
    mark_agent_loop_request(&mut options);
    let mut stream = LlmRuntime::new().stream(&env.ctx, options);
    let _ = stream.next().await;
    settle().await;
    {
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1]
                .0
                .iter()
                .map(|message| message.seq)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
        assert_eq!(
            requests[1].1,
            Some(SessionTitleModelIdentity {
                provider: "main-route".into(),
                model: "chat-model".into(),
            })
        );
    }
    env.shutdown().await;
}

#[tokio::test]
async fn ignores_model_streams_that_are_not_a_matching_loop_request() {
    let env = setup().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "pending",
                SessionTitleAutomaticMode::AllPrompts,
                move |request| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(result("Should not run", seqs(&request))) }
                },
            ),
        )
        .unwrap();
    let runtime = LlmRuntime::new();
    let mut plain = runtime.stream(
        &env.ctx,
        GenerateOptions {
            provider: "main-route".into(),
            model: "chat-model".into(),
            ..Default::default()
        },
    );
    let _ = plain.next().await;
    let mut missing_options = GenerateOptions {
        provider: "main-route".into(),
        model: "chat-model".into(),
        session_id: Some("missing".into()),
        ..Default::default()
    };
    mark_agent_loop_request(&mut missing_options);
    let mut missing = runtime.stream(&env.ctx, missing_options);
    let _ = missing.next().await;
    let quiet = session(&env, "quiet");
    let mut quiet_options = GenerateOptions {
        provider: "main-route".into(),
        model: "chat-model".into(),
        session_id: Some(quiet.id().clone()),
        ..Default::default()
    };
    mark_agent_loop_request(&mut quiet_options);
    let mut quiet_stream = runtime.stream(&env.ctx, quiet_options);
    let _ = quiet_stream.next().await;
    let pending = session(&env, "unmatched-boundary");
    pending
        .append("turn/start", serde_json::json!({"turn":1}), None)
        .unwrap();
    append_human(&pending, "Wait for a matching request boundary");
    settle().await;
    let mut pending_options = GenerateOptions {
        provider: "main-route".into(),
        model: "chat-model".into(),
        session_id: Some(pending.id().clone()),
        ..Default::default()
    };
    mark_agent_loop_request(&mut pending_options);
    let mut pending_stream = runtime.stream(&env.ctx, pending_options);
    let _ = pending_stream.next().await;
    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    env.shutdown().await;
}

#[tokio::test]
async fn automatic_failure_is_contained_but_explicit_refresh_rejects() {
    let env = setup().await;
    env.titles
        .register(
            &env.ctx,
            TestProvider::new(
                "failure",
                SessionTitleAutomaticMode::AllPrompts,
                |_request| async move { Err("provider exploded".into()) },
            ),
        )
        .unwrap();
    let session = session(&env, "failure");
    append_human(&session, "Keep fallback");
    settle().await;
    append_route(&session, "initial");
    settle().await;
    assert!(matches!(
        env.titles.get(&session).unwrap().source,
        SessionTitleSource::Fallback
    ));
    assert!(
        env.titles
            .refresh(&session, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("provider exploded")
    );
    env.shutdown().await;
}
