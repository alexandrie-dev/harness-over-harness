//! `snapshots/sdk/manual-compact/capture-requests.mjs`: record the isolated scenario's model inputs
//! before adapter execution. Five SDK scenarios load it under a `capture.cordis.yml`.
use super::SNAPSHOTS;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Plugin};
use hoh_llm::LlmStreamEvent;
use hoh_loader::PluginRegistry;
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CaptureConfig {
    /// Output path under the scenario's private DSH home.
    pub path: String,
}

/// Keys of one captured line, in the order the TypeScript object literal lists them.
const KEYS: [&str; 7] = [
    "provider",
    "model",
    "purpose",
    "maxTokens",
    "reasoningEffort",
    "messages",
    "tools",
];

pub struct CaptureRequests;
impl Plugin for CaptureRequests {
    const NAME: &'static str = "capture-requests";
    type Config = CaptureConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), config: Cfg<CaptureConfig>) -> Result<(), BoxError> {
        ctx.listen()
            .on_waterfall::<LlmStreamEvent, hoh_llm::LlmStream, _>(move |event, next| {
                let path = config.get().path.clone();
                let written = write_line(&path, &event.options());
                async move {
                    written?;
                    next.run().await
                }
            })?;
        Ok(())
    }
}

fn write_line(path: &str, options: &hoh_llm::GenerateOptions) -> Result<(), BoxError> {
    let all = serde_json::to_value(options)?;
    // `JSON.stringify` drops `undefined`; the serialized options omit absent fields the same way.
    let line: serde_json::Map<String, serde_json::Value> = KEYS
        .iter()
        .filter_map(|key| all.get(*key).map(|v| ((*key).to_owned(), v.clone())))
        .collect();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(
        file,
        "{}",
        hoh_util_values::js_json_stringify(&serde_json::Value::Object(line))
    )?;
    Ok(())
}

pub fn add(registry: &mut PluginRegistry) {
    registry.add::<CaptureRequests>(&format!(
        "{SNAPSHOTS}sdk/manual-compact/capture-requests.mjs"
    ));
}
