mod support;
use hoh_kernel::*;
use serde_json::json;
use support::*;
#[tokio::test(start_paused = true)]
async fn dispose_by_component() {
    let (k, r, t) = harness();
    let f = r.plugin::<Recorded>(json!({})).unwrap();
    f.settled().await.unwrap();
    f.dispose().await;
    f.dispose().await;
    assert_eq!(t.list(), ["load 0", "unload 0"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn dispose_manually() {
    let (k, r, t) = harness();
    let d = r
        .effect("test", {
            let t = t.clone();
            move || Undo::sync(move || t.push("disposed"))
        })
        .unwrap();
    assert!(t.list().is_empty());
    d.dispose().await;
    assert_eq!(t.list(), ["disposed"]);
    k.shutdown().await;
    assert_eq!(t.list(), ["disposed"]);
}
#[tokio::test(start_paused = true)]
async fn yield_dispose() {
    let (k, r, t) = harness();
    let d = r
        .try_effect::<KernelError>("composed", |u| {
            let t1 = t.clone();
            u.push(Undo::sync(move || t1.push("1")))?;
            u.push(r.on::<Ping>(|_| {})?)?;
            let t2 = t.clone();
            u.push(Undo::sync(move || t2.push("2")))?;
            u.push(r.try_effect::<KernelError>("inner", |u| {
                u.push(r.on::<Ping>(|_| {})?)?;
                let t = t.clone();
                u.push(Undo::sync(move || t.push("3")))
            })?)?;
            Ok(())
        })
        .unwrap();
    d.dispose().await;
    assert_eq!(t.list(), ["3", "2", "1"]);
    k.shutdown().await;
}
async fn async_steps(abort_at: Option<u64>, steps: u64, finally: bool) -> Vec<String> {
    let (k, r, t) = harness();
    let t2 = t.clone();
    let ctx = r.clone();
    let setup = tokio::spawn(async move {
        ctx.effect_async::<KernelError, _>("steps", move |u| async move {
            struct Final(std::sync::Arc<Track>, bool);
            impl Drop for Final {
                fn drop(&mut self) {
                    if self.1 {
                        self.0.push("0");
                    }
                }
            }
            let _final = Final(t2.clone(), finally);
            for i in 0..steps {
                tokio::time::sleep(Duration::from_millis(100)).await;
                t2.push((i * 2 + 1).to_string());
                let t = t2.clone();
                u.push(Undo::sync(move || t.push((i * 2 + 2).to_string())))?;
            }
            Ok(())
        })
        .await
    });
    turn().await;
    if let Some(ms) = abort_at {
        tokio::time::advance(Duration::from_millis(ms)).await;
        turn().await;
        let shutdown = k.shutdown();
        tokio::time::advance(Duration::from_millis(100)).await;
        let result = setup.await.unwrap();
        assert!(result.is_err() || ms >= steps * 100);
        shutdown.await;
    } else {
        for _ in 0..steps {
            tokio::time::advance(Duration::from_millis(100)).await;
            turn().await;
        }
        let d = setup.await.unwrap().unwrap();
        d.dispose().await;
        k.shutdown().await;
    }
    t.list()
}
#[tokio::test(start_paused = true)]
async fn async_return_1() {
    assert_eq!(async_steps(None, 1, false).await, ["1", "2"]);
}
#[tokio::test(start_paused = true)]
async fn async_return_2() {
    assert_eq!(async_steps(Some(0), 1, false).await, ["1", "2"]);
}
#[tokio::test(start_paused = true)]
async fn async_yield_1() {
    assert_eq!(
        async_steps(None, 3, false).await,
        ["1", "3", "5", "6", "4", "2"]
    );
}
#[tokio::test(start_paused = true)]
async fn async_yield_2_aborted() {
    assert_eq!(async_steps(Some(50), 3, false).await, ["1", "2"]);
}
#[tokio::test(start_paused = true)]
async fn async_yield_3_aborted() {
    assert_eq!(async_steps(Some(100), 3, false).await, ["1", "3", "4", "2"]);
}
#[tokio::test(start_paused = true)]
async fn async_yield_aborted_finally() {
    let (k, r, t) = harness();
    let tt = t.clone();
    let setup = tokio::spawn(async move {
        r.effect_async::<KernelError, _>("finally", move |u| async move {
            struct Finally(std::sync::Arc<Track>);
            impl Drop for Finally {
                fn drop(&mut self) {
                    self.0.push("0");
                }
            }
            let _finally = Finally(tt.clone());
            let t1 = tt.clone();
            u.push(Undo::sync(move || t1.push("1")))?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            u.push(Undo::sync(move || tt.push("2")))?;
            Ok(())
        })
        .await
    });
    turn().await;
    tokio::time::advance(Duration::from_millis(50)).await;
    let shutdown = k.shutdown();
    turn().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    assert!(setup.await.unwrap().is_err());
    shutdown.await;
    assert_eq!(t.list(), ["0", "2", "1"]);
}
#[tokio::test(start_paused = true)]
async fn async_yield_4_await_dispose() {
    assert_eq!(
        async_steps(None, 3, false).await,
        ["1", "3", "5", "6", "4", "2"]
    );
}
#[tokio::test(start_paused = true)]
async fn return_with_error() {
    let (k, r, t) = harness();
    let result = r.try_effect::<Error>("error", |_| Err("test".into()));
    assert_eq!(result.err().unwrap().to_string(), "test");
    assert!(t.list().is_empty());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn yield_with_error() {
    let (k, r, t) = harness();
    let result = r.try_effect::<Error>("error", |u| {
        let t = t.clone();
        u.push(Undo::sync(move || t.push("1")))?;
        Err("test".into())
    });
    assert!(result.is_err());
    assert_eq!(t.list(), ["1"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn async_return_with_error() {
    let (k, r, t) = harness();
    let result = r
        .effect_async::<Error, _>("error", |_| async { Err("test".into()) })
        .await;
    assert_eq!(result.err().unwrap().to_string(), "test");
    assert!(t.list().is_empty());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn async_yield_with_error() {
    let (k, r, t) = harness();
    let result = r
        .effect_async::<Error, _>("error", |u| {
            let t = t.clone();
            async move {
                u.push(Undo::sync(move || t.push("1")))?;
                Err("test".into())
            }
        })
        .await;
    assert_eq!(result.err().unwrap().to_string(), "test");
    assert_eq!(t.list(), ["1"]);
    k.shutdown().await;
}
