//! Compiled plugin table. Missing packages remain unresolved, never stand-in plugins.
use hoh_kernel::{Cfg, Context, Error, NoConfig, Plugin, Service};
#[cfg(debug_assertions)]
use hoh_loader::cmdline::AppExitKey;
use hoh_loader::{LoaderKey, PluginRegistry, ProfileContextKey};
#[derive(hoh_kernel::Inject)]
struct EditorDeps {
    _loader: Service<LoaderKey>,
    _profile: Service<ProfileContextKey>,
}
struct ConfigEditor;
impl Plugin for ConfigEditor {
    const NAME: &'static str = "config-editor";
    type Config = NoConfig;
    type Deps = EditorDeps;
    async fn apply(ctx: Context, _: EditorDeps, _: Cfg<NoConfig>) -> Result<(), Error> {
        let host = ctx
            .tag::<super::profile_boot::EditorHost>()
            .ok_or("configuration editor requires an active profile host")?;
        hoh_loader::config_editor::ConfigEditor::mount_with_overlays(
            &ctx,
            host.dir.clone(),
            host.bundles.clone(),
            host.home.clone(),
            host.overlays.clone(),
            host.env.clone(),
        )
        .await?;
        Ok(())
    }
}

#[cfg(debug_assertions)]
struct ExitThenFailFixture;
#[cfg(debug_assertions)]
impl Plugin for ExitThenFailFixture {
    const NAME: &'static str = "hoh:test-exit-then-fail";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        ctx.get::<AppExitKey>().ok_or("missing appExit")?.request(7);
        Err("failure after exit request".into())
    }
}

pub fn registry() -> PluginRegistry {
    let mut registry = PluginRegistry::default();
    registry
        .add::<ConfigEditor>("@deepseek-ai/dsh-config-editor")
        .add::<hoh_llm::LlmPlugin>("@deepseek-ai/dsh-llm")
        .add::<hoh_session::SessionPlugin>("@deepseek-ai/dsh-session")
        .add::<hoh_session_projection::SessionProjectionRegistry>(
            "@deepseek-ai/dsh-session-projection",
        )
        .add::<hoh_fs_local::FsLocalPlugin>("@deepseek-ai/dsh-fs-local")
        .add::<hoh_persona::Persona>("@deepseek-ai/dsh-persona")
        .add::<hoh_system_prompt::SystemPrompt>("@deepseek-ai/dsh-system-prompt")
        .add::<hoh_skill::SkillRegistry>("@deepseek-ai/dsh-skill")
        .add::<hoh_storage::StoragePlugin>("@deepseek-ai/dsh-storage")
        .add::<hoh_storage_json::StorageJsonPlugin>("@deepseek-ai/dsh-storage-json")
        .add::<hoh_storage_domain::StorageDomainPlugin>("@deepseek-ai/dsh-storage-domain")
        .add::<hoh_attachment_local::LocalAttachmentStore>("@deepseek-ai/dsh-attachment-local")
        .add::<hoh_deepseek_llm_api_extensions::DeepSeekLlmApiExtensionRegistry>(
            "@deepseek-ai/dsh-deepseek-llm-api-extensions",
        )
        .add::<hoh_web::WebRuntime>("@deepseek-ai/dsh-web")
        .add::<hoh_web_fetch_http::WebFetchHttpPlugin>("@deepseek-ai/dsh-web-fetch-http")
        .add::<hoh_deliverable_file_references::DeliverableFileReferences>(
            "@deepseek-ai/dsh-deliverable-file-references",
        )
        .add::<hoh_subprocess_local::LocalSubprocessRuntime>("@deepseek-ai/dsh-subprocess-local")
        .add::<hoh_bash_local::LocalBashExecutor>("@deepseek-ai/dsh-bash-local")
        .add::<hoh_shell_env::ShellEnvPlugin>("@deepseek-ai/dsh-shell-env")
        .add::<hoh_tools::ToolRuntime>("@deepseek-ai/dsh-tools")
        .add::<hoh_tool_executor_native::NativeToolExecutorPlugin>(
            "@deepseek-ai/dsh-tool-executor-native",
        )
        .add::<hoh_agent::AgentRegistryPlugin>("@deepseek-ai/dsh-agent")
        .add::<hoh_working_directory::WorkingDirectory>("@deepseek-ai/dsh-working-directory")
        .add::<hoh_tool_working_directory::ToolWorkingDirectory>(
            "@deepseek-ai/dsh-tool-working-directory",
        )
        .add::<hoh_tool_fs::ToolFs>("@deepseek-ai/dsh-tool-fs")
        .add::<hoh_tool_bash::ToolBash>("@deepseek-ai/dsh-tool-bash")
        .add::<hoh_user_questions::UserQuestionsPlugin>("@deepseek-ai/dsh-user-questions")
        .add::<hoh_user_approval::ApprovalService>("@deepseek-ai/dsh-user-approval")
        .add::<hoh_tool_ask_user::ToolAskUser>("@deepseek-ai/dsh-tool-ask-user")
        .add::<hoh_tool_todo::ToolTodo>("@deepseek-ai/dsh-tool-todo")
        .add::<hoh_agent_loop::AgentLoopPlugin>("@deepseek-ai/dsh-agent-loop")
        .add::<hoh_sdk_app::SdkApp>("@deepseek-ai/dsh-sdk-app")
        .add::<hoh_sdk_jsonrpc_server::SdkJsonRpcServer>("@deepseek-ai/dsh-sdk-jsonrpc-server")
        .add::<hoh_credentials_local::LocalCredentialProvider>("@deepseek-ai/dsh-credentials-local")
        .add::<hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin>(
            "@deepseek-ai/dsh-session-persistence-jsonl",
        )
        .add::<hoh_sandbox_local::LocalSandboxPlugin>("@deepseek-ai/dsh-sandbox-local")
        .add::<hoh_sandbox_policy::SandboxPolicyService>("@deepseek-ai/dsh-sandbox-policy")
        .add::<hoh_bash_sandbox::SandboxBashExecutor>("@deepseek-ai/dsh-bash-sandbox")
        .add::<hoh_fs_sandbox::FsSandboxPlugin>("@deepseek-ai/dsh-fs-sandbox")
        .add::<hoh_model_host::ModelHostPlugin>("hoh:model-host")
        .add::<super::window_glue::WindowModelSettings>("hoh:window-model-settings")
        .add::<super::window_glue::WindowPrivilegedApprover>("hoh:window-privileged-approver")
        .add::<hoh_window_destructive_guard::DestructiveGuard>("hoh:window-destructive-guard")
        .add::<hoh_destructive_classifier::DestructiveClassifierPlugin>(
            "hoh:destructive-classifier",
        )
        .add::<hoh_external_harness::ExternalHarnessPlugin>("hoh:external-harness")
        .add::<hoh_harness_claude::HarnessClaudePlugin>("hoh:harness-claude")
        .add::<hoh_harness_codex::HarnessCodexPlugin>("hoh:harness-codex")
        .add::<hoh_external_harness::ExternalHarnessWindowPlugin>("hoh:external-harness-window");
    crate::v1_registry::add(&mut registry);
    #[cfg(debug_assertions)]
    if std::env::var_os("HOH_TEST_EXIT_THEN_FAIL").is_some() {
        registry.add::<ExitThenFailFixture>("hoh:test-exit-then-fail");
    }
    registry
}
