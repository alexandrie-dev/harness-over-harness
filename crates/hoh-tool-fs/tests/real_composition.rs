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
use hoh_sandbox::{SandboxExecutionPolicy, SandboxMode};
use hoh_session::Session;
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_fs::ToolFs;
use hoh_tool_fs::shim::{SandboxPolicyApi, SandboxPolicyKey};
use hoh_tools::{Agent, ToolRuntime, Tools};
use hoh_working_directory::WorkingDirectory;
use serde_json::json;
use support::{TestAgent, call, error_info};

/// The one standing policy of the composition: workspace-write over one root.
struct Policy {
    workspace: String,
}

impl SandboxPolicyApi for Policy {
    fn resolve(&self, session: Option<&Session>) -> SandboxExecutionPolicy {
        SandboxExecutionPolicy {
            mode: SandboxMode::WorkspaceWrite,
            workspace_root: self.workspace.clone(),
            additional_writable_roots: None,
            session_id: session.map(|session| session.id().clone()),
        }
    }
}

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
        let policy: Arc<dyn SandboxPolicyApi> = Arc::new(Policy {
            workspace: workspace_text.clone(),
        });
        ctx.provide::<SandboxPolicyKey>(policy).await.unwrap();
        let sandbox = ctx
            .plugin::<FsSandboxPlugin>(json!({ "cwd": workspace_text }))
            .unwrap();
        sandbox.settled().await.unwrap();
        for plugin in [
            ctx.plugin::<SessionProjectionRegistry>(json!({})).unwrap(),
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
