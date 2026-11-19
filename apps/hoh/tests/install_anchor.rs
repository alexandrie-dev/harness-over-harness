//! The launcher provides the installation anchor, so `DSH_INSTALL_DIR` is the installation root
//! (TS `shell-env/src/index.ts:176-180`: `dirname(profile.installAnchor)`), not the binary's directory.
mod support;
use futures_util::future::BoxFuture;
use hoh::profile_boot::*;
use hoh_agent::{
    AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_launch_environment::create_launch_environment_snapshot;
use hoh_llm::UserMessage;
use hoh_loader::PluginRegistry;
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_shell_env::{Config, InstallAnchorKey, ShellEnvRegistry};
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::AbortController;
use hoh_tools::{
    Agent, DefineToolOptions, DefineToolOutput, ToolExecutionInput, ToolExecutionPipeline,
    ToolRegistrationOptions, ToolRuntime, Tools, define_tool,
};
use serde_json::json;
use std::{path::Path, sync::Arc};

/// The pipeline reads only the agent's scope key and session; the rest of the surface is unused.
struct NoAgent {
    session: Session,
    options: AgentOptions,
}
impl NoAgent {
    fn new() -> Self {
        Self {
            session: Session::create("install-anchor".into(), None, None, None, None).unwrap(),
            options: AgentOptions::default(),
        }
    }
}
impl Agent for NoAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }
    fn scope_key(&self) -> ScopeKey {
        ScopeKey::new()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn model(&self) -> &dyn Model {
        unreachable!("the install-anchor test never reads the model")
    }
    fn inbox(&self) -> &dyn Inbox {
        unreachable!("the install-anchor test never reads the inbox")
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        unreachable!("the install-anchor test never reads the agent context")
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        unreachable!("the install-anchor test never cancels")
    }
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        unreachable!("the install-anchor test never runs maintenance")
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        unreachable!("the install-anchor test never sends input")
    }
    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        unreachable!("the install-anchor test never sends input")
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        unreachable!("the install-anchor test never sends input")
    }
    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        unreachable!("the install-anchor test never sends input")
    }
}

fn options(root: &Path, install_root: Option<&Path>) -> RunProfileOptions {
    RunProfileOptions {
        profile: support::profile(root, "desktop", &[]),
        patch_files: vec![],
        home: root.join("home"),
        bundles: Arc::new(ShippedBundles),
        environment: create_launch_environment_snapshot(&[]),
        env: support::env(root),
        args: vec![],
        registry: Arc::new(PluginRegistry::default()),
        install_root: install_root.map(Path::to_path_buf),
    }
}

/// `DSH_INSTALL_DIR` as a bash call under the launched profile would see it.
async fn collected_install_dir(running: &RunningProfile) -> Option<String> {
    let ctx = running.ctx.clone();
    for plugin in [
        ctx.plugin::<SystemPrompt>(json!({})).unwrap(),
        ctx.plugin::<ToolRuntime>(json!({})).unwrap(),
    ] {
        plugin.settled().await.unwrap();
    }
    let bash = define_tool(DefineToolOptions::new(
        "bash",
        "test bash",
        json!({ "command": { "type": "string" } }),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| vec![hoh_llm::ContentBlock::text(value.as_str().unwrap())]),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(json!("ok")) }),
    ))
    .unwrap();
    std::mem::forget(
        ctx.get::<Tools>()
            .unwrap()
            .register(&ctx, bash, ToolRegistrationOptions::default())
            .unwrap(),
    );
    let mut input = ToolExecutionInput::new(
        "install-anchor-call".into(),
        "bash",
        json!({ "command": "true" }),
        AbortController::new().signal(),
    );
    input.agent = Some(Arc::new(NoAgent::new()));
    let execution = ToolExecutionPipeline::new(ctx.clone())
        .unwrap()
        .prepare(input)
        .await
        .unwrap()
        .exec()
        .clone();
    let registry = ShellEnvRegistry::new(&ctx, &Config::default()).unwrap();
    registry
        .collect(&execution)
        .unwrap()
        .get("DSH_INSTALL_DIR")
        .cloned()
}

#[test]
fn the_installation_root_is_the_prefix_above_bin_else_the_binary_directory() {
    assert_eq!(
        install_root_of(Path::new("/opt/dsh/bin/hoh")),
        Path::new("/opt/dsh")
    );
    assert_eq!(
        install_root_of(Path::new("/opt/dsh/hoh")),
        Path::new("/opt/dsh")
    );
}

#[tokio::test(start_paused = true)]
async fn dsh_install_dir_is_the_installation_root_not_the_binary_directory() {
    let root = tempfile::tempdir().unwrap();
    let install = root.path().join("dsh-install");
    // `support::env` puts the binary at `<root>/hoh`: the installation root differs from it.
    let running = run_profile(
        options(root.path(), Some(&install)),
        ProfileHost::new(Arc::new(|_| {}), Arc::new(|_| {})),
    )
    .await
    .unwrap();
    assert_eq!(
        running.ctx.get::<InstallAnchorKey>().unwrap().0,
        install.join("package.json")
    );
    let dir = collected_install_dir(&running).await.unwrap();
    assert_eq!(dir, install.to_string_lossy());
    assert_ne!(dir, root.path().to_string_lossy());
    running.host.shutdown.shutdown(0).await;
}

#[tokio::test(start_paused = true)]
async fn without_an_installation_root_no_anchor_is_provided() {
    let root = tempfile::tempdir().unwrap();
    let running = run_profile(
        options(root.path(), None),
        ProfileHost::new(Arc::new(|_| {}), Arc::new(|_| {})),
    )
    .await
    .unwrap();
    assert!(running.ctx.get::<InstallAnchorKey>().is_none());
    running.host.shutdown.shutdown(0).await;
}
