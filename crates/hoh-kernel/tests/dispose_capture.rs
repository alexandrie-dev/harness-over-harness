use hoh_kernel::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

tokio::task_local! { static CALLER: String; }

type Seen = Arc<Mutex<Vec<(&'static str, Vec<String>)>>>;

fn texts(values: Vec<Arc<String>>) -> Vec<String> {
    values.iter().map(|value| (**value).clone()).collect()
}

/// An effect whose cleanup records what the teardown it runs in was handed.
fn probe(ctx: &Context, name: &'static str, seen: &Seen) {
    let (seen, own) = (seen.clone(), ctx.clone());
    ctx.effect("probe", move || {
        Undo::future(move || async move {
            seen.lock()
                .unwrap()
                .push((name, texts(own.dispose_captures::<String>())));
            Ok(())
        })
    })
    .unwrap();
}

fn capture_caller(ctx: &Context) -> Disposer {
    ctx.capture_on_dispose::<String>(|| CALLER.try_with(|name| Arc::new(name.clone())).ok())
        .unwrap()
}

fn sorted(seen: &Seen) -> Vec<(&'static str, Vec<String>)> {
    let mut seen = seen.lock().unwrap().clone();
    seen.sort();
    seen
}

fn warnings(ctx: &Context) -> Arc<Mutex<Vec<String>>> {
    let out = Arc::new(Mutex::new(vec![]));
    let sink = out.clone();
    ctx.logger()
        .exporter(
            Levels {
                default: Some(Level::Warn),
                by_name: Default::default(),
            },
            move |record| {
                if record.level == Level::Warn {
                    sink.lock().unwrap().push(record.message.clone());
                }
            },
        )
        .unwrap();
    out
}

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("teardown did not settle")
}

#[tokio::test]
async fn hands_the_callers_value_to_the_teardown_of_the_fiber_and_of_its_descendants() {
    let kernel = Kernel::start();
    let owner = kernel.root().child("owner").unwrap();
    let leaf = owner.child("leaf").unwrap();
    capture_caller(&owner);
    let seen = Seen::default();
    probe(&owner, "owner", &seen);
    probe(&leaf, "leaf", &seen);

    CALLER
        .scope("caller".to_owned(), async { owner.fiber().dispose().await })
        .await;
    assert_eq!(
        sorted(&seen),
        [
            ("leaf", vec!["caller".to_owned()]),
            ("owner", vec!["caller".to_owned()])
        ]
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn hands_nothing_when_the_caller_has_no_value_or_nobody_captures() {
    let kernel = Kernel::start();
    let (plain, unregistered) = (
        kernel.root().child("plain").unwrap(),
        kernel.root().child("unregistered").unwrap(),
    );
    capture_caller(&plain);
    let seen = Seen::default();
    probe(&plain, "plain", &seen);
    probe(&unregistered, "unregistered", &seen);

    // No value in the calling task: the function answers `None`.
    plain.fiber().dispose().await;
    // A value in the calling task, but the only capture function belonged to `plain`, which has
    // unloaded and taken its registration with it.
    CALLER
        .scope("late".to_owned(), async {
            unregistered.fiber().dispose().await
        })
        .await;
    assert_eq!(sorted(&seen), [("plain", vec![]), ("unregistered", vec![])]);
    assert!(kernel.root().dispose_captures::<String>().is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn stops_capturing_once_its_disposer_has_run() {
    let kernel = Kernel::start();
    let (owner, other) = (
        kernel.root().child("owner").unwrap(),
        kernel.root().child("other").unwrap(),
    );
    let registration = capture_caller(&owner);
    let seen = Seen::default();
    probe(&other, "other", &seen);
    registration.dispose().await;

    CALLER
        .scope("ignored".to_owned(), async {
            other.fiber().dispose().await
        })
        .await;
    assert_eq!(sorted(&seen), [("other", vec![])]);
    kernel.shutdown().await;
}

/// A disposal that arrives while the teardown is already running adds its values to it.
#[tokio::test]
async fn a_second_disposal_during_a_running_teardown_reaches_that_teardown() {
    let kernel = Kernel::start();
    let owner = kernel.root().child("owner").unwrap();
    let started = Arc::new(Notify::new());
    let seen = Seen::default();
    {
        let (seen, own, started) = (seen.clone(), owner.clone(), started.clone());
        let registration = capture_caller(&owner);
        owner
            .try_effect::<KernelError>("waits-for-the-second-disposal", move |undos| {
                // Undone last: the capture has to outlive the wait below.
                undos.push(Undo::from(registration))?;
                undos.push(Undo::future(move || async move {
                    let first = texts(own.dispose_captures::<String>());
                    started.notify_one();
                    let both = texts(own.next_dispose_captures::<String>(first.len()).await);
                    seen.lock().unwrap().push(("owner", [first, both].concat()));
                    Ok(())
                }))
            })
            .unwrap();
    }
    let fiber = owner.fiber();
    let first = tokio::spawn({
        let fiber = fiber.clone();
        CALLER.scope("first".to_owned(), async move { fiber.dispose().await })
    });
    within(started.notified()).await;
    within(CALLER.scope("second".to_owned(), async { fiber.dispose().await })).await;
    within(first).await.unwrap();
    assert_eq!(
        sorted(&seen),
        [(
            "owner",
            ["first", "first", "second"].map(str::to_owned).to_vec()
        )]
    );
    within(kernel.shutdown()).await;
}

#[tokio::test]
async fn shutdown_hands_the_callers_value_to_every_teardown() {
    let kernel = Kernel::start();
    let owner = kernel.root().child("owner").unwrap();
    capture_caller(&owner);
    let seen = Seen::default();
    probe(&owner, "owner", &seen);

    // `shutdown()` captures when it is called, in the calling task.
    let down = CALLER.sync_scope("down".to_owned(), || kernel.shutdown());
    within(down).await;
    assert_eq!(sorted(&seen), [("owner", vec!["down".to_owned()])]);
}

#[tokio::test]
async fn a_disposer_hands_the_callers_value_on_to_the_child_disposal_it_runs() {
    let kernel = Kernel::start();
    let owner = kernel.root().child("owner").unwrap();
    capture_caller(&owner);
    let seen = Seen::default();
    probe(&owner, "owner", &seen);

    // The child's `dispose()` runs in a cleanup task of the disposer, not in the caller's task.
    let disposer = owner.fiber().disposer().unwrap();
    CALLER
        .scope("via-disposer".to_owned(), async {
            disposer.dispose().await
        })
        .await;
    assert_eq!(sorted(&seen), [("owner", vec!["via-disposer".to_owned()])]);
    kernel.shutdown().await;
}

#[tokio::test]
async fn only_the_disposed_subtree_sees_the_value_not_a_sibling_or_another_kernel() {
    let (kernel, other_kernel) = (Kernel::start(), Kernel::start());
    let (disposed, sibling) = (
        kernel.root().child("disposed").unwrap(),
        kernel.root().child("sibling").unwrap(),
    );
    // Both register, both are probed; only the one that is disposed may see the value.
    capture_caller(&disposed);
    capture_caller(&sibling);
    let seen = Seen::default();
    probe(&disposed, "disposed", &seen);
    probe(&sibling, "sibling", &seen);
    // A teardown of the first kernel that reads through a context of the second one.
    let foreign = other_kernel.root();
    {
        let (seen, foreign) = (seen.clone(), foreign.clone());
        disposed
            .effect("foreign-reader", move || {
                Undo::future(move || async move {
                    seen.lock()
                        .unwrap()
                        .push(("foreign", texts(foreign.dispose_captures::<String>())));
                    Ok(())
                })
            })
            .unwrap();
    }

    CALLER
        .scope("only-here".to_owned(), async {
            disposed.fiber().dispose().await
        })
        .await;
    sibling.fiber().dispose().await;
    assert_eq!(
        sorted(&seen),
        [
            ("disposed", vec!["only-here".to_owned()]),
            ("foreign", vec![]),
            ("sibling", vec![])
        ]
    );
    kernel.shutdown().await;
    other_kernel.shutdown().await;
}

#[tokio::test]
async fn a_panicking_capture_function_neither_breaks_the_disposal_nor_hides_the_others() {
    let kernel = Kernel::start();
    let owner = kernel.root().child("owner").unwrap();
    let warned = warnings(&owner);
    owner
        .capture_on_dispose::<String>(|| panic!("capture bug"))
        .unwrap();
    capture_caller(&owner);
    let seen = Seen::default();
    probe(&owner, "owner", &seen);

    CALLER
        .scope("survivor".to_owned(), async {
            owner.fiber().dispose().await
        })
        .await;
    assert_eq!(sorted(&seen), [("owner", vec!["survivor".to_owned()])]);
    {
        let warned = warned.lock().unwrap();
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(warned[0].contains("capture function panicked"));
    }
    kernel.shutdown().await;
}

/// The disposer applies only the capture functions of the subtree it unloads: a function of a
/// sibling is not run for it, and the sibling's own teardown never sees the value.
#[tokio::test]
async fn a_disposer_runs_only_the_capture_functions_of_the_subtree_it_unloads() {
    let kernel = Kernel::start();
    let (unloaded, sibling) = (
        kernel.root().child("unloaded").unwrap(),
        kernel.root().child("sibling").unwrap(),
    );
    capture_caller(&sibling);
    let seen = Seen::default();
    probe(&unloaded, "unloaded", &seen);
    probe(&sibling, "sibling", &seen);

    let disposer = unloaded.fiber().disposer().unwrap();
    within(CALLER.scope("x".to_owned(), async { disposer.dispose().await })).await;
    within(sibling.fiber().dispose()).await;
    assert_eq!(sorted(&seen), [("sibling", vec![]), ("unloaded", vec![])]);
    within(kernel.shutdown()).await;
}

/// The disposer of the source is single-shot (`fiber.ts`: a repeat call returns at once): the
/// cleanup of a fiber that its parent-owned effect is already unloading may call it.
#[tokio::test]
async fn a_repeat_call_of_the_disposer_from_the_teardown_it_waits_on_returns_at_once() {
    let kernel = Kernel::start();
    let child = kernel
        .root()
        .child("parent")
        .unwrap()
        .child("child")
        .unwrap();
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (own, reached) = (child.fiber(), reached.clone());
        child
            .effect("reenters", move || {
                Undo::future(move || async move {
                    own.disposer().unwrap().dispose().await;
                    reached.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                })
            })
            .unwrap();
    }
    within(child.fiber().disposer().unwrap().dispose()).await;
    assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
    within(kernel.shutdown()).await;
}

/// `Kernel::shutdown()` from a task of the kernel itself sends the command and does not wait for
/// the teardown that waits for it (the rule `Fiber::dispose()` of the root follows).
#[tokio::test]
async fn shutdown_called_from_a_cleanup_of_its_own_kernel_does_not_wait_for_itself() {
    let kernel = Kernel::start();
    let owner = kernel.root().child("owner").unwrap();
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (own, reached) = (kernel.clone(), reached.clone());
        owner
            .effect("shuts-down", move || {
                Undo::future(move || async move {
                    own.shutdown().await;
                    reached.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                })
            })
            .unwrap();
    }
    within(owner.fiber().dispose()).await;
    assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
    within(kernel.shutdown()).await;
}
