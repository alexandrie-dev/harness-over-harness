//! Schema collection never mounts plugins or evaluates entry expressions.
use hoh_error::BoxError;
use hoh_loader::{PluginRegistry, Profile};
use serde_json::Value;
use std::path::{Path, PathBuf};
pub struct SchemaDump {
    pub document: Value,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
pub async fn run_dump_config_schema(
    profile: &Profile,
    patches: &[PathBuf],
    home: &Path,
    cwd: &Path,
    registry: &PluginRegistry,
) -> Result<SchemaDump, BoxError> {
    let layers =
        super::dump_config::collect_config_dump_layers(profile, false, patches, home, cwd)?;
    let document =
        hoh_loader::config_schema::generate_config_schema(profile, &[], &layers, registry).await;
    let mut stderr = String::new();
    if let Some(diagnostics) = document["x-cordis"]["diagnostics"].as_array() {
        for diagnostic in diagnostics {
            let location = diagnostic["path"]
                .as_str()
                .map(|p| format!(" [{p}]"))
                .unwrap_or_default();
            stderr.push_str(&format!(
                "dsh: {}:{location} {}\n",
                diagnostic["level"].as_str().unwrap_or("error"),
                diagnostic["message"].as_str().unwrap_or_default()
            ));
        }
    }
    let stdout = format!(
        "{}\n",
        hoh_util_values::js_json_stringify_pretty(&document, 2)
    );
    let exit_code = i32::from(document["x-cordis"]["complete"] != true);
    Ok(SchemaDump {
        document,
        stdout,
        stderr,
        exit_code,
    })
}
