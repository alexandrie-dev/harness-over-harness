mod support;
use hoh_kernel::*;
use serde_json::json;
use std::sync::Arc;
use support::*;
#[tokio::test(start_paused = true)]
async fn exposes_the_root_context_through_context_root_only() {
    let (k, r, _) = harness();
    let f = r
        .inject(|ctx, (): ()| async move {
            assert_eq!(ctx.root().fiber().uid(), 0);
            assert_ne!(ctx.fiber().uid(), 0);
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_inherit_the_injected_view_across_an_isolation_boundary() {
    let (k, r, _) = harness();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    let f = r
        .inject(|ctx, d: ADeps| async move {
            assert_eq!(*d.a, 1);
            assert!(ctx.isolate("foo", Realm::new()).get::<A>().is_none());
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn isolates_a_context_pinned_to_its_parent_realm_label_from_the_injected_view() {
    let (k, r, _) = harness();
    r.provide::<A>(Arc::new(1)).await.unwrap();
    let f = r
        .inject(|ctx, d: ADeps| async move {
            let pinned = ctx.isolate("foo", ctx.realm_of("foo"));
            assert_eq!(*d.a, 1);
            assert_eq!(*pinned.get::<A>().unwrap(), 1);
            assert_eq!(pinned.realm_of("foo"), Realm::ROOT);
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn settles_pending_and_failed_fibers_to_disposed() {
    let (k, r, _) = harness();
    let pending = r.inject(|_, _: MissingDeps| async { Ok(()) }).unwrap();
    pending.dispose().await;
    assert_eq!(pending.state(), FiberState::Disposed);
    let failed = r.plugin::<Recorded>(json!({"fail":true})).unwrap();
    assert!(failed.settled().await.is_err());
    assert_eq!(failed.state(), FiberState::Failed);
    failed.dispose().await;
    assert_eq!(failed.state(), FiberState::Disposed);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn names_the_failing_event_in_the_parallel_aggregate_error() {
    let (k, r, _) = harness();
    r.on_parallel::<Par, _>(|_| async { Err("listener failed".into()) })
        .unwrap();
    let error = r.parallel(Par).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "one or more listeners of event \"parallel\" failed"
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn formats_config_validation_paths_with_keyed_segments() {
    let error = ValidationError {
        issues: vec![Issue {
            message: "bad".into(),
            path: vec![PathSeg::Index(0), PathSeg::Key("name".into())],
        }],
    };
    assert_eq!(error.to_string(), "invalid config:\n  - bad (at 0.name)");
    assert_eq!(
        ValidationError {
            issues: vec![Issue {
                message: "bad".into(),
                path: vec![]
            }]
        }
        .to_string(),
        "invalid config:\n  - bad"
    );
}
#[tokio::test(start_paused = true)]
async fn logs_an_aggregate_error_without_members_as_itself() {
    let (k, r, _) = harness();
    r.logger().error(KernelError::Listeners {
        event: "empty",
        errors: vec![],
    });
    assert_eq!(r.logger().buffer().len(), 1);
    assert_eq!(
        r.logger().buffer()[0].message,
        "one or more listeners of event \"empty\" failed"
    );
    k.shutdown().await;
}
