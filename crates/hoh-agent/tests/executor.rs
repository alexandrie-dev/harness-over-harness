//! HOH-18 seam (no source counterpart): named session executors beside the loop's factory.
mod common;

use common::*;
use futures_util::future::BoxFuture;
use hoh_agent::{
    AgentFactory, AgentHandle, AgentOptions, AgentSessionMeta, Agents, CreateAgentOptions,
    EXECUTOR_BINDING_RECORD, ExecutorSelection, Live, ResumeAgentOptions, SessionExecutor,
};
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_session::{SessionEvent, SessionHeader, SessionId};
use hoh_session_persistence::SessionPersistenceService;
use hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Records which entry point was called, and claims the sessions named in `owned`.
struct Recorder {
    name: &'static str,
    owned: Vec<&'static str>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl AgentFactory for Recorder {
    fn create_agent(
        &self,
        owner_ctx: Context,
        options: CreateAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        let id = options.session_id.expect("session id").to_string();
        // The route it was given, when it names a model (the selection does).
        let model = options
            .agent_options
            .as_ref()
            .and_then(AgentOptions::model)
            .map(|model| format!("@{model}"))
            .unwrap_or_default();
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}:create:{id}{model}", self.name));
        let agent = stub_agent(&owner_ctx, &id);
        Box::pin(async move { Ok(AgentHandle::new(agent, || Box::pin(async { Ok(()) }))) })
    }
    fn resume(
        &self,
        owner_ctx: Context,
        options: ResumeAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        let id = options.resume_session_id.to_string();
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}:resume:{id}", self.name));
        let agent = stub_agent(&owner_ctx, &id);
        Box::pin(async move { Ok(AgentHandle::new(agent, || Box::pin(async { Ok(()) }))) })
    }
}

impl SessionExecutor for Recorder {
    fn claims(
        &self,
        _owner_ctx: &Context,
        id: &SessionId,
    ) -> BoxFuture<'static, Result<bool, BoxError>> {
        let claimed = self.owned.contains(&id.as_str());
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}:claims:{id}", self.name));
        Box::pin(async move { Ok(claimed) })
    }
}

fn recorder(
    name: &'static str,
    owned: Vec<&'static str>,
    calls: &Arc<Mutex<Vec<String>>>,
) -> Arc<Recorder> {
    Arc::new(Recorder {
        name,
        owned,
        calls: calls.clone(),
    })
}

fn create_options(id: &str, provider: Option<&str>) -> CreateAgentOptions {
    let mut options = CreateAgentOptions::new(SessionId::from(id));
    options.agent_options = Some(AgentOptions {
        provider: provider.map(|p| Live::Value(p.to_owned())),
        ..AgentOptions::default()
    });
    options
}

#[tokio::test]
async fn create_routes_a_provider_that_names_an_executor_and_keeps_the_loop_for_others() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    agents
        .set_factory(&h.ctx, recorder("loop", vec![], &calls))
        .unwrap();
    agents
        .set_executor(&h.ctx, "claude", recorder("claude", vec![], &calls))
        .unwrap();

    agents
        .create(&h.ctx, create_options("a", Some("claude")))
        .await
        .unwrap();
    agents
        .create(&h.ctx, create_options("b", Some("deepseek")))
        .await
        .unwrap();
    agents
        .create(&h.ctx, CreateAgentOptions::new(SessionId::from("c")))
        .await
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        ["claude:create:a", "loop:create:b", "loop:create:c"]
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn resume_asks_executors_in_order_and_falls_back_to_the_loop() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    agents
        .set_factory(&h.ctx, recorder("loop", vec![], &calls))
        .unwrap();
    agents
        .set_executor(&h.ctx, "codex", recorder("codex", vec!["x"], &calls))
        .unwrap();
    agents
        .set_executor(&h.ctx, "claude", recorder("claude", vec!["y"], &calls))
        .unwrap();

    agents
        .resume(&h.ctx, ResumeAgentOptions::new(SessionId::from("y")))
        .await
        .unwrap();
    agents
        .resume(&h.ctx, ResumeAgentOptions::new(SessionId::from("z")))
        .await
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "codex:claims:y",
            "claude:claims:y",
            "claude:resume:y",
            "codex:claims:z",
            "claude:claims:z",
            "loop:resume:z",
        ]
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn an_executor_session_needs_no_loop_factory_but_others_still_do() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    agents
        .set_executor(&h.ctx, "claude", recorder("claude", vec!["y"], &calls))
        .unwrap();
    agents
        .create(&h.ctx, create_options("a", Some("claude")))
        .await
        .unwrap();
    agents
        .resume(&h.ctx, ResumeAgentOptions::new(SessionId::from("y")))
        .await
        .unwrap();
    let error = agents
        .resume(&h.ctx, ResumeAgentOptions::new(SessionId::from("z")))
        .await
        .err()
        .expect("no factory");
    assert_eq!(error.to_string(), hoh_agent::NO_FACTORY_MESSAGE);
    let error = agents
        .create(&h.ctx, create_options("b", None))
        .await
        .err()
        .expect("no factory");
    assert_eq!(error.to_string(), hoh_agent::NO_FACTORY_MESSAGE);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn executor_names_are_unique_and_the_disposer_removes_the_executor() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let disposer = agents
        .set_executor(&h.ctx, "claude", recorder("claude", vec![], &calls))
        .unwrap();
    let error = agents
        .set_executor(&h.ctx, "claude", recorder("other", vec![], &calls))
        .err()
        .expect("duplicate name");
    assert_eq!(
        error.to_string(),
        "an agent executor named \"claude\" is already registered"
    );
    assert_eq!(agents.executor_names(), ["claude"]);
    disposer.dispose().await;
    assert!(agents.executor_names().is_empty());
    assert!(agents.executor("claude").is_none());
    h.kernel.shutdown().await;
}

/// Mount a JSONL store under `root` and write session `id` to it, bound to `harness` when given.
async fn store_session(h: &Harness, root: &std::path::Path, id: &str, harness: Option<&str>) {
    let binding = harness.map(|harness| {
        json!({"type": EXECUTOR_BINDING_RECORD, "data": {"harness": harness, "nativeSessionId": null}})
    });
    store_events(h, root, id, binding.into_iter().collect()).await;
}

/// Mount a JSONL store under `root` and write session `id` with `events` (`{type, data}`).
async fn store_events(
    h: &Harness,
    root: &std::path::Path,
    id: &str,
    events: Vec<serde_json::Value>,
) {
    if h.ctx.get::<SessionPersistenceService>().is_none() {
        h.ctx
            .plugin::<JsonlSessionPersistencePlugin>(
                json!({"root": root.to_str().unwrap(), "compression": "none"}),
            )
            .unwrap()
            .settled()
            .await
            .unwrap();
    }
    let persistence = h.ctx.get::<SessionPersistenceService>().unwrap();
    let handle = persistence
        .create(&SessionHeader::new(SessionId::from(id), 1), None)
        .await
        .unwrap();
    let events: Vec<Arc<SessionEvent>> = events
        .into_iter()
        .enumerate()
        .map(|(seq, event)| {
            Arc::new(SessionEvent::from_value(json!({
                "type": event["type"], "seq": seq, "time": 1, "ignorable": true,
                "data": event["data"]})))
        })
        .collect();
    if !events.is_empty() {
        handle.append(&events, None).await.unwrap();
    }
    handle.flush(None).await.unwrap();
    handle.close().await.unwrap();
}

#[tokio::test]
async fn resume_refuses_a_session_bound_to_an_external_harness_that_is_not_loaded() {
    std::fs::create_dir_all("/dev/shm/hoh").unwrap();
    let root = tempfile::Builder::new()
        .prefix("o1-agent-store-")
        .tempdir_in("/dev/shm/hoh")
        .unwrap();
    let h = mount().await;
    store_session(&h, root.path(), "bound", Some("claude")).await;
    store_session(&h, root.path(), "plain", None).await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    agents
        .set_factory(&h.ctx, recorder("loop", vec![], &calls))
        .unwrap();

    // No executor at all: the loop does not pick up the log the harness wrote.
    let error = agents
        .resume(&h.ctx, ResumeAgentOptions::new(SessionId::from("bound")))
        .await
        .err()
        .expect("refused");
    assert_eq!(
        error.to_string(),
        "session belongs to external harness claude"
    );
    // Another harness is loaded, but not the one the session belongs to.
    agents
        .set_executor(&h.ctx, "codex", recorder("codex", vec![], &calls))
        .unwrap();
    let error = agents
        .resume(&h.ctx, ResumeAgentOptions::new(SessionId::from("bound")))
        .await
        .err()
        .expect("refused");
    assert_eq!(
        error.to_string(),
        "session belongs to external harness claude"
    );
    // A stored session without a binding is still the loop's.
    agents
        .resume(&h.ctx, ResumeAgentOptions::new(SessionId::from("plain")))
        .await
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "codex:claims:bound",
            "codex:claims:plain",
            "loop:resume:plain"
        ]
    );
    h.kernel.shutdown().await;
}

/// Every file under `dir`.
fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// Review 232500, item 3: a stored log the store fails to read may be bound to a harness, so the
/// loop does not get it; the resume is refused (fail closed) instead of treated as unbound.
#[tokio::test]
async fn a_log_that_cannot_be_read_is_not_handed_to_the_loop() {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all("/dev/shm/hoh").unwrap();
    let root = tempfile::Builder::new()
        .prefix("o1-agent-store-")
        .tempdir_in("/dev/shm/hoh")
        .unwrap();
    let h = mount().await;
    store_session(&h, root.path(), "bound", Some("claude")).await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    agents
        .set_factory(&h.ctx, recorder("loop", vec![], &calls))
        .unwrap();
    let files = files_under(root.path());
    assert!(!files.is_empty());
    let mode = |file: &std::path::Path, mode: u32| {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    for file in &files {
        mode(file, 0o000);
    }
    let error = agents
        .resume(&h.ctx, ResumeAgentOptions::new(SessionId::from("bound")))
        .await
        .err()
        .expect("refused");
    for file in &files {
        mode(file, 0o600);
    }
    // The store's own error, as the loop's resume would report it.
    assert!(
        error
            .to_string()
            .starts_with("EACCES: permission denied, open "),
        "{error}"
    );
    assert!(
        calls.lock().unwrap().is_empty(),
        "{:?}",
        calls.lock().unwrap()
    );
    // Readable again, it is refused for the harness it belongs to, as before.
    let error = agents
        .resume(&h.ctx, ResumeAgentOptions::new(SessionId::from("bound")))
        .await
        .err()
        .expect("refused");
    assert_eq!(
        error.to_string(),
        "session belongs to external harness claude"
    );
    h.kernel.shutdown().await;
}

/// Review 221315, item 5: listeners of `session/created` may write any number of records before
/// the binding; the session is still bound. A binding after the first turn binds nothing.
#[tokio::test]
async fn a_binding_after_many_leading_records_still_binds_the_session() {
    std::fs::create_dir_all("/dev/shm/hoh").unwrap();
    let root = tempfile::Builder::new()
        .prefix("o1-agent-store-")
        .tempdir_in("/dev/shm/hoh")
        .unwrap();
    let h = mount().await;
    let binding = json!({"type": EXECUTOR_BINDING_RECORD, "data": {"harness": "claude", "nativeSessionId": null}});
    let mut leading: Vec<serde_json::Value> = (0..40)
        .map(|n| json!({"type": "plugin:other/x", "data": {"n": n}}))
        .collect();
    leading.push(binding.clone());
    store_events(&h, root.path(), "late-bound", leading).await;
    store_events(
        &h,
        root.path(),
        "turn-first",
        vec![json!({"type": "turn/start", "data": {"turn": 1}}), binding],
    )
    .await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    agents
        .set_factory(&h.ctx, recorder("loop", vec![], &calls))
        .unwrap();
    let error = agents
        .resume(
            &h.ctx,
            ResumeAgentOptions::new(SessionId::from("late-bound")),
        )
        .await
        .err()
        .expect("refused");
    assert_eq!(
        error.to_string(),
        "session belongs to external harness claude"
    );
    agents
        .resume(
            &h.ctx,
            ResumeAgentOptions::new(SessionId::from("turn-first")),
        )
        .await
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), ["loop:resume:turn-first"]);
    h.kernel.shutdown().await;
}

/// HOH-18, CEO 20.11: the executor the user selected in the window takes new top-level chats with
/// its model, whatever route they were given; subagents, forks and the rest stay with the loop.
#[tokio::test]
async fn the_selected_executor_takes_new_chats_and_leaves_subagents_to_the_loop() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    agents
        .set_factory(&h.ctx, recorder("loop", vec![], &calls))
        .unwrap();
    agents
        .set_executor(&h.ctx, "claude", recorder("claude", vec![], &calls))
        .unwrap();
    assert_eq!(agents.selected_executor(), None);
    agents
        .create(&h.ctx, create_options("before", Some("deepseek")))
        .await
        .unwrap();
    let selection = ExecutorSelection {
        executor: "claude".into(),
        model: "opus".into(),
    };
    agents.select_executor(Some(selection.clone()));
    assert_eq!(agents.selected_executor(), Some(selection));
    agents
        .create(&h.ctx, create_options("chat", Some("deepseek")))
        .await
        .unwrap();
    agents
        .create(&h.ctx, CreateAgentOptions::new(SessionId::from("bare")))
        .await
        .unwrap();
    let mut subagent = create_options("sub", Some("deepseek"));
    subagent.meta = Some(AgentSessionMeta {
        parent_session: Some(SessionId::from("chat")),
        origin: Some("subagent".into()),
        ..AgentSessionMeta::default()
    });
    agents.create(&h.ctx, subagent).await.unwrap();
    // A selected executor that is not loaded is an error, not a silent fallback.
    agents.select_executor(Some(ExecutorSelection {
        executor: "codex".into(),
        model: "m".into(),
    }));
    let error = agents
        .create(&h.ctx, create_options("lost", Some("deepseek")))
        .await
        .err()
        .expect("refused");
    assert_eq!(
        error.to_string(),
        "the executor \"codex\" chosen for new chats is not loaded"
    );
    agents.select_executor(None);
    agents
        .create(&h.ctx, create_options("after", Some("deepseek")))
        .await
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "loop:create:before",
            "claude:create:chat@opus",
            "claude:create:bare@opus",
            "loop:create:sub",
            "loop:create:after"
        ]
    );
    h.kernel.shutdown().await;
}
