use hoh_commander::Output;
use hoh_kernel::{Context, Fiber, FiberState, Kernel};
use hoh_loader::cmdline::{
    AppExit, AppReady, AppStdin, CmdlineHost, Listener, Subscription, provide_cmdline,
};
use hoh_sdk_app::{SdkApp, SdkAppRuntimeHooks, SdkAppRuntimeKey, SdkAppStartupKey};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct StdinState {
    ended: bool,
    listeners: Vec<Listener>,
}

#[derive(Default)]
struct TestStdin(Arc<Mutex<StdinState>>);

impl AppStdin for TestStdin {
    fn readable_ended(&self) -> bool {
        lock(&self.0).ended
    }

    fn once_end(&self, listener: Listener) -> Subscription {
        lock(&self.0).listeners.push(listener.clone());
        let state = self.0.clone();
        Subscription::new(move || {
            lock(&state)
                .listeners
                .retain(|candidate| !Arc::ptr_eq(candidate, &listener));
        })
    }
}

impl TestStdin {
    fn end(&self) {
        let listeners = {
            let mut state = lock(&self.0);
            state.ended = true;
            std::mem::take(&mut state.listeners)
        };
        for listener in listeners {
            listener();
        }
    }
}

struct Fixture {
    kernel: Kernel,
    ctx: Context,
    _fiber: Fiber,
    exits: Arc<Mutex<Vec<i32>>>,
    out: Arc<Mutex<String>>,
    stdin: Arc<TestStdin>,
}

async fn start(args: &[&str], config: Value, runtime: bool) -> Fixture {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let exits = Arc::new(Mutex::new(Vec::new()));
    let observed_exits = exits.clone();
    provide_cmdline(
        &ctx,
        CmdlineHost {
            args: args.iter().map(|arg| (*arg).into()).collect(),
            exit: AppExit::new(move |code| lock(&observed_exits).push(code)),
            ready: Some(AppReady::new(|listener| {
                listener();
                Subscription::new(|| {})
            })),
        },
    )
    .await
    .unwrap();

    let out = Arc::new(Mutex::new(String::new()));
    let stdout = out.clone();
    let stderr = out.clone();
    let stdin = Arc::new(TestStdin::default());
    if runtime {
        ctx.provide::<SdkAppRuntimeKey>(Arc::new(SdkAppRuntimeHooks::new(
            stdin.clone(),
            Output::new(
                move |text| lock(&stdout).push_str(text),
                move |text| lock(&stderr).push_str(text),
            ),
        )))
        .await
        .unwrap();
    }
    let fiber = ctx.plugin::<SdkApp>(config).unwrap();
    fiber.settled().await.unwrap();
    Fixture {
        kernel,
        ctx,
        _fiber: fiber,
        exits,
        out,
        stdin,
    }
}

#[tokio::test]
async fn publishes_readiness_and_requests_bounded_exit_on_client_eof() {
    let fixture = start(&[], json!({}), true).await;
    assert_eq!(
        fixture.ctx.get::<SdkAppStartupKey>().as_deref(),
        Some(&hoh_sdk_app::SdkAppStartup { accepted: true })
    );
    fixture.stdin.end();
    assert_eq!(*lock(&fixture.exits), [0]);
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn prints_app_help_without_publishing_readiness_or_binding_stdin() {
    let fixture = start(&["--help"], json!({}), true).await;
    assert!(lock(&fixture.out).contains("dsh --profile sdk"));
    assert!(fixture.ctx.get::<SdkAppStartupKey>().is_none());
    assert_eq!(*lock(&fixture.exits), [0]);
    fixture.stdin.end();
    assert_eq!(*lock(&fixture.exits), [0]);
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn renders_the_selected_sdk_profile_name_in_help() {
    let fixture = start(&["--help"], json!({ "profile": "sdk-minimal" }), true).await;
    {
        let out = lock(&fixture.out);
        assert!(out.contains("Usage: dsh --profile sdk-minimal"));
        assert!(out.contains("dsh --profile sdk-minimal"));
    }
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn mounts_with_the_temporary_non_consuming_stdio_glue() {
    let fixture = start(&[], json!({}), false).await;
    assert!(fixture.ctx.get::<SdkAppStartupKey>().unwrap().accepted);
    assert!(lock(&fixture.exits).is_empty());
    fixture.kernel.shutdown().await;
}

// Schemastery replaces an explicit null (`profile:` or `profile: ~` in YAML) with the default.
#[tokio::test]
async fn treats_a_null_profile_like_an_absent_one_in_help() {
    let fixture = start(&["--help"], json!({ "profile": null }), true).await;
    assert!(lock(&fixture.out).contains("Usage: dsh --profile sdk [options]"));
    assert_eq!(*lock(&fixture.exits), [0]);
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn publishes_readiness_for_a_null_profile() {
    let fixture = start(&[], json!({ "profile": null }), true).await;
    assert!(fixture.ctx.get::<SdkAppStartupKey>().unwrap().accepted);
    fixture.kernel.shutdown().await;
}

// SHIM(hoh-kernel): the schema is `string | null`, so the kernel words the refusal differently from
// schemastery (`expected string but got 5`). Drop this expectation together with the shim.
#[tokio::test]
async fn refuses_a_profile_of_the_wrong_type_with_the_shim_wording() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<SdkApp>(json!({ "profile": 5 })).unwrap();
    fiber.settled().await.unwrap();
    // The plugin is not mounted, and its configuration is refused with the kernel's wording.
    assert_eq!(fiber.state(), FiberState::Pending);
    assert!(fiber.effective_config().is_none());
    let refusal = fiber
        .resolve_config(&json!({ "profile": 5 }))
        .unwrap_err()
        .to_string();
    assert!(
        refusal.contains("expected string | null but got 5 (at profile)"),
        "{refusal}"
    );
    kernel.shutdown().await;
}
