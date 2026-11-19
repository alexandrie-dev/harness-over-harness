//! The confining composition with the accepted crates only: one `sandboxPolicy` service, the
//! sandboxing filesystem plugin (`hoh-fs-sandbox`), the read-before-mutation policy
//! (`hoh-fs-observation-policy`), the working directory and `ToolFs`. No fake filesystem.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hoh_agent::AgentRegistryPlugin;
use hoh_fs::Fs;
use hoh_fs_observation_policy::FsObservationPolicy;
use hoh_fs_sandbox::FsSandboxPlugin;
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_sandbox::SandboxMode;
use hoh_sandbox_policy::SandboxPolicyService;
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_fs::ToolFs;
use hoh_tools::{Agent, ToolRuntime, Tools};
use hoh_user_approval::{ApprovalOutcome, ApprovalRequestEvent, ApprovalService};
use hoh_working_directory::WorkingDirectory;
use serde_json::Value;
use serde_json::json;
use support::{Answerer, TestAgent, call, error_info};

struct Composition {
    _base: tempfile::TempDir,
    workspace: PathBuf,
    outside: PathBuf,
    kernel: Kernel,
    ctx: Context,
    sandbox: Fiber,
    observation: Fiber,
    tool_fs: Fiber,
}

impl Composition {
    async fn new() -> Self {
        Self::with_approval(None).await
    }

    /// `approval`: mount the real approval service with a user answering with this outcome.
    async fn with_approval(approval: Option<Arc<Answerer>>) -> Self {
        // Under the working directory, not `/tmp`: the temporary directory is always writable.
        let base = tempfile::Builder::new()
            .prefix("hoh-toolfs-real-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        let root = base.path().canonicalize().unwrap();
        let workspace = root.join("ws");
        let outside = root.join("out");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let workspace_text = workspace.to_string_lossy().into_owned();
        let kernel = Kernel::start();
        let ctx = kernel.root();
        // The one real `sandboxPolicy` service: workspace-write over the workspace.
        ctx.plugin::<SessionProjectionRegistry>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<SandboxPolicyService>(json!({
            "mode": "workspace-write",
            "workspaceRoot": workspace_text,
        }))
        .unwrap()
        .settled()
        .await
        .unwrap();
        if let Some(answerer) = approval {
            ctx.plugin::<ApprovalService>(json!({}))
                .unwrap()
                .settled()
                .await
                .unwrap();
            ctx.on_waterfall::<ApprovalRequestEvent, Value, _>(move |_, _| {
                answerer
                    .asked
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let outcome = *answerer
                    .outcome
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                async move { Ok(outcome.into()) }
            })
            .unwrap();
        }
        let sandbox = ctx
            .plugin::<FsSandboxPlugin>(json!({ "cwd": workspace_text }))
            .unwrap();
        sandbox.settled().await.unwrap();
        for plugin in [
            ctx.plugin::<AgentRegistryPlugin>(json!({})).unwrap(),
            ctx.plugin::<SystemPrompt>(json!({ "includeRuntimeContext": false }))
                .unwrap(),
            ctx.plugin::<ToolRuntime>(json!({})).unwrap(),
            ctx.plugin::<NativeToolExecutorPlugin>(json!({})).unwrap(),
            ctx.plugin::<WorkingDirectory>(json!({ "defaultDirectory": workspace_text }))
                .unwrap(),
        ] {
            plugin.settled().await.unwrap();
        }
        let observation = ctx.plugin::<FsObservationPolicy>(json!({})).unwrap();
        observation.settled().await.unwrap();
        let tool_fs = ctx.plugin::<ToolFs>(json!({})).unwrap();
        tool_fs.settled().await.unwrap();
        Self {
            _base: base,
            workspace,
            outside,
            kernel,
            ctx,
            sandbox,
            observation,
            tool_fs,
        }
    }

    fn agent(&self, id: &str) -> Arc<dyn Agent> {
        TestAgent::in_dir(&self.ctx, id, &self.workspace.to_string_lossy())
    }

    async fn write(&self, agent: &Arc<dyn Agent>, path: &str, content: &str) -> String {
        let result = call(
            &self.ctx,
            "write",
            json!({ "file_path": path, "content": content }),
            Some(agent.clone()),
        )
        .await;
        if result.is_error() {
            format!(
                "ERR {:?} {}",
                error_info(&result).map(|(_, code)| code),
                result.text()
            )
        } else {
            result.text()
        }
    }
}

fn denied(outcome: &str) {
    assert!(
        outcome.starts_with("ERR Some(\"FS_SANDBOX_DENIED\")"),
        "{outcome}"
    );
    assert!(
        outcome.contains("[sandbox: file access denied"),
        "{outcome}"
    );
}

fn path(base: &Path, tail: &str) -> String {
    base.join(tail).to_string_lossy().into_owned()
}

#[tokio::test]
async fn one_policy_service_activates_the_sandbox_and_tool_fs_together() {
    let c = Composition::new().await;
    for (name, fiber) in [
        ("sandbox", &c.sandbox),
        ("observation", &c.observation),
        ("tool-fs", &c.tool_fs),
    ] {
        assert_eq!(fiber.state(), hoh_kernel::FiberState::Active, "{name}");
    }
    assert_eq!(
        c.ctx.get::<Fs>().unwrap().sandbox_mode(),
        Some(SandboxMode::WorkspaceWrite)
    );
    let tools = c.ctx.get::<Tools>().unwrap();
    let schemas = tools.schemas(None, None).unwrap();
    let write = schemas.iter().find(|tool| tool.name == "write").unwrap();
    let parameters = serde_json::to_value(&write.parameters).unwrap();
    assert!(
        parameters["properties"]
            .get("sandbox_permissions")
            .is_some(),
        "a confining backend advertises the escalation fields"
    );
    c.kernel.shutdown().await;
}

#[tokio::test]
async fn writes_inside_the_workspace_and_refuses_everything_outside_it() {
    let c = Composition::new().await;
    let agent = c.agent("confined");

    let inside = c.write(&agent, "inside.txt", "ok").await;
    assert!(inside.contains("Created file"), "{inside}");
    assert_eq!(
        std::fs::read_to_string(c.workspace.join("inside.txt")).unwrap(),
        "ok"
    );

    // An absolute path outside the workspace.
    denied(
        &c.write(&agent, &path(&c.outside, "absolute.txt"), "pwn")
            .await,
    );
    // A relative path that leaves the workspace through `..`.
    denied(&c.write(&agent, "../out/dotdot.txt", "pwn").await);
    // A symbolic link inside the workspace that points outside it.
    std::os::unix::fs::symlink(&c.outside, c.workspace.join("link")).unwrap();
    denied(&c.write(&agent, "link/through-link.txt", "pwn").await);

    for name in ["absolute.txt", "dotdot.txt", "through-link.txt"] {
        assert!(!c.outside.join(name).exists(), "{name} must not exist");
    }
    c.kernel.shutdown().await;
}

#[tokio::test]
async fn an_unread_file_is_not_overwritten_until_it_has_been_read() {
    let c = Composition::new().await;
    let agent = c.agent("unread");
    std::fs::write(c.workspace.join("existing.txt"), "original\n").unwrap();

    let unread = c.write(&agent, "existing.txt", "clobbered").await;
    assert!(
        unread.starts_with("ERR Some(\"FS_NOT_OBSERVED\")"),
        "{unread}"
    );
    assert_eq!(
        std::fs::read_to_string(c.workspace.join("existing.txt")).unwrap(),
        "original\n"
    );

    let read = call(
        &c.ctx,
        "read",
        json!({ "file_path": "existing.txt" }),
        Some(agent.clone()),
    )
    .await;
    assert!(!read.is_error(), "{}", read.text());
    let overwritten = c.write(&agent, "existing.txt", "replaced\n").await;
    assert!(!overwritten.starts_with("ERR"), "{overwritten}");
    assert_eq!(
        std::fs::read_to_string(c.workspace.join("existing.txt")).unwrap(),
        "replaced\n"
    );
    // Another session never read it, so it is refused there too.
    let other = c.agent("unread-other");
    let refused = c.write(&other, "existing.txt", "again").await;
    assert!(
        refused.starts_with("ERR Some(\"FS_NOT_OBSERVED\")"),
        "{refused}"
    );
    c.kernel.shutdown().await;
}

fn write_args(path: &str) -> Value {
    json!({
        "file_path": path,
        "content": "approved",
        "sandbox_permissions": "danger-full-access",
        "justification": "The user asked for a file outside the workspace.",
    })
}

#[tokio::test]
async fn a_write_outside_the_workspace_asks_the_user_and_obeys_the_answer() {
    for (outcome, lands) in [
        (ApprovalOutcome::Rejected, false),
        (ApprovalOutcome::AllowedOnce, true),
    ] {
        let answerer = Answerer::new(outcome);
        let c = Composition::with_approval(Some(answerer.clone())).await;
        let agent = c.agent("escalating");
        let target = path(&c.outside, "escalated.txt");

        // Without the escalation fields the standing policy refuses and nobody is asked.
        denied(&c.write(&agent, &target, "plain").await);
        assert_eq!(answerer.asked.load(std::sync::atomic::Ordering::SeqCst), 0);

        let result = call(&c.ctx, "write", write_args(&target), Some(agent.clone())).await;
        assert_eq!(
            answerer.asked.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the escalation is resolved through the approval service: {}",
            result.text()
        );
        assert_eq!(result.is_error(), !lands, "{}", result.text());
        assert_eq!(
            std::fs::read_to_string(c.outside.join("escalated.txt")).ok(),
            lands.then(|| "approved".to_owned()),
            "{}",
            result.text()
        );
        c.kernel.shutdown().await;
    }
}
