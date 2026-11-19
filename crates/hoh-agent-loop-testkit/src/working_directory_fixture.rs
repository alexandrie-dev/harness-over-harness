use crate::mount;
use hoh_error::BoxError;
use hoh_fs::Fs;
use hoh_fs_local::FsLocalPlugin;
use hoh_kernel::Context;
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use hoh_system_prompt::{SystemPrompt, SystemPromptKey};
use hoh_working_directory::{WorkingDirectory, WorkingDirectoryKey};
use serde_json::json;

/// Provide the exact production working-directory service key for structural tests.
///
/// The owner crate has no public fixture constructor, so this mounts the production service over
/// a local filesystem. Reads match the source fixture; `ensure` additionally validates the path.
pub async fn provide_working_directory_fixture(
    ctx: &Context,
    default_directory: Option<&str>,
) -> Result<(), BoxError> {
    let default_directory = match default_directory {
        Some(directory) => directory.to_owned(),
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };
    if ctx.get::<SessionProjections>().is_none() {
        mount::<SessionProjectionRegistry>(ctx, json!({})).await?;
    }
    if ctx.get::<SystemPromptKey>().is_none() {
        mount::<SystemPrompt>(ctx, json!({})).await?;
    }
    if ctx.get::<Fs>().is_none() {
        mount::<FsLocalPlugin>(ctx, json!({"cwd":default_directory})).await?;
    }
    if ctx.get::<WorkingDirectoryKey>().is_none() {
        mount::<WorkingDirectory>(ctx, json!({"defaultDirectory":default_directory})).await?;
    }
    Ok(())
}
