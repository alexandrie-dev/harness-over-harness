mod support;

use futures_util::future::BoxFuture;
use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Plugin};
use hoh_llm::Llm;
use hoh_sdk_jsonrpc_server::{
    JsonRpcRuntime, JsonRpcRuntimeHooks, SdkJsonRpcServer,
    integration::{Subagents, SubagentsApi},
};
use serde_json::{Value, json};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll},
};
use support::{Replay, base, lock};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader, DuplexStream};

#[derive(Default)]
struct CaptureState {
    bytes: Vec<u8>,
    flushes: usize,
}

struct DelayedGate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    model: Arc<Replay>,
}
hoh_kernel::service!(DelayedGateKey: DelayedGate = "sdkDelayedAdapterGate");

struct DelayedAdapter;
impl Plugin for DelayedAdapter {
    const NAME: &'static str = "sdk-test-delayed-adapter";
    type Config = Value;
    type Deps = ();

    async fn apply(ctx: Context, _: (), _: Cfg<Value>) -> Result<(), hoh_kernel::Error> {
        let gate = ctx
            .get::<DelayedGateKey>()
            .ok_or("missing delayed adapter gate")?;
        gate.entered.notify_one();
        gate.release.acquire().await.unwrap().forget();
        ctx.get::<Llm>().ok_or("missing llm")?.register_adapter(
            &ctx,
            vec!["delayed-private".into()],
            gate.model.clone(),
        )?;
        Ok(())
    }
}

#[derive(Clone)]
struct CaptureWriter {
    state: Arc<Mutex<CaptureState>>,
    changed: Arc<tokio::sync::Notify>,
    fail_flush: bool,
}

impl AsyncWrite for CaptureWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        lock(&self.state).bytes.extend_from_slice(bytes);
        self.changed.notify_waiters();
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        lock(&self.state).flushes += 1;
        self.changed.notify_waiters();
        if self.fail_flush {
            Poll::Ready(Err(io::Error::other("flush callback failed")))
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct Harness {
    kernel: hoh_kernel::Kernel,
    ctx: hoh_kernel::Context,
    fiber: hoh_kernel::Fiber,
    input: DuplexStream,
    capture: CaptureWriter,
    exits: Arc<Mutex<Vec<i32>>>,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn new(fail_flush: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (kernel, ctx, _, _, _tool_executor) = base(&dir, None).await;
        Self::attach(dir, kernel, ctx, fail_flush).await
    }

    async fn attach(
        dir: tempfile::TempDir,
        kernel: hoh_kernel::Kernel,
        ctx: hoh_kernel::Context,
        fail_flush: bool,
    ) -> Self {
        Self::attach_with_prefill(dir, kernel, ctx, fail_flush, None).await
    }

    async fn attach_with_prefill(
        dir: tempfile::TempDir,
        kernel: hoh_kernel::Kernel,
        ctx: hoh_kernel::Context,
        fail_flush: bool,
        prefill: Option<&str>,
    ) -> Self {
        let (server_input, mut input) = tokio::io::duplex(64 * 1024);
        if let Some(prefill) = prefill {
            input.write_all(prefill.as_bytes()).await.unwrap();
            input.flush().await.unwrap();
        }
        let capture = CaptureWriter {
            state: Arc::new(Mutex::new(CaptureState::default())),
            changed: Arc::new(tokio::sync::Notify::new()),
            fail_flush,
        };
        let exits = Arc::new(Mutex::new(Vec::new()));
        let observed_exits = exits.clone();
        let exit_changed = capture.changed.clone();
        ctx.provide::<JsonRpcRuntime>(Arc::new(JsonRpcRuntimeHooks::new(
            Box::new(BufReader::new(server_input)),
            Box::new(capture.clone()),
            move |code| {
                lock(&observed_exits).push(code);
                exit_changed.notify_waiters();
            },
        )))
        .await
        .unwrap();
        let fiber = ctx.plugin::<SdkJsonRpcServer>(json!({})).unwrap();
        fiber.settled().await.unwrap();
        Self {
            kernel,
            ctx,
            fiber,
            input,
            capture,
            exits,
            _dir: dir,
        }
    }

    async fn send(&mut self, frame: Value) {
        self.send_raw(&format!("{}\n", hoh_util_values::js_json_stringify(&frame)))
            .await;
    }

    async fn send_raw(&mut self, text: &str) {
        self.input.write_all(text.as_bytes()).await.unwrap();
        self.input.flush().await.unwrap();
    }

    fn frames(&self) -> Vec<Value> {
        let bytes = lock(&self.capture.state).bytes.clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    async fn wait_for(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        loop {
            let changed = self.capture.changed.notified();
            if let Some(frame) = self.frames().into_iter().find(&predicate) {
                return frame;
            }
            changed.await;
        }
    }

    async fn wait_for_exit(&self) {
        loop {
            let changed = self.capture.changed.notified();
            if !lock(&self.exits).is_empty() {
                return;
            }
            changed.await;
        }
    }

    async fn dispose(self) {
        self.fiber.dispose().await;
        self.kernel.shutdown().await;
    }
}

struct FailingDrain;

impl SubagentsApi for FailingDrain {
    fn root_of(&self, _: &Arc<dyn Agent>) -> Result<Arc<dyn Agent>, BoxError> {
        Err("root_of is not used by shutdown".into())
    }

    fn wait_for_tree_work(&self, _: Arc<dyn Agent>) -> BoxFuture<'static, Result<bool, BoxError>> {
        Box::pin(async { Ok(false) })
    }

    fn drain_descendants(
        &self,
        _: Vec<Arc<dyn Agent>>,
    ) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Err("descendant drain failed".into()) })
    }
}

#[tokio::test]
async fn serves_initialize_over_the_injected_stdio_pair() {
    let mut harness = Harness::new(false).await;
    harness.send(json!({"jsonrpc":"2.0","id":"init-1","method":"initialize","params":{"cwd":harness._dir.path(),"provider":"mock","model":"apply-model"}})).await;
    assert_eq!(
        harness.wait_for(|frame| frame["id"] == "init-1").await,
        json!({"jsonrpc":"2.0","id":"init-1","result":{"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"0.0.1"}}})
    );
    assert!(lock(&harness.exits).is_empty());
    harness.dispose().await;
}

#[tokio::test]
async fn handles_prefilled_initialize_only_after_installing_the_request_handler() {
    let dir = tempfile::tempdir().unwrap();
    let (kernel, ctx, _, _, _tool_executor) = base(&dir, None).await;
    let request = format!(
        "{}\n",
        hoh_util_values::js_json_stringify(&json!({
            "jsonrpc":"2.0",
            "id":"prefilled-init",
            "method":"initialize",
            "params":{"cwd":dir.path(),"provider":"mock","model":"apply-model"}
        }))
    );
    let harness = Harness::attach_with_prefill(dir, kernel, ctx, false, Some(&request)).await;
    let response = harness
        .wait_for(|frame| frame["id"] == "prefilled-init")
        .await;
    assert!(response.get("result").is_some(), "{response}");
    harness.dispose().await;
}

#[tokio::test]
async fn waits_for_loader_owned_adapter_registration_before_initialize() {
    let dir = tempfile::tempdir().unwrap();
    let (kernel, ctx, model, _, _tool_executor) = base(&dir, None).await;
    let base_file = dir.path().join("cordis.yml");
    std::fs::write(&base_file, "[]\n").unwrap();
    let gate = Arc::new(DelayedGate {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
        model,
    });
    ctx.provide::<DelayedGateKey>(gate.clone()).await.unwrap();
    let mut registry = hoh_loader::PluginRegistry::default();
    registry.add::<DelayedAdapter>("sdk-test-delayed-adapter");
    let loader = hoh_loader::boot(
        &ctx,
        hoh_loader::BootOptions {
            registry: Arc::new(registry),
            base: base_file,
            layers: Vec::new(),
            env: hoh_loader::EvalEnv {
                home: dir.path().to_owned(),
                cwd: dir.path().to_owned(),
                exe: std::env::current_exe().unwrap(),
                platform: "linux",
                var: Arc::new(|_| None),
            },
        },
    )
    .await
    .unwrap();
    let loading = loader.clone();
    let creation = tokio::spawn(async move {
        loading
            .create(
                hoh_loader::Entry {
                    id: "delayed".into(),
                    name: "sdk-test-delayed-adapter".into(),
                    config: json!({}),
                    disabled: Value::Null,
                    group: false,
                    inject: None,
                    isolate: None,
                    intercept: None,
                },
                Some("include"),
            )
            .await
    });
    gate.entered.notified().await;
    let mut harness = Harness::attach(dir, kernel, ctx, false).await;
    let initialize = json!({"jsonrpc":"2.0","id":"init-delayed","method":"initialize","params":{"cwd":harness._dir.path(),"provider":"delayed-private","model":"apply-model"}});
    let probe = json!({"jsonrpc":"2.0","id":"probe-during-delay","method":"nope/unknown"});
    harness
        .send_raw(&format!(
            "{}\n{}\n",
            hoh_util_values::js_json_stringify(&initialize),
            hoh_util_values::js_json_stringify(&probe)
        ))
        .await;
    harness
        .wait_for(|frame| frame["id"] == "probe-during-delay")
        .await;
    assert!(
        !harness
            .frames()
            .iter()
            .any(|frame| frame["id"] == "init-delayed")
    );
    gate.release.add_permits(1);
    creation.await.unwrap().unwrap();
    assert!(
        harness
            .wait_for(|frame| frame["id"] == "init-delayed")
            .await
            .get("result")
            .is_some()
    );
    harness.dispose().await;
}

#[tokio::test]
async fn drives_a_prompt_turn_end_to_end_and_forwards_session_notifications() {
    let mut harness = Harness::new(false).await;
    harness.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"cwd":harness._dir.path(),"provider":"mock","model":"apply-model"}})).await;
    harness.wait_for(|frame| frame["id"] == 1).await;
    harness.send(json!({"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"main","contentBlocks":[{"type":"text","text":"fix it"}]}})).await;
    assert!(harness.wait_for(|frame| frame["id"] == 2).await["result"]["messageId"].is_string());
    harness
        .wait_for(|frame| {
            frame["method"] == "session.status" && frame["params"]["status"] == "idle"
        })
        .await;
    assert!(
        harness
            .frames()
            .iter()
            .any(|frame| frame["method"] == "session.event")
    );
    harness.dispose().await;
}

#[tokio::test]
async fn answers_racing_shutdowns_before_disposal_and_exits_zero_once() {
    let mut harness = Harness::new(false).await;
    harness.send_raw("{\"jsonrpc\":\"2.0\",\"id\":\"sd-1\",\"method\":\"shutdown\"}\n{\"jsonrpc\":\"2.0\",\"id\":\"sd-2\",\"method\":\"shutdown\"}\n").await;
    harness.wait_for_exit().await;
    let frames = harness.frames();
    assert!(frames.iter().any(|frame| frame["id"] == "sd-1"));
    assert!(frames.iter().any(|frame| frame["id"] == "sd-2"));
    assert_eq!(*lock(&harness.exits), vec![0]);
    assert!(harness.ctx.ensure_active().is_err());
    assert!(lock(&harness.capture.state).flushes > 0);
    harness.dispose().await;
}

#[tokio::test]
async fn keeps_the_process_alive_after_a_failed_shutdown_response() {
    let mut harness = Harness::new(false).await;
    harness.send(json!({"jsonrpc":"2.0","id":"init-failed-shutdown","method":"initialize","params":{"cwd":harness._dir.path(),"provider":"mock","model":"apply-model"}})).await;
    harness
        .wait_for(|frame| frame["id"] == "init-failed-shutdown")
        .await;
    harness.send(json!({"jsonrpc":"2.0","id":"open-before-failed-shutdown","method":"session/working-directory/get","params":{"sessionId":"main"}})).await;
    harness
        .wait_for(|frame| frame["id"] == "open-before-failed-shutdown")
        .await;
    harness
        .ctx
        .provide::<Subagents>(Arc::new(FailingDrain))
        .await
        .unwrap();

    harness
        .send(json!({"jsonrpc":"2.0","id":"failed-shutdown","method":"shutdown"}))
        .await;
    let response = harness
        .wait_for(|frame| frame["id"] == "failed-shutdown")
        .await;
    assert_eq!(response["error"]["message"], "descendant drain failed");
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert!(lock(&harness.exits).is_empty());
    assert!(harness.ctx.ensure_active().is_ok());
    harness.dispose().await;
}

#[tokio::test]
async fn still_disposes_and_exits_once_when_flush_fails() {
    let mut harness = Harness::new(true).await;
    harness
        .send(json!({"jsonrpc":"2.0","id":"sd-fail","method":"shutdown"}))
        .await;
    harness.wait_for_exit().await;
    assert_eq!(*lock(&harness.exits), vec![0]);
    assert!(harness.ctx.ensure_active().is_err());
    assert!(lock(&harness.capture.state).flushes > 0);
    harness.dispose().await;
}

#[tokio::test]
async fn stops_serving_on_bare_fiber_dispose_without_process_exit() {
    let mut harness = Harness::new(false).await;
    harness
        .send(json!({"jsonrpc":"2.0","id":"probe-1","method":"nope/unknown"}))
        .await;
    assert_eq!(
        harness.wait_for(|frame| frame["id"] == "probe-1").await["error"]["code"],
        -32603
    );
    harness.fiber.clone().dispose().await;
    assert!(harness.ctx.ensure_active().is_ok());
    let before = harness.frames().len();
    let _ = harness
        .input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"probe-2\",\"method\":\"initialize\"}\n")
        .await;
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(harness.frames().len(), before);
    assert!(lock(&harness.exits).is_empty());
    harness.dispose().await;
}
