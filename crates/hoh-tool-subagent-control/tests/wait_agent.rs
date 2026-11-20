mod support;

use std::{future::Future, task::Poll};

use futures_util::future::join;
use hoh_agent::{AgentDisposed, AgentInboxInserted, agent_events};
use hoh_error::HarnessError;
use hoh_llm::ToolCallId;
use hoh_timeout::AbortController;
use serde_json::json;

use support::{Env, error_code, message};

fn is_pending<F: Future>(future: std::pin::Pin<&mut F>) -> bool {
    let mut context = std::task::Context::from_waker(futures_util::task::noop_waker_ref());
    matches!(future.poll(&mut context), Poll::Pending)
}

#[tokio::test]
async fn pending_input_returns_without_consuming_it() {
    let env = Env::new(json!({})).await;
    let input = message("pending");
    let id = input.id().clone();
    env.parent.inject(input).unwrap();

    let result = env
        .call("wait_agent", json!({}), Some(env.parent.clone()))
        .await;
    assert_eq!(result.value(), Some(&json!({ "timedOut": false })));
    assert_eq!(env.parent.inbox().next_step()[0].id(), &id);
    env.close().await;
}

#[tokio::test]
async fn wakes_only_for_the_calling_agents_input() {
    let env = Env::new(json!({})).await;
    let other = env
        .harness
        .create(hoh_session::SessionId::from("other"))
        .await
        .unwrap();
    let waiting = env.call(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
    );
    tokio::pin!(waiting);
    for _ in 0..8 {
        assert!(is_pending(waiting.as_mut()));
        tokio::task::yield_now().await;
    }
    other.inject(message("unrelated")).unwrap();
    assert!(is_pending(waiting.as_mut()));
    env.parent.inject(message("for caller")).unwrap();
    assert_eq!(
        waiting.as_mut().await.value(),
        Some(&json!({ "timedOut": false }))
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn concurrent_waits_end_on_the_same_input_without_consuming_it() {
    let env = Env::new(json!({})).await;
    let root = ToolCallId::from("same-code");
    let first = env.call_with(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
        support::live_signal(),
        Some(root.clone()),
    );
    let second = env.call_with(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
        support::live_signal(),
        Some(root.clone()),
    );
    tokio::pin!(first);
    tokio::pin!(second);
    for _ in 0..8 {
        assert!(is_pending(first.as_mut()));
        assert!(is_pending(second.as_mut()));
        tokio::task::yield_now().await;
    }
    let input = message("wake both");
    let id = input.id().clone();
    env.parent.inject(input).unwrap();
    let (first, second) = tokio::time::timeout(
        std::time::Duration::from_millis(1),
        join(first.as_mut(), second.as_mut()),
    )
    .await
    .expect("both concurrent waits must retain their initial seen snapshots");
    assert_eq!(first.value(), Some(&json!({ "timedOut": false })));
    assert_eq!(second.value(), Some(&json!({ "timedOut": false })));
    assert_eq!(env.parent.inbox().next_step()[0].id(), &id);

    env.parent.inbox().clear().unwrap();
    let later = env.call_with(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
        support::live_signal(),
        Some(root),
    );
    tokio::pin!(later);
    assert!(is_pending(later.as_mut()));
    tokio::time::advance(std::time::Duration::from_millis(10_000)).await;
    assert_eq!(
        later.as_mut().await.value(),
        Some(&json!({ "timedOut": true }))
    );
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn pending_input_is_reported_once_per_root_model_call() {
    let env = Env::new(json!({})).await;
    env.parent.inject(message("pending")).unwrap();
    let root = ToolCallId::from("code");

    let first = env
        .call_with(
            "wait_agent",
            json!({ "timeout_ms": 10_000 }),
            Some(env.parent.clone()),
            support::live_signal(),
            Some(root.clone()),
        )
        .await;
    assert_eq!(first.value(), Some(&json!({ "timedOut": false })));

    let second = env.call_with(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
        support::live_signal(),
        Some(root),
    );
    tokio::pin!(second);
    assert!(is_pending(second.as_mut()));
    tokio::time::advance(std::time::Duration::from_millis(10_000)).await;
    assert_eq!(
        second.as_mut().await.value(),
        Some(&json!({ "timedOut": true }))
    );

    let different = env
        .call_with(
            "wait_agent",
            json!({}),
            Some(env.parent.clone()),
            support::live_signal(),
            Some(ToolCallId::from("next-code")),
        )
        .await;
    assert_eq!(different.value(), Some(&json!({ "timedOut": false })));
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn unrelated_inbox_notifications_do_not_restart_the_timeout() {
    let env = Env::new(json!({})).await;
    env.parent.inject(message("already reported")).unwrap();
    let root = ToolCallId::from("code");
    assert_eq!(
        env.call_with(
            "wait_agent",
            json!({ "timeout_ms": 10_000 }),
            Some(env.parent.clone()),
            support::live_signal(),
            Some(root.clone()),
        )
        .await
        .value(),
        Some(&json!({ "timedOut": false }))
    );

    let waiting = env.call_with(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
        support::live_signal(),
        Some(root),
    );
    tokio::pin!(waiting);
    for _ in 0..8 {
        assert!(is_pending(waiting.as_mut()));
        tokio::task::yield_now().await;
    }
    tokio::time::advance(std::time::Duration::from_millis(6_000)).await;
    agent_events(&env.ctx, &env.parent).emit(|agent| AgentInboxInserted {
        agent,
        message: message("next-turn notification"),
    });
    assert!(is_pending(waiting.as_mut()));
    tokio::time::advance(std::time::Duration::from_millis(3_999)).await;
    assert!(is_pending(waiting.as_mut()));
    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    let result = tokio::time::timeout(std::time::Duration::from_millis(1), waiting.as_mut())
        .await
        .expect("the original timeout must be ready exactly at 10 seconds");
    assert_eq!(result.value(), Some(&json!({ "timedOut": true })));
    env.close().await;
}

#[tokio::test]
async fn invalid_timeouts_are_rejected_before_pending_input() {
    let env = Env::new(json!({})).await;
    env.parent.inject(message("still pending")).unwrap();
    for timeout in [json!(9_999), json!(3_600_001)] {
        let result = env
            .call(
                "wait_agent",
                json!({ "timeout_ms": timeout }),
                Some(env.parent.clone()),
            )
            .await;
        assert!(result.is_error());
        assert_eq!(error_code(&result), Some("WAIT_INVALID_TIMEOUT"));
    }
    for timeout in [json!(10_000.5), json!("10000"), json!(null)] {
        let result = env
            .call(
                "wait_agent",
                json!({ "timeout_ms": timeout }),
                Some(env.parent.clone()),
            )
            .await;
        assert_eq!(error_code(&result), Some("INVALID_ARGS"));
    }
    env.close().await;
}

#[tokio::test(start_paused = true)]
async fn accepts_both_timeout_boundaries_and_uses_the_default() {
    for (arguments, expected) in [
        (json!({ "timeout_ms": 10_000 }), 10_000),
        (json!({}), 30_000),
        (json!({ "timeout_ms": 3_600_000 }), 3_600_000),
    ] {
        let env = Env::new(json!({})).await;
        let waiting = env.call("wait_agent", arguments, Some(env.parent.clone()));
        tokio::pin!(waiting);
        assert!(is_pending(waiting.as_mut()));
        tokio::time::advance(std::time::Duration::from_millis(expected - 1)).await;
        assert!(is_pending(waiting.as_mut()));
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        assert_eq!(
            waiting.as_mut().await.value(),
            Some(&json!({ "timedOut": true }))
        );
        env.close().await;
    }
}

#[tokio::test]
async fn preserves_caller_cancellation() {
    let env = Env::new(json!({})).await;
    let controller = AbortController::new();
    let waiting = env.call_with(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
        controller.signal(),
        None,
    );
    tokio::pin!(waiting);
    for _ in 0..8 {
        assert!(is_pending(waiting.as_mut()));
        tokio::task::yield_now().await;
    }
    controller.abort_with(HarnessError::named(
        "CallerStopped",
        "caller stopped waiting",
        "CALLER_STOPPED",
    ));
    let result = waiting.as_mut().await;
    assert_eq!(error_code(&result), Some("CALLER_STOPPED"));
    assert_eq!(result.error().unwrap().message, "caller stopped waiting");
    env.close().await;
}

#[tokio::test]
async fn calling_agent_disposal_rejects_the_wait_as_unavailable() {
    let env = Env::new(json!({})).await;
    let waiting = env.call(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
    );
    tokio::pin!(waiting);
    for _ in 0..8 {
        assert!(is_pending(waiting.as_mut()));
        tokio::task::yield_now().await;
    }

    agent_events(&env.ctx, &env.parent).emit(|agent| AgentDisposed { agent });
    let result = waiting.as_mut().await;
    assert_eq!(error_code(&result), Some("WAIT_UNAVAILABLE"));
    assert_eq!(
        result.error().unwrap().message,
        "wait_agent ended because the calling agent was disposed"
    );
    env.close().await;
}

#[tokio::test]
async fn another_agent_disposal_does_not_interrupt_the_wait() {
    let env = Env::new(json!({})).await;
    let other = env
        .harness
        .create(hoh_session::SessionId::from("other-disposed"))
        .await
        .unwrap();
    let waiting = env.call(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
    );
    tokio::pin!(waiting);
    for _ in 0..8 {
        assert!(is_pending(waiting.as_mut()));
        tokio::task::yield_now().await;
    }

    agent_events(&env.ctx, &other).emit(|agent| AgentDisposed { agent });
    assert!(is_pending(waiting.as_mut()));
    env.parent
        .inject(message("still waiting for caller"))
        .unwrap();
    assert_eq!(
        waiting.as_mut().await.value(),
        Some(&json!({ "timedOut": false }))
    );
    env.close().await;
}

#[tokio::test]
async fn plugin_unload_rejects_wait_and_unregisters_the_tool() {
    let env = Env::new(json!({})).await;
    let waiting = env.call(
        "wait_agent",
        json!({ "timeout_ms": 10_000 }),
        Some(env.parent.clone()),
    );
    tokio::pin!(waiting);
    for _ in 0..8 {
        assert!(is_pending(waiting.as_mut()));
        tokio::task::yield_now().await;
    }
    env.control.dispose().await;
    let result = waiting.await;
    assert_eq!(error_code(&result), Some("WAIT_UNAVAILABLE"));
    assert!(env.tools().get("wait_agent", None, None).is_none());
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_calls_without_a_live_calling_agent() {
    let env = Env::new(json!({})).await;
    let result = env.call("wait_agent", json!({}), None).await;
    assert_eq!(error_code(&result), Some("WAIT_UNAVAILABLE"));
    env.close().await;
}
