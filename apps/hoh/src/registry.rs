//! Compiled plugin table. Missing packages remain unresolved, never stand-in plugins.
use hoh_kernel::{Cfg, Context, Error, NoConfig, Plugin, Service};
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
        );
    registry
}
