use crate::{Result, error};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayModelConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_request_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_update: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_update: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<hoh_llm::LlmModelBadge>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayProviderConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ReplayModelConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_policy: Option<Value>,
}
#[derive(Clone, Debug, Default)]
pub struct ReplayConfig {
    pub file: String,
    pub override_file: Option<String>,
    pub child_files: Option<Vec<String>>,
    pub providers: Option<Vec<ReplayProviderConfig>>,
    pub pace_ms: Option<f64>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_files: Option<Vec<String>>,
    /// Kept raw until source-specific model diagnostics have run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace_ms: Option<Value>,
}
impl Config {
    pub fn resolve(self) -> Result<ReplayConfig> {
        self.resolve_with_env(|key| std::env::var(key).ok())
    }
    /// Deterministic environment seam; the plugin uses resolve().
    pub fn resolve_with_env(self, env: impl Fn(&str) -> Option<String>) -> Result<ReplayConfig> {
        let file = self
            .file
            .or_else(|| env("DSH_SNAPSHOT_FILE"))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                error("llm-replay: a fixture path is required (Config.file or $DSH_SNAPSHOT_FILE)")
            })?;
        if let Some(providers) = &self.providers {
            validate_configured_models(providers)?;
        }
        let providers = self.providers.map(serde_json::from_value).transpose()?;
        let override_file = self
            .override_file
            .or_else(|| env("DSH_SNAPSHOT_OVERRIDE"))
            .filter(|s| !s.is_empty());
        let child_files = self.child_files.unwrap_or_else(|| {
            env("DSH_SNAPSHOT_CHILD_FILES")
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.split(if cfg!(windows) { ';' } else { ':' })
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        });
        let pace_ms = self
            .pace_ms
            .map(|v| {
                v.as_f64().ok_or_else(|| {
                    error(format!(
                        "llm-replay: paceMs must be a non-negative integer, got {}",
                        hoh_util_values::js_string(&v)
                    ))
                })
            })
            .transpose()?;
        Ok(ReplayConfig {
            file,
            override_file,
            child_files: (!child_files.is_empty()).then_some(child_files),
            providers,
            pace_ms,
        })
    }
}
fn validate_configured_models(providers: &Value) -> Result<()> {
    let providers = providers
        .as_array()
        .ok_or_else(|| error("llm-replay: providers must be an array"))?;
    for provider in providers {
        for model in provider["models"].as_array().into_iter().flatten() {
            let prefix = format!(
                "llm-replay: provider \"{}\" model \"{}\"",
                provider["id"].as_str().unwrap_or(""),
                model["id"].as_str().unwrap_or("")
            );
            let fail = |detail: &str| Err(error(format!("{prefix} {detail}")));
            if let Some(modalities) = model.get("inputModalities")
                && !modalities
                    .as_array()
                    .is_some_and(|a| a.iter().all(|v| v == "text" || v == "image"))
            {
                return fail(
                    "inputModalities must be an array containing only \"text\" and \"image\"",
                );
            }
            if let Some(tokens) = model.get("imageRequestTokens") {
                if !tokens
                    .as_u64()
                    .is_some_and(|n| n > 0 && n <= 9_007_199_254_740_991)
                {
                    return fail("imageRequestTokens must be a positive safe integer");
                }
                if !model["inputModalities"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == "image"))
                {
                    return fail(
                        "imageRequestTokens requires inputModalities to include \"image\"",
                    );
                }
            }
            if model
                .get("systemPromptUpdate")
                .is_some_and(|v| v != "in-history")
            {
                return fail("systemPromptUpdate must be \"in-history\" when present");
            }
            if model
                .get("toolUpdate")
                .is_some_and(|v| v != "in-history" && v != "addition-only")
            {
                return fail("toolUpdate must be \"in-history\" or \"addition-only\" when present");
            }
            if let Some(badge) = model.get("badge") {
                let texts = if let Some(text) = badge["text"].as_str() {
                    vec![Value::String(text.into())]
                } else if let Some(texts) =
                    badge["text"].as_object().filter(|o| o.contains_key("en"))
                {
                    texts.values().cloned().collect()
                } else {
                    vec![Value::Null]
                };
                if !["neutral", "info", "success", "warning"]
                    .iter()
                    .any(|tone| badge["tone"] == *tone)
                    || !texts.iter().all(|v| {
                        v.as_str()
                            .is_some_and(|s| !hoh_util_values::js_trim(s).is_empty())
                    })
                {
                    return fail(
                        "badge needs non-blank text or translations with an \"en\" entry, and a neutral, info, success, or warning tone",
                    );
                }
            }
        }
    }
    Ok(())
}
