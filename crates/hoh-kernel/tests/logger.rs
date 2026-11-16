mod support;
use hoh_kernel::*;
use std::sync::{Arc, Mutex};
use support::*;
#[tokio::test(start_paused = true)]
async fn keeps_the_bounded_buffer_in_place_and_chronological() {
    let (k, r, _) = harness();
    let logger = r.logger();
    for i in 0..1005 {
        logger.info(i);
    }
    let records = logger.buffer();
    assert_eq!(records.len(), 1000);
    assert_eq!(records[0].message, "5");
    assert_eq!(records[999].message, "1004");
    assert!(records.windows(2).all(|p| p[0].sn + 1 == p[1].sn));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn disposes_the_exporter_that_registered_the_disposer() {
    let (k, r, _) = harness();
    let a = Arc::new(Mutex::new(vec![]));
    let b = Arc::new(Mutex::new(vec![]));
    let aa = a.clone();
    let first = r
        .logger()
        .exporter(Levels::default(), move |r| {
            lock(&aa).push(r.message.clone())
        })
        .unwrap();
    let bb = b.clone();
    r.logger()
        .exporter(Levels::default(), move |r| {
            lock(&bb).push(r.message.clone())
        })
        .unwrap();
    r.logger().info("first");
    first.dispose().await;
    r.logger().info("second");
    assert_eq!(*lock(&a), ["first"]);
    assert_eq!(*lock(&b), ["first", "second"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn uses_fiber_name_when_called_from_outside_any_service() {
    let (k, r, _) = harness();
    r.logger().info("root");
    let f = r
        .inject(|ctx, (): ()| async move {
            ctx.logger().info("anonymous");
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    assert!(
        r.logger()
            .buffer()
            .iter()
            .all(|r| r.name.as_ref() == "root")
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn honours_explicit_name_argument() {
    let (k, r, _) = harness();
    r.logger().named("custom").info("message");
    assert_eq!(r.logger().buffer()[0].name.as_ref(), "custom");
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn honours_intercept_name() {
    let (k, r, _) = harness();
    r.intercept("logger", serde_json::json!({"name":"intercepted"}))
        .logger()
        .info("message");
    assert_eq!(r.logger().buffer()[0].name.as_ref(), "intercepted");
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn service_writes_through_its_own_context_under_its_fiber_name() {
    let (k, r, _) = harness();
    let f = r.plugin::<Recorded>(serde_json::json!({})).unwrap();
    f.settled().await.unwrap();
    let snap = k.snapshot().await;
    assert_eq!(snap.fibers[1].name.as_ref(), "recorded");
    let t = r.tag::<Arc<Track>>().unwrap();
    t.context().logger().info("service");
    assert_eq!(
        r.logger().buffer().last().unwrap().name.as_ref(),
        "recorded"
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn logs_an_error_whose_errors_array_is_empty() {
    let (k, r, _) = harness();
    r.logger().error(KernelError::Listeners {
        event: "empty",
        errors: vec![],
    });
    assert_eq!(r.logger().buffer().len(), 1);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn flattens_an_aggregate_error_into_its_members() {
    let (k, r, _) = harness();
    let f = r
        .inject(|_, (): ()| async {
            Err(Box::new(KernelError::Listeners {
                event: "test",
                errors: vec![Arc::new("one".into()), Arc::new("two".into())],
            }) as Error)
        })
        .unwrap();
    assert!(f.settled().await.is_err());
    assert_eq!(
        r.logger()
            .buffer()
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn exporter_levels_override_context_level_and_default() {
    let (k, r, _) = harness();
    let seen = Arc::new(Mutex::new(vec![]));
    let output = seen.clone();
    r.logger()
        .exporter(
            Levels {
                default: Some(Level::Warn),
                by_name: std::collections::HashMap::from([("debugger".into(), Level::Debug)]),
            },
            move |r| lock(&output).push(r.message.clone()),
        )
        .unwrap();
    r.logger().info("filtered");
    r.logger().warn("warning");
    r.logger().named("debugger").debug("debug");
    assert_eq!(*lock(&seen), ["warning", "debug"]);
    k.shutdown().await;
}
