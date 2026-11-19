//! server.ts:297-311 does the shutting-down check and the registration of the command task in
//! one synchronous step; here they now share one lock too. This stress test (multi-thread, the
//! shutdown swept across the query's critical section) states the observable guarantee: when
//! `shutdown()` returns, no command body is alive. It also passes without the shared lock:
//! `CommandRuntime::execute` refuses an aborted signal before it calls the handler
//! (hoh-commands/src/lib.rs, `throw_if_aborted`), and shutdown aborts the lifetime before it
//! snapshots the tasks, so the review's scenario (a handler running over disposing agents)
//! cannot be produced. The lock closes the gap by construction.
mod support;
use hoh_commands::CommandResult;
use hoh_sdk_jsonrpc_server::command_helpers::BuiltinCommandName;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// Counts a command body from its start until it finishes or is dropped.
struct Alive(Arc<AtomicUsize>);
impl Alive {
    fn enter(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter.clone())
    }
}
impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
use support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_racing_shutdown_never_leaves_a_command_running_after_shutdown() {
    for round in 0..200u32 {
        let h = Arc::new(Env::new().await);
        h.initialize().await;
        let commands = commands::Registry::mount(&h).await;
        commands.register("plan", Some(BuiltinCommandName::Plan));
        let alive = Arc::new(AtomicUsize::new(0));
        let counter = alive.clone();
        commands.handler(Arc::new(move |_, _, _, _| {
            let counter = counter.clone();
            Box::pin(async move {
                let _alive = Alive::enter(&counter);
                tokio::time::sleep(std::time::Duration::from_millis(3)).await;
                Ok(Some(CommandResult::Success {
                    text: Some("done".into()),
                    source_event_seq: None,
                }))
            })
        }));
        h.open("main").await;
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let query = {
            let (h, barrier) = (h.clone(), barrier.clone());
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                tokio::runtime::Handle::current().block_on(async move {
                    let _ = h
                        .server
                        .handle_request(
                            "session/query",
                            json!({"sessionId":"main","contentBlocks":[{"type":"text","text":"/plan"}]}),
                        )
                        .await;
                })
            })
        };
        let shutdown = {
            let (h, barrier, alive) = (h.clone(), barrier.clone(), alive.clone());
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                // sweep the offset of the shutdown across the query's critical section
                for i in 0..(round * 5) % 400 {
                    std::hint::black_box(i);
                }
                tokio::runtime::Handle::current().block_on(async move {
                    let _ = h.server.shutdown().await;
                    alive.load(Ordering::SeqCst)
                })
            })
        };
        let running_at_shutdown = shutdown.await.unwrap();
        query.await.unwrap();
        assert_eq!(
            running_at_shutdown, 0,
            "round {round}: a command was still running when shutdown returned"
        );
        let h = Arc::into_inner(h).expect("no other owner");
        h.close().await;
    }
}
