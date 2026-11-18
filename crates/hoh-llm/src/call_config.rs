use crate::types::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmCallControls {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffortId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmCallConfig {
    #[serde(flatten)]
    pub controls: LlmCallControls,
    pub provider: String,
    pub model: String,
}
pub type ConfigureCall<'a> = dyn Fn(&LlmCallControls, &LlmResolvedModelInfo) -> Result<LlmCallControls, hoh_error::BoxError>
    + Send
    + Sync
    + 'a;
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmCallConfigAdapterDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<bool>,
}
pub fn call_config_equals(a: &LlmCallConfig, b: &LlmCallConfig) -> bool {
    a == b
}
pub fn mark_agent_loop_request(request: &mut GenerateOptions) -> &mut GenerateOptions {
    request.agent_loop_request = true;
    request
}
pub fn is_agent_loop_request(request: &GenerateOptions) -> bool {
    request.agent_loop_request
}
impl GenerateOptions {
    pub fn call_config(&self) -> LlmCallConfig {
        LlmCallConfig {
            provider: self.provider.clone(),
            model: self.model.clone(),
            controls: LlmCallControls {
                reasoning_effort: self.reasoning_effort.clone(),
                temperature: self.temperature,
                max_tokens: self.max_tokens,
                stop: self.stop.clone(),
            },
        }
    }
    pub fn with_call_config(mut self, config: &LlmCallConfig) -> Self {
        self.provider.clone_from(&config.provider);
        self.model.clone_from(&config.model);
        self.reasoning_effort
            .clone_from(&config.controls.reasoning_effort);
        self.temperature = config.controls.temperature;
        self.max_tokens = config.controls.max_tokens;
        self.stop.clone_from(&config.controls.stop);
        self
    }
}
