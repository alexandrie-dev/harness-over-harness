mod support;

use futures_util::future::BoxFuture;
use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Plugin, Undo};
use hoh_llm::Llm;
use hoh_sdk_jsonrpc_server::command_helpers::BuiltinCommandName;
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
    /// Ordered record of everything observable: `frame:<id>`, `flush`, `root-disposed`, `exit`.
    events: Vec<String>,
    output_errors: Vec<String>,
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
        {
            let mut state = lock(&self.state);
            state.bytes.extend_from_slice(bytes);
            for line in String::from_utf8_lossy(bytes).lines() {
                if let Ok(frame) = serde_json::from_str::<Value>(line)
                    && let Some(id) = frame["id"].as_str()
                {
                    state.events.push(format!("frame:{id}"));
                }
            }
        }
        self.changed.notify_waiters();
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        {
            let mut state = lock(&self.state);
            state.flushes += 1;
            state.events.push("flush".into());
            if self.fail_flush {
                state.output_errors.push("flush callback failed".into());
            }
        }
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
        let exit_state = capture.state.clone();
        let root_state = capture.state.clone();
        ctx.root()
            .effect("test.root-disposed", move || {
                Undo::sync(move || lock(&root_state).events.push("root-disposed".into()))
            })
            .unwrap();
        ctx.provide::<JsonRpcRuntime>(Arc::new(JsonRpcRuntimeHooks::new(
            Box::new(BufReader::new(server_input)),
            Box::new(capture.clone()),
            move |code| {
                lock(&observed_exits).push(code);
                lock(&exit_state).events.push("exit".into());
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
async fn runs_the_window_scenario_over_stdio_without_echoing_the_approval_key() {
    const KEY: &str = "4242424242424242424242424242424242424242424242424242424242424242";
    let mut harness = Harness::new(false).await;
    support::mount::<hoh_user_approval::ApprovalService>(&harness.ctx, json!({})).await;
    assert!(
        harness
            .ctx
            .get::<hoh_sdk_jsonrpc_server::HohRpcMethods>()
            .is_some()
    );
    let init = json!({"cwd":harness._dir.path(),"provider":"mock","model":"apply-model",
        "capabilities":{"approvals":"host"},"hoh":{"protocolVersion":1,"approvalKey":KEY}});
    harness
        .send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":init}))
        .await;
    assert_eq!(
        harness.wait_for(|frame| frame["id"] == 1).await["result"]["hoh"],
        json!({"protocolVersion":1,"approvals":"host"})
    );
    harness
        .send(json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":init}))
        .await;
    assert!(harness.wait_for(|frame| frame["id"] == 2).await["error"].is_object());
    harness.send(json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"main","contentBlocks":[{"type":"text","text":"hi window"}]}})).await;
    harness.wait_for(|frame| frame["id"] == 3).await;
    harness
        .wait_for(|frame| {
            frame["method"] == "session.status" && frame["params"]["status"] == "idle"
        })
        .await;
    harness
        .send(json!({"jsonrpc":"2.0","id":4,"method":"session/list","params":{}}))
        .await;
    let list = harness.wait_for(|frame| frame["id"] == 4).await;
    assert_eq!(list["result"]["sessions"][0]["id"], "main");
    assert_eq!(list["result"]["sessions"][0]["title"], "hi window");
    harness.send(json!({"jsonrpc":"2.0","id":5,"method":"session/events","params":{"sessionId":"main","limit":1000}})).await;
    let events = harness.wait_for(|frame| frame["id"] == 5).await;
    assert!(events["result"]["events"].as_array().unwrap().len() > 1);
    harness
        .send(
            json!({"jsonrpc":"2.0","id":6,"method":"session/cancel","params":{"sessionId":"main"}}),
        )
        .await;
    assert_eq!(
        harness.wait_for(|frame| frame["id"] == 6).await["result"],
        json!({})
    );
    harness
        .send(json!({"jsonrpc":"2.0","id":7,"method":"shutdown"}))
        .await;
    harness.wait_for_exit().await;
    assert_eq!(*lock(&harness.exits), vec![0]);
    let wire = String::from_utf8(lock(&harness.capture.state).bytes.clone()).unwrap();
    assert!(!wire.contains(KEY));
    harness.dispose().await;
}

impl Harness {
    fn events(&self) -> Vec<String> {
        lock(&self.capture.state).events.clone()
    }
    /// Input sent after the process exited must produce no frame at all.
    async fn assert_silent_after_exit(&mut self) {
        let before = self.frames().len();
        let _ = self
            .input
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":\"after-exit\",\"method\":\"initialize\",\"params\":{\"cwd\":\"/\",\"provider\":\"mock\",\"model\":\"x\"}}\n",
            )
            .await;
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(self.frames().len(), before);
    }
}

#[tokio::test]
async fn answers_racing_shutdowns_before_disposal_and_exits_zero_once() {
    let mut harness = Harness::new(false).await;
    harness.send_raw("{\"jsonrpc\":\"2.0\",\"id\":\"sd-1\",\"method\":\"shutdown\"}\n{\"jsonrpc\":\"2.0\",\"id\":\"sd-2\",\"method\":\"shutdown\"}\n").await;
    harness.wait_for_exit().await;
    assert_eq!(*lock(&harness.exits), vec![0]);
    assert!(harness.ctx.ensure_active().is_err());
    // plugin-apply.spec.ts:302-326: both answers, then the flush barrier, then the root
    // disposal, then exit.
    let events = harness.events();
    let at = |name: &str| events.iter().position(|e| e == name);
    let (first, second) = (at("frame:sd-1").unwrap(), at("frame:sd-2").unwrap());
    let flush = at("flush").expect("flush barrier");
    let root = at("root-disposed").expect("root disposed");
    let exit = at("exit").expect("exit");
    assert!(flush > first && flush > second, "{events:?}");
    assert!(root > flush, "{events:?}");
    assert!(exit > root, "{events:?}");
    assert_eq!(events.iter().filter(|e| *e == "root-disposed").count(), 1);
    assert_eq!(events.iter().filter(|e| *e == "exit").count(), 1);
    harness.assert_silent_after_exit().await;
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
    // plugin-apply.spec.ts:341-348: one root disposal, the one flush failure, then silence.
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(*lock(&harness.exits), vec![0]);
    let events = harness.events();
    assert_eq!(events.iter().filter(|e| *e == "root-disposed").count(), 1);
    assert_eq!(
        lock(&harness.capture.state).output_errors,
        vec!["flush callback failed".to_string()]
    );
    harness.assert_silent_after_exit().await;
    harness.dispose().await;
}

/// transport.ts:238-243 / index.ts:97-102: unloading only stops reading; a request that is
/// still running is answered. An abort-waiting command is released by the unload itself.
#[tokio::test]
async fn answers_a_running_request_when_the_plugin_is_unloaded() {
    let dir = tempfile::tempdir().unwrap();
    let (kernel, ctx, _, _, _tool_executor) = base(&dir, None).await;
    let registry = support::commands::Registry::mount_on(&ctx).await;
    let mut harness = Harness::attach(dir, kernel, ctx, false).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let signalled = entered.clone();
    registry.handler(Arc::new(move |_, _, _, signal| {
        let signalled = signalled.clone();
        Box::pin(async move {
            signalled.notify_one();
            let reason = signal.cancelled().await;
            Err(reason.into())
        })
    }));
    registry.register("plan", Some(BuiltinCommandName::Plan));
    harness.send(json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"cwd":harness._dir.path(),"provider":"mock","model":"apply-model"}})).await;
    harness.wait_for(|frame| frame["id"] == "init").await;
    harness.send(json!({"jsonrpc":"2.0","id":"q","method":"session/query","params":{"sessionId":"main","contentBlocks":[{"type":"text","text":"/plan"}]}})).await;
    entered.notified().await;
    harness.fiber.clone().dispose().await;
    let frames = harness.frames();
    let answer = frames
        .iter()
        .find(|frame| frame["id"] == "q")
        .unwrap_or_else(|| panic!("no answer for the running request: {frames:?}"));
    assert!(answer["error"].is_object(), "{answer}");
    harness.dispose().await;
}

/// The window falls back only on `-32601`; unknown `hoh/*` methods must carry exactly that
/// code, while an unknown SDK method keeps the source's `-32603`.
#[tokio::test]
async fn answers_unregistered_hoh_methods_with_method_not_found() {
    let mut harness = Harness::new(false).await;
    harness.send(json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"cwd":harness._dir.path(),"provider":"mock","model":"apply-model"}})).await;
    harness.wait_for(|frame| frame["id"] == "init").await;
    for (id, method) in [
        ("a", "hoh/employee/open"),
        ("b", "hoh/security/summary"),
        ("c", "hoh/not/declared"),
    ] {
        harness
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}}))
            .await;
        let frame = harness.wait_for(|frame| frame["id"] == id).await;
        assert_eq!(frame["error"]["code"], -32601, "{frame}");
        assert_eq!(
            frame["error"]["message"],
            format!("unknown HoH method: {method}")
        );
    }
    harness
        .send(json!({"jsonrpc":"2.0","id":"d","method":"nope/unknown"}))
        .await;
    assert_eq!(
        harness.wait_for(|frame| frame["id"] == "d").await["error"]["code"],
        -32603
    );
    harness.dispose().await;
}

/// transport.ts:266 never rejects a write and `user-questions.ts` cancels an open question on
/// teardown: a plain unload (no `shutdown` request) must still deliver `user-question.cancelled`.
#[tokio::test]
async fn tells_the_host_a_question_is_cancelled_when_the_plugin_is_unloaded() {
    let dir = tempfile::tempdir().unwrap();
    let (kernel, ctx, _, _, _tool_executor) = base(&dir, None).await;
    support::mount::<hoh_user_questions::UserQuestionsPlugin>(&ctx, json!({})).await;
    let mut harness = Harness::attach(dir, kernel, ctx, false).await;
    harness.send(json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"cwd":harness._dir.path(),"provider":"mock","model":"apply-model","capabilities":{"userQuestions":true}}})).await;
    harness.wait_for(|frame| frame["id"] == "init").await;
    harness.send(json!({"jsonrpc":"2.0","id":"open","method":"session/working-directory/get","params":{"sessionId":"main"}})).await;
    harness.wait_for(|frame| frame["id"] == "open").await;
    let agent = harness
        .ctx
        .get::<hoh_agent::Agents>()
        .unwrap()
        .get(&"main".into())
        .unwrap();
    let service = harness
        .ctx
        .get::<hoh_user_questions::UserQuestions>()
        .unwrap();
    let asking = tokio::spawn(async move {
        service
            .ask(hoh_user_questions::AskUserQuestionRequest {
                questions: serde_json::from_value(json!([{"id":"q1","question":"Which?","options":[{"label":"A"},{"label":"B"}]}])).unwrap(),
                agent: Some(agent),
                signal: None,
                wait: None,
            })
            .await
    });
    let ask = harness
        .wait_for(|frame| frame["method"] == "user-question/ask")
        .await;
    let request_id = ask["params"]["requestId"].clone();
    harness.fiber.clone().dispose().await;
    let frames = harness.frames();
    let cancelled = frames
        .iter()
        .find(|frame| frame["method"] == "user-question.cancelled")
        .unwrap_or_else(|| panic!("host was not told the question is cancelled: {frames:?}"));
    assert_eq!(cancelled["params"]["requestId"], request_id);
    assert_eq!(cancelled["params"]["sessionId"], "main");
    assert!(lock(&harness.exits).is_empty());
    assert!(asking.await.unwrap().is_err());
    harness.dispose().await;
}

#[tokio::test]
async fn stops_serving_on_bare_fiber_dispose_without_process_exit() {
    let mut harness = Harness::new(false).await;
    harness
        .send(json!({"jsonrpc":"2.0","id":"probe-1","method":"nope/unknown"}))
        .await;
    let error = harness.wait_for(|frame| frame["id"] == "probe-1").await;
    assert_eq!(error["error"]["code"], -32603);
    assert_eq!(
        error["error"]["message"],
        "unknown DeepSeek Harness SDK runtime method: nope/unknown"
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
