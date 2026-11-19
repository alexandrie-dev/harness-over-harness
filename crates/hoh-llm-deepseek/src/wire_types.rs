//! DeepSeek's supported subset of the Anthropic Messages request protocol.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type WireBlock = Value;
pub type WireInput = Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireMessage {
    pub role: String,
    pub content: Vec<WireBlock>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireThinking {
    #[serde(rename = "type")]
    pub r#type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireOutputConfig {
    pub effort: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireTool {
    pub name: String,
    pub description: String,
    pub input_schema: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireRequest {
    pub model: String,
    pub stream: bool,
    pub max_tokens: u64,
    pub messages: Vec<WireMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub thinking: WireThinking,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_config: Option<WireOutputConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<WireTool>>,
}
