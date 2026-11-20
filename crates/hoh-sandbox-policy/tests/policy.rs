use super::*;
use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_brand::ids::SessionId;
use hoh_kernel::{Fiber, Kernel};
use hoh_llm::UserMessage;
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, SESSION_FORMAT_VERSION};
use hoh_system_prompt::{AssembleContext, PromptAssembly, render_context_snapshot, render_prompt};
use hoh_timeout::AbortController;
use serde_json::{Value, json};
use std::sync::Arc;

async fn load<P: Plugin>(ctx: &Context, config: Value) -> Fiber {
    let fiber = ctx.plugin::<P>(config).unwrap();
    fiber.settled().await.unwrap();
    fiber
}

struct Mounted {
    kernel: Kernel,
    ctx: Context,
    policy_fiber: Fiber,
}

#[tokio::test]
async fn deployment_protected_roots_are_deduplicated_and_cannot_be_relative() {
    let mounted = Mounted::new(
        json!({"protectedRoots":["/engine","/engine","/other"]}),
        false,
    )
    .await;
    assert_eq!(mounted.policy().protected_roots(), ["/engine", "/other"]);
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest::default())
            .unwrap()
            .protected_roots,
        Some(vec!["/engine".into(), "/other".into()])
    );
    let first = mounted
        .policy()
        .lock_protected_effect()
        .await
        .expect("protected configuration owns an effect lock");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(10),
            mounted.policy().lock_protected_effect(),
        )
        .await
        .is_err(),
        "a second model-controlled filesystem effect must wait"
    );
    drop(first);
    assert!(mounted.policy().lock_protected_effect().await.is_some());
    mounted.close().await;

    let kernel = Kernel::start();
    let ctx = kernel.root();
    load::<hoh_session_projection::SessionProjectionRegistry>(&ctx, json!({})).await;
    let fiber = ctx
        .plugin::<SandboxPolicyService>(json!({"protectedRoots":["relative/engine"]}))
        .unwrap();
    let error = fiber.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("protected paths must be absolute")
    );
    kernel.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_a_recreated_protected_path_after_its_original_identity_moves() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("application-data");
    let engine = data.join("engine");
    let moved = root.path().join("application-data-moved");
    let work = root.path().join("work");
    std::fs::create_dir_all(&engine).unwrap();
    std::fs::create_dir(&work).unwrap();
    let mounted = Mounted::new(
        json!({
            "workspaceRoot": work,
            "protectedRoots": [engine],
        }),
        false,
    )
    .await;

    std::fs::rename(&data, &moved).unwrap();
    std::fs::create_dir_all(&engine).unwrap();
    let error = mounted
        .policy()
        .validate_protected_root_identities()
        .await
        .expect_err("a replacement at the configured pathname must fail closed");
    let message = error.to_string();
    assert!(
        message.contains("protected path identity changed"),
        "{message}"
    );
    assert!(message.contains("refusing to run"), "{message}");
    mounted.close().await;
}

#[tokio::test]
async fn rejects_equal_nested_and_parent_workspaces_before_a_working_directory_change_commits() {
    let root = tempfile::tempdir().unwrap();
    let engine = root.path().join("engine");
    let engine_child = engine.join("sessions/work");
    let safe = root.path().join("safe");
    std::fs::create_dir_all(&engine_child).unwrap();
    std::fs::create_dir(&safe).unwrap();
    let mounted = Mounted::new(
        json!({
            "workspaceRoot": safe,
            "protectedRoots": [engine],
        }),
        false,
    )
    .await;

    for workspace in [root.path(), engine.as_path(), engine_child.as_path()] {
        let error = mounted
            .policy()
            .validate_workspace_root(&workspace.to_string_lossy())
            .await
            .expect_err("overlapping workspace must be rejected");
        let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.name(), WorkingDirectoryError::NAME);
        assert_eq!(error.code(), "WORKING_DIRECTORY_INVALID");
        assert!(error.message().contains("папкой данных HoH"));
    }

    let active = session("sess-overlap", Some(safe.to_str().unwrap()));
    let agent: Arc<dyn Agent> = Arc::new(FakeAgent::new(mounted.ctx.clone(), active));
    let reached_tail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tail_reached = reached_tail.clone();
    let result = mounted
        .ctx
        .waterfall(
            WorkingDirectoryAccess {
                agent,
                path: Some(engine.to_string_lossy().into_owned()),
                signal: AbortController::new().signal(),
            },
            move |_| async move {
                tail_reached.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok("committed".to_owned())
            },
        )
        .await;
    let error = result.expect_err("listener must reject before the working-directory tail");
    assert!(!reached_tail.load(std::sync::atomic::Ordering::SeqCst));
    let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.name(), WorkingDirectoryError::NAME);
    assert_eq!(error.code(), "WORKING_DIRECTORY_INVALID");
    mounted.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn canonicalizes_a_symlinked_protected_home_before_overlap_checks() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let physical = root.path().join("physical-engine");
    let workspace = physical.join("work");
    let safe = root.path().join("safe");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir(&safe).unwrap();
    let alias = root.path().join("engine-alias");
    symlink(&physical, &alias).unwrap();
    let mounted = Mounted::new(
        json!({"workspaceRoot":safe,"protectedRoots":[alias]}),
        false,
    )
    .await;
    let error = mounted
        .policy()
        .validate_workspace_root(&workspace.to_string_lossy())
        .await
        .expect_err("physical descendant of symlinked DSH_HOME must be rejected");
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        "WORKING_DIRECTORY_INVALID"
    );
    mounted.close().await;
}

impl Mounted {
    async fn new(config: Value, prompt: bool) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        load::<hoh_session_projection::SessionProjectionRegistry>(&ctx, json!({})).await;
        if prompt {
            load::<SystemPrompt>(&ctx, json!({})).await;
        }
        let policy_fiber = load::<SandboxPolicyService>(&ctx, config).await;
        Self {
            kernel,
            ctx,
            policy_fiber,
        }
    }

    fn policy(&self) -> Service<SandboxPolicy> {
        self.ctx.get::<SandboxPolicy>().unwrap()
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

fn session(id: &str, cwd: Option<&str>) -> Session {
    let id = SessionId::from(id);
    let mut header = json!({
        "version": SESSION_FORMAT_VERSION,
        "id": id,
        "createdAt": 0,
        "isSeeded": false,
    });
    if let Some(cwd) = cwd {
        header["cwd"] = cwd.into();
    }
    Session::create(id, None, Some(header), None, None).unwrap()
}

struct FakeAgent {
    session: Session,
    ctx: Context,
    key: ScopeKey,
    options: AgentOptions,
}

impl FakeAgent {
    fn new(ctx: Context, session: Session) -> Self {
        Self {
            session,
            ctx,
            key: ScopeKey::new(),
            options: AgentOptions::default(),
        }
    }
}

impl Agent for FakeAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }
    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn model(&self) -> &dyn Model {
        panic!("model is not used by sandbox-policy tests")
    }
    fn inbox(&self) -> &dyn Inbox {
        panic!("inbox is not used by sandbox-policy tests")
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("maintenance is not used by sandbox-policy tests")
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("send is not used by sandbox-policy tests")
    }
    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("followup is not used by sandbox-policy tests")
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("steer is not used by sandbox-policy tests")
    }
    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("inject is not used by sandbox-policy tests")
    }
}

async fn assembly(mounted: &Mounted, active: Session) -> PromptAssembly {
    let agent: Arc<dyn Agent> = Arc::new(FakeAgent::new(mounted.ctx.clone(), active));
    mounted
        .ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .assemble(AssembleContext::default().with_extension(AssembleAgent(agent)))
        .await
        .unwrap()
}

async fn policy_context(mounted: &Mounted, active: Session) -> Option<String> {
    assembly(mounted, active)
        .await
        .contexts
        .into_iter()
        .find(|context| context.name == "sandbox:policy")
        .map(|context| context.text)
}

#[tokio::test]
async fn defaults_to_read_only_under_the_process_cwd() {
    let mounted = Mounted::new(json!({}), false).await;
    assert_eq!(mounted.policy().default_mode, SandboxMode::ReadOnly);
    assert_eq!(
        mounted.policy().workspace_root,
        std::env::current_dir().unwrap().to_string_lossy()
    );
    mounted.close().await;
}

#[tokio::test]
async fn preserves_an_absolute_execution_world_root_without_host_path_normalization() {
    let mounted = Mounted::new(
        json!({"mode":"workspace-write","workspaceRoot":"/ws/../ws/./sub"}),
        false,
    )
    .await;
    assert_eq!(mounted.policy().default_mode, SandboxMode::WorkspaceWrite);
    assert_eq!(mounted.policy().workspace_root, "/ws/../ws/./sub");
    mounted.close().await;
}

#[tokio::test]
async fn rejects_an_invalid_deployment_workspace_root_at_load() {
    for root in ["relative/workspace", "/invalid\0root"] {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        load::<hoh_session_projection::SessionProjectionRegistry>(&ctx, json!({})).await;
        let fiber = ctx
            .plugin::<SandboxPolicyService>(json!({"workspaceRoot":root}))
            .unwrap();
        let error = fiber.settled().await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("workspace root must be an absolute")
        );
        assert!(ctx.get::<SandboxPolicy>().is_none());
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn rejects_a_session_cwd_containing_a_nul_byte_with_sandbox_invalid_root() {
    let mounted = Mounted::new(
        json!({"mode":"workspace-write","workspaceRoot":"/ws"}),
        false,
    )
    .await;
    let cwd = "/invalid\0session";
    let error = mounted
        .policy()
        .resolve(SandboxPolicyRequest {
            session: Some(&session("sess-invalid-cwd", Some(cwd))),
            mode: None,
        })
        .unwrap_err();
    let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.name(), "SandboxInvalidRootError");
    assert_eq!(error.code(), "SANDBOX_INVALID_ROOT");
    assert_eq!(
        error.message(),
        "Invalid sandbox workspace root \"/invalid\\u0000session\": the Session working directory must be an absolute execution-world path without NUL bytes"
    );
    mounted.close().await;
}

#[tokio::test]
async fn resolves_the_deployment_policy_for_an_agentless_call() {
    let mounted = Mounted::new(
        json!({"mode":"workspace-write","workspaceRoot":"/fallback"}),
        false,
    )
    .await;
    let policy = mounted.policy().resolve(Default::default()).unwrap();
    assert_eq!(policy.mode, SandboxMode::WorkspaceWrite);
    assert_eq!(policy.workspace_root, "/fallback");
    assert!(policy.additional_writable_roots.is_none());
    assert!(policy.session_id.is_none());
    mounted.close().await;
}

#[tokio::test]
async fn carries_a_frozen_detached_copy_of_the_configured_extra_writable_roots() {
    let mut configured = vec![
        "/projects/shared/.git".to_owned(),
        "/projects/shared/../shared/worktrees".to_owned(),
    ];
    let mounted = Mounted::new(
        json!({
            "mode":"workspace-write",
            "workspaceRoot":"/fallback",
            "additionalWritableRoots":configured,
        }),
        false,
    )
    .await;
    configured.push("/late/mutation".into());
    assert_eq!(
        mounted
            .policy()
            .resolve(Default::default())
            .unwrap()
            .additional_writable_roots
            .unwrap(),
        [
            "/projects/shared/.git",
            "/projects/shared/../shared/worktrees"
        ]
    );
    assert_eq!(
        mounted
            .policy()
            .additional_writable_roots
            .as_deref()
            .unwrap()
            .len(),
        2
    );
    mounted.close().await;
}

#[tokio::test]
async fn rejects_a_relative_additional_writable_root_at_load() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    load::<hoh_session_projection::SessionProjectionRegistry>(&ctx, json!({})).await;
    let fiber = ctx
        .plugin::<SandboxPolicyService>(json!({"additionalWritableRoots":["relative/extra"]}))
        .unwrap();
    let error = fiber.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("writable directories must be absolute")
    );
    assert!(ctx.get::<SandboxPolicy>().is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn resolves_each_session_mode_and_cwd_together_without_changing_the_fallback() {
    let mounted = Mounted::new(
        json!({"mode":"workspace-write","workspaceRoot":"/fallback"}),
        false,
    )
    .await;
    let first = session("sess-first", Some("/projects/first"));
    let second = session("sess-second", Some("/projects/second"));
    set_sandbox_mode(&second, SandboxMode::ReadOnly).unwrap();
    let first_policy = mounted
        .policy()
        .resolve(SandboxPolicyRequest {
            session: Some(&first),
            mode: None,
        })
        .unwrap();
    let second_policy = mounted
        .policy()
        .resolve(SandboxPolicyRequest {
            session: Some(&second),
            mode: None,
        })
        .unwrap();
    assert_eq!(first_policy.mode, SandboxMode::WorkspaceWrite);
    assert_eq!(first_policy.workspace_root, "/projects/first");
    assert_eq!(first_policy.session_id, Some(SessionId::from("sess-first")));
    assert_eq!(second_policy.mode, SandboxMode::ReadOnly);
    assert_eq!(second_policy.workspace_root, "/projects/second");
    assert_eq!(
        second_policy.session_id,
        Some(SessionId::from("sess-second"))
    );
    assert_eq!(mounted.policy().override_of(&first).unwrap(), None);
    assert_eq!(
        mounted.policy().override_of(&second).unwrap(),
        Some(SandboxMode::ReadOnly)
    );
    let deployment_policy = mounted
        .policy()
        .resolve(SandboxPolicyRequest::default())
        .unwrap();
    assert_eq!(deployment_policy.mode, SandboxMode::WorkspaceWrite);
    assert_eq!(deployment_policy.workspace_root, "/fallback");
    assert_eq!(deployment_policy.session_id, None);
    mounted.close().await;
}

#[tokio::test]
async fn preserves_symlink_sensitive_session_cwd_for_its_enforcing_provider() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let lexical = root.path().join("lexical");
        let child = root.path().join("physical/child");
        std::fs::create_dir(&lexical).unwrap();
        std::fs::create_dir_all(&child).unwrap();
        let link = lexical.join("link");
        symlink(&child, &link).unwrap();
        let cwd = link.join("..").to_string_lossy().into_owned();
        let mounted = Mounted::new(
            json!({"mode":"workspace-write","workspaceRoot":"/fallback"}),
            false,
        )
        .await;
        let active = session("sess-symlink-parent", Some(&cwd));
        let policy = mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&active),
                mode: None,
            })
            .unwrap();
        assert_eq!(policy.workspace_root, cwd);
        assert_eq!(
            policy.session_id,
            Some(SessionId::from("sess-symlink-parent"))
        );
        mounted.close().await;
    }
}

#[tokio::test]
async fn lets_an_approved_mode_outrank_the_session_mode_while_retaining_its_root() {
    let mounted = Mounted::new(json!({"workspaceRoot":"/fallback"}), false).await;
    let active = session("sess-approved", Some("/projects/approved"));
    set_sandbox_mode(&active, SandboxMode::ReadOnly).unwrap();
    let policy = mounted
        .policy()
        .resolve(SandboxPolicyRequest {
            session: Some(&active),
            mode: Some(SandboxMode::DangerFullAccess),
        })
        .unwrap();
    assert_eq!(policy.mode, SandboxMode::DangerFullAccess);
    assert_eq!(policy.workspace_root, "/projects/approved");
    assert_eq!(policy.session_id, Some(SessionId::from("sess-approved")));
    mounted.close().await;
}

#[tokio::test]
async fn uses_the_configured_root_when_a_session_has_no_cwd() {
    let mounted = Mounted::new(json!({"workspaceRoot":"/fallback"}), false).await;
    let active = session("sess-no-cwd", None);
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&active),
                mode: None,
            })
            .unwrap()
            .workspace_root,
        "/fallback"
    );
    mounted.close().await;
}

#[tokio::test]
async fn rejects_a_mode_outside_the_closed_vocabulary_at_load() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    load::<hoh_session_projection::SessionProjectionRegistry>(&ctx, json!({})).await;
    let fiber = ctx
        .plugin::<SandboxPolicyService>(json!({"mode":"yolo"}))
        .unwrap();
    assert!(fiber.settled().await.is_err());
    assert!(ctx.get::<SandboxPolicy>().is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn disposes_the_service_and_context_contribution_from_a_child_fiber() {
    let mounted = Mounted::new(json!({}), true).await;
    assert!(mounted.ctx.get::<SandboxPolicy>().is_some());
    assert!(
        policy_context(&mounted, session("sess-hmr", None))
            .await
            .unwrap()
            .contains("read-only")
    );
    mounted.policy_fiber.dispose().await;
    assert!(mounted.ctx.get::<SandboxPolicy>().is_none());
    let contexts = assembly(&mounted, session("sess-hmr-after", None))
        .await
        .contexts;
    assert!(contexts.iter().all(|entry| entry.name != "sandbox:policy"));
    mounted.close().await;
}

#[tokio::test]
async fn isolates_live_sessions_preserves_configured_roots_and_detaches_resolved_policies() {
    let mounted = Mounted::new(
        json!({"mode":"workspace-write","additionalWritableRoots":["/shared"]}),
        false,
    )
    .await;
    let first = session("first", Some("/workspace"));
    let second = session("second", Some("/workspace"));
    let before = mounted
        .policy()
        .resolve(SandboxPolicyRequest {
            session: Some(&first),
            mode: None,
        })
        .unwrap();
    let remove = mounted
        .policy()
        .register_writable_root(&mounted.ctx, &first, "/memory")
        .unwrap();
    let granted = mounted
        .policy()
        .resolve(SandboxPolicyRequest {
            session: Some(&first),
            mode: None,
        })
        .unwrap();
    assert_eq!(
        granted
            .additional_writable_roots
            .as_ref()
            .map(|roots| roots.iter().map(String::as_str).collect::<Vec<_>>()),
        Some(vec!["/shared", "/memory"])
    );
    assert_eq!(
        before
            .additional_writable_roots
            .as_ref()
            .map(|roots| roots.iter().map(String::as_str).collect::<Vec<_>>()),
        Some(vec!["/shared"])
    );
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest::default())
            .unwrap()
            .additional_writable_roots
            .as_ref()
            .map(|roots| roots.iter().map(String::as_str).collect::<Vec<_>>()),
        Some(vec!["/shared"])
    );
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&second),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .as_ref()
            .map(|roots| roots.iter().map(String::as_str).collect::<Vec<_>>()),
        Some(vec!["/shared"])
    );
    let resumed = Session::create(
        first.id().clone(),
        Some(&first.snapshot_events(None, None)),
        Some((*first.header()).clone()),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&resumed),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .as_ref()
            .map(|roots| roots.iter().map(String::as_str).collect::<Vec<_>>()),
        Some(vec!["/shared"])
    );
    remove.dispose().await;
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&first),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .as_ref()
            .map(|roots| roots.iter().map(String::as_str).collect::<Vec<_>>()),
        Some(vec!["/shared"])
    );
    assert_eq!(
        granted
            .additional_writable_roots
            .as_ref()
            .map(|roots| roots.iter().map(String::as_str).collect::<Vec<_>>()),
        Some(vec!["/shared", "/memory"])
    );
    mounted.close().await;
}

#[tokio::test]
async fn keeps_duplicate_grants_independently_owned_across_plugin_unload_and_disposal() {
    let mounted = Mounted::new(
        json!({"mode":"workspace-write","additionalWritableRoots":["/shared"]}),
        false,
    )
    .await;
    let active = session("duplicates", None);
    let child = mounted.ctx.child("sandbox-policy-grant-owner").unwrap();
    child.fiber().settled().await.unwrap();
    let _duplicate = mounted
        .policy()
        .register_writable_root(&child, &active, "/memory")
        .unwrap();
    let _plugin_only = mounted
        .policy()
        .register_writable_root(&child, &active, "/plugin-only")
        .unwrap();
    let _configured_duplicate = mounted
        .policy()
        .register_writable_root(&child, &active, "/shared")
        .unwrap();
    let remove = mounted
        .policy()
        .register_writable_root(&mounted.ctx, &active, "/memory")
        .unwrap();
    let replacement = mounted
        .policy()
        .register_writable_root(&mounted.ctx, &active, "/memory")
        .unwrap();
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&active),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .unwrap(),
        ["/shared", "/memory", "/plugin-only"]
    );
    child.fiber().dispose().await;
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&active),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .unwrap(),
        ["/shared", "/memory"]
    );
    remove.dispose().await;
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&active),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .unwrap(),
        ["/shared", "/memory"]
    );
    replacement.dispose().await;
    assert_eq!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&active),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .unwrap(),
        ["/shared"]
    );
    mounted.close().await;
}

#[tokio::test]
async fn rejects_an_invalid_directory_without_installing_a_grant() {
    let mounted = Mounted::new(json!({}), false).await;
    let active = session("invalid", None);
    for directory in ["relative/memory", "/memory\0invalid"] {
        let error = match mounted
            .policy()
            .register_writable_root(&mounted.ctx, &active, directory)
        {
            Ok(_) => panic!("invalid root registration succeeded"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("absolute execution-world paths"));
    }
    assert!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&active),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .is_none()
    );
    mounted.close().await;
}

#[tokio::test]
async fn updates_request_context_without_changing_read_only_policy_or_the_stable_prompt() {
    let mounted = Mounted::new(json!({"mode":"workspace-write"}), false).await;
    load::<SystemPrompt>(&mounted.ctx, json!({})).await;
    let active = session("context", Some("/workspace"));
    let before = assembly(&mounted, active.clone()).await;
    let remove = mounted
        .policy()
        .register_writable_root(&mounted.ctx, &active, "/memory")
        .unwrap();
    let after = assembly(&mounted, active.clone()).await;
    assert_eq!(
        render_prompt(&after).unwrap(),
        render_prompt(&before).unwrap()
    );
    assert!(
        render_context_snapshot(&after)
            .unwrap()
            .contains("Additional writable directories: [\"/memory\"]")
    );
    set_sandbox_mode(&active, SandboxMode::ReadOnly).unwrap();
    assert!(
        !policy_context(&mounted, active.clone())
            .await
            .unwrap()
            .contains("/memory")
    );
    set_sandbox_mode(&active, SandboxMode::WorkspaceWrite).unwrap();
    remove.dispose().await;
    assert!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&active),
                mode: None,
            })
            .unwrap()
            .additional_writable_roots
            .is_none()
    );
    assert!(
        !policy_context(&mounted, active)
            .await
            .unwrap()
            .contains("/memory")
    );
    mounted.close().await;
}

#[tokio::test]
async fn renders_each_exact_policy_without_a_capability_inventory() {
    let expected = [
        (
            SandboxMode::ReadOnly,
            "Current DSH file policy: read-only. Any available operation enforced by the DSH file sandbox cannot modify files in the standing mode. Do not refuse a required modification from this policy alone: try an available tool normally and follow any denial and escalation guidance it returns.",
        ),
        (
            SandboxMode::WorkspaceWrite,
            "Current DSH file policy: workspace-write. Any available operation enforced by the DSH file sandbox may modify files under the session workspace: \"/projects/../projects/current\". Some platform temporary areas may also be writable.",
        ),
        (
            SandboxMode::DangerFullAccess,
            "Current DSH file policy: danger-full-access. The DSH file sandbox does not restrict file modifications by available operations.",
        ),
    ];
    for (mode, expected) in expected {
        let mounted = Mounted::new(json!({"mode":mode,"workspaceRoot":"/fallback"}), true).await;
        assert_eq!(
            policy_context(
                &mounted,
                session(
                    &format!("sess-{mode}"),
                    Some("/projects/../projects/current")
                ),
            )
            .await
            .as_deref(),
            Some(expected)
        );
        mounted.close().await;
    }
}

#[tokio::test]
async fn renders_explicit_extra_writable_directories_under_workspace_write() {
    let mounted = Mounted::new(
        json!({
            "mode":"workspace-write",
            "workspaceRoot":"/fallback",
            "additionalWritableRoots":["/projects/shared/.git","/projects/shared/worktrees"],
        }),
        true,
    )
    .await;
    assert_eq!(
        policy_context(&mounted, session("sess-extra", Some("/projects/current")))
            .await
            .as_deref(),
        Some(
            "Current DSH file policy: workspace-write. Any available operation enforced by the DSH file sandbox may modify files under the session workspace: \"/projects/current\". Additional writable directories: [\"/projects/shared/.git\",\"/projects/shared/worktrees\"]. On Windows, these additional grants apply to file tools only, not sandboxed processes. Some platform temporary areas may also be writable."
        )
    );
    mounted.close().await;
}

#[tokio::test]
async fn does_not_advertise_extra_directories_under_read_only() {
    let mounted = Mounted::new(
        json!({
            "mode":"read-only",
            "workspaceRoot":"/fallback",
            "additionalWritableRoots":["/projects/shared/.git"],
        }),
        true,
    )
    .await;
    assert_eq!(
        policy_context(
            &mounted,
            session("sess-readonly-extra", Some("/projects/current"))
        )
        .await
        .as_deref(),
        Some(
            "Current DSH file policy: read-only. Any available operation enforced by the DSH file sandbox cannot modify files in the standing mode. Do not refuse a required modification from this policy alone: try an available tool normally and follow any denial and escalation guidance it returns."
        )
    );
    mounted.close().await;
}

#[tokio::test]
async fn keeps_the_complete_rendered_prompt_byte_stable_across_host_temp_choices() {
    let mounted = Mounted::new(json!({"mode":"workspace-write"}), true).await;
    let active = session("sess-tmpdir-stability", Some("/projects/current"));
    let first = assembly(&mounted, active.clone()).await;
    let second = assembly(&mounted, active).await;
    assert_eq!(
        render_prompt(&second).unwrap(),
        render_prompt(&first).unwrap()
    );
    assert_eq!(
        render_context_snapshot(&second).unwrap(),
        render_context_snapshot(&first).unwrap()
    );
    assert!(
        !render_context_snapshot(&first)
            .unwrap()
            .contains("host-temp")
    );
    mounted.close().await;
}

#[tokio::test]
async fn reflects_the_latest_durable_switch_and_stays_byte_stable_otherwise() {
    let mounted = Mounted::new(json!({}), true).await;
    let active = session("sess-switch", Some("/projects/current"));
    let first = policy_context(&mounted, active.clone()).await.unwrap();
    assert_eq!(
        policy_context(&mounted, active.clone()).await.as_deref(),
        Some(first.as_str())
    );
    set_sandbox_mode(&active, SandboxMode::DangerFullAccess).unwrap();
    let danger = policy_context(&mounted, active.clone()).await.unwrap();
    assert_eq!(
        danger,
        "Current DSH file policy: danger-full-access. The DSH file sandbox does not restrict file modifications by available operations."
    );
    assert_eq!(
        policy_context(&mounted, active.clone()).await.as_deref(),
        Some(danger.as_str())
    );
    set_sandbox_mode(&active, SandboxMode::WorkspaceWrite).unwrap();
    assert_eq!(
        policy_context(&mounted, active).await.as_deref(),
        Some(
            "Current DSH file policy: workspace-write. Any available operation enforced by the DSH file sandbox may modify files under the session workspace: \"/projects/current\". Some platform temporary areas may also be writable."
        )
    );
    mounted.close().await;
}

#[tokio::test]
async fn reconstructs_resumed_policy_from_the_log_and_omits_diagnostics_without_an_agent() {
    let active = session("sess-resume", Some("/projects/current"));
    set_sandbox_mode(&active, SandboxMode::WorkspaceWrite).unwrap();
    let resumed = Session::create(
        active.id().clone(),
        Some(&active.snapshot_events(None, None)),
        Some((*active.header()).clone()),
        None,
        None,
    )
    .unwrap();
    let mounted = Mounted::new(json!({"mode":"read-only"}), true).await;
    assert!(
        policy_context(&mounted, resumed)
            .await
            .unwrap()
            .contains("workspace-write")
    );
    let assembly = mounted
        .ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .assemble(AssembleContext::default())
        .await
        .unwrap();
    assert_eq!(
        assembly
            .contexts
            .iter()
            .find(|context| context.name == "sandbox:policy")
            .map(|context| context.text.as_str()),
        Some("")
    );
    mounted.close().await;
}

#[test]
fn sandbox_modes_lists_every_mode_for_advertisement_and_validation() {
    assert_eq!(
        SANDBOX_MODES,
        [
            SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite,
            SandboxMode::DangerFullAccess,
        ]
    );
}

#[tokio::test]
async fn sandbox_mode_projection_folds_to_the_last_switch_or_none() {
    let mounted = Mounted::new(json!({}), false).await;
    let active = session("sess-fold", None);
    assert_eq!(mounted.policy().override_of(&active).unwrap(), None);
    let projections = mounted.ctx.get::<SessionProjections>().unwrap();
    assert_eq!(
        projections.checkpoint(&active).unwrap()["sandboxMode"]
            .val
            .as_ref(),
        Some(&Value::Null)
    );
    set_sandbox_mode(&active, SandboxMode::WorkspaceWrite).unwrap();
    set_sandbox_mode(&active, SandboxMode::ReadOnly).unwrap();
    assert_eq!(
        mounted.policy().override_of(&active).unwrap(),
        Some(SandboxMode::ReadOnly)
    );
    assert_eq!(
        projections.checkpoint(&active).unwrap()["sandboxMode"]
            .val
            .as_ref(),
        Some(&json!("read-only"))
    );
    mounted.close().await;
}

#[test]
fn sandbox_mode_projection_state_schema_accepts_only_null_or_a_known_mode() {
    assert!(sandbox_mode_state(Some(&json!(1))).is_err());
    assert!(sandbox_mode_state(Some(&json!("yolo"))).is_err());
    assert_eq!(sandbox_mode_state(Some(&Value::Null)).unwrap().0, None);
}

#[test]
fn set_sandbox_mode_appends_exactly_one_event_per_switch() {
    let active = session("sess-write", None);
    set_sandbox_mode(&active, SandboxMode::DangerFullAccess).unwrap();
    let snapshot = active.snapshot_events(None, None);
    let events: Vec<_> = snapshot
        .iter()
        .filter(|event| event.event_type() == SandboxModeEvent::TYPE)
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data(), &json!({"mode":"danger-full-access"}));
}

#[tokio::test]
async fn reads_the_session_projection_only_when_no_mode_is_explicit() {
    let mounted = Mounted::new(
        json!({"mode":"workspace-write","workspaceRoot":"/ws"}),
        false,
    )
    .await;
    let broken = session("sess-broken", Some("/ws"));
    broken
        .append("sandbox/mode", json!({"mode": "not-a-mode"}), None)
        .unwrap();
    // The setup is only meaningful if the projection really fails on this session.
    assert!(mounted.policy().override_of(&broken).is_err());
    assert!(
        mounted
            .policy()
            .resolve(SandboxPolicyRequest {
                session: Some(&broken),
                mode: None,
            })
            .is_err()
    );
    // An explicit mode never consults it (source `index.ts:215`).
    let policy = mounted
        .policy()
        .resolve(SandboxPolicyRequest {
            session: Some(&broken),
            mode: Some(SandboxMode::ReadOnly),
        })
        .unwrap();
    assert_eq!(policy.mode, SandboxMode::ReadOnly);
    mounted.close().await;
}
