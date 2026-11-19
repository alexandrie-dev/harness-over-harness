use hoh_kernel::{Context, Fiber, Kernel};
use hoh_session::{Session, SessionSeqCursor};
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use hoh_system_prompt::SystemPrompt;
use hoh_tool_todo::{TODOS_PROJECTION, ToolTodo};
use hoh_tools::ToolRuntime;
use serde_json::json;

struct Bench {
    kernel: Kernel,
    ctx: Context,
    session: Session,
    tool: Option<Fiber>,
}

async fn harness(with_tool: bool) -> Bench {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({ "includeRuntimeContext": false }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let tool = if with_tool {
        let fiber = ctx
            .plugin::<ToolTodo>(json!({ "allowParallelInProgress": true }))
            .unwrap();
        fiber.settled().await.unwrap();
        Some(fiber)
    } else {
        None
    };
    let session = Session::create("projection".into(), None, None, None, None).unwrap();
    Bench {
        kernel,
        ctx,
        session,
        tool,
    }
}

fn seed(session: &Session) {
    session
        .append("test/seed", json!({ "value": "hi" }), None)
        .unwrap();
}

#[tokio::test]
async fn serves_null_before_first_write() {
    let bench = harness(true).await;
    seed(&bench.session);
    let snapshot = bench
        .ctx
        .get::<SessionProjections>()
        .unwrap()
        .snapshot(&bench.session, None)
        .unwrap();
    assert_eq!(snapshot.values[TODOS_PROJECTION].as_ref(), &json!(null));
    assert_eq!(
        snapshot.as_of_seq,
        SessionSeqCursor::Seq(
            bench
                .session
                .snapshot_events(None, None)
                .last()
                .unwrap()
                .seq()
        )
    );
    bench.kernel.shutdown().await;
}

#[tokio::test]
async fn serves_latest_whole_list_after_writes() {
    let bench = harness(true).await;
    seed(&bench.session);
    bench
        .session
        .append("turn/start", json!({ "turn": 1 }), None)
        .unwrap();
    bench
        .session
        .append(
            "todo/write",
            json!({ "todos": [{ "content": "a", "status": "pending" }] }),
            None,
        )
        .unwrap();
    let second = json!([
        { "content": "a", "status": "completed" },
        { "content": "b", "status": "in_progress" }
    ]);
    let event = bench
        .session
        .append("todo/write", json!({ "todos": second }), None)
        .unwrap();
    let snapshot = bench
        .ctx
        .get::<SessionProjections>()
        .unwrap()
        .snapshot(&bench.session, None)
        .unwrap();
    assert_eq!(snapshot.values[TODOS_PROJECTION].as_ref(), &second);
    assert_eq!(snapshot.as_of_seq, SessionSeqCursor::Seq(event.seq()));
    bench.kernel.shutdown().await;
}

#[tokio::test]
async fn next_turn_start_clears_but_turn_end_keeps_the_plan() {
    let bench = harness(true).await;
    seed(&bench.session);
    let list = json!([{ "content": "done", "status": "completed" }]);
    bench
        .session
        .append("turn/start", json!({ "turn": 1 }), None)
        .unwrap();
    bench
        .session
        .append("todo/write", json!({ "todos": list }), None)
        .unwrap();
    bench
        .session
        .append(
            "turn/end",
            json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();
    let projections = bench.ctx.get::<SessionProjections>().unwrap();
    assert_eq!(
        projections.snapshot(&bench.session, None).unwrap().values[TODOS_PROJECTION].as_ref(),
        &list
    );
    let start = bench
        .session
        .append("turn/start", json!({ "turn": 2 }), None)
        .unwrap();
    let cleared = projections.snapshot(&bench.session, None).unwrap();
    assert_eq!(cleared.values[TODOS_PROJECTION].as_ref(), &json!(null));
    assert_eq!(cleared.as_of_seq, SessionSeqCursor::Seq(start.seq()));
    bench.kernel.shutdown().await;
}

#[tokio::test]
async fn composition_without_tool_has_no_todos_key() {
    let bench = harness(false).await;
    seed(&bench.session);
    let snapshot = bench
        .ctx
        .get::<SessionProjections>()
        .unwrap()
        .snapshot(&bench.session, None)
        .unwrap();
    assert!(!snapshot.values.contains_key(TODOS_PROJECTION));
    bench.kernel.shutdown().await;
}

#[tokio::test]
async fn unloading_tool_drops_projection_key() {
    let bench = harness(true).await;
    seed(&bench.session);
    let projections = bench.ctx.get::<SessionProjections>().unwrap();
    assert_eq!(
        projections.snapshot(&bench.session, None).unwrap().values[TODOS_PROJECTION].as_ref(),
        &json!(null)
    );
    bench.tool.unwrap().disposer().unwrap().dispose().await;
    assert!(
        !projections
            .snapshot(&bench.session, None)
            .unwrap()
            .values
            .contains_key(TODOS_PROJECTION)
    );
    bench.kernel.shutdown().await;
}

#[tokio::test]
async fn todos_is_excluded_from_projection_summaries() {
    let bench = harness(true).await;
    let keys = bench
        .ctx
        .get::<SessionProjections>()
        .unwrap()
        .summary_keys();
    assert!(!keys.contains(TODOS_PROJECTION));
    bench.kernel.shutdown().await;
}
