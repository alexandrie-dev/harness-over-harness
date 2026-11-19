//! Minimal native thinking metadata retained with durable assistant blocks.

use hoh_error::HarnessError;
use hoh_llm::{AssistantMessage, BuiltinLlmFailureCode as Code, LlmError, ReplayEnvelope};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayBlock {
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

fn invalid(detail: &str) -> HarnessError {
    LlmError::new(
        format!("DeepSeek Messages replay: {detail}"),
        Code::InvalidReplayState,
    )
    .expect("replay diagnostic is valid")
}

pub fn object(value: &Value, code: Code) -> Result<&Map<String, Value>, HarnessError> {
    value.as_object().ok_or_else(|| {
        LlmError::new("DeepSeek Messages expected a JSON object", code)
            .expect("object diagnostic is valid")
    })
}

pub fn replay_state(model: &str, blocks: &[ReplayBlock]) -> ReplayEnvelope {
    ReplayEnvelope {
        response: json!({"kind":"deepseek-messages","version":1,"model":model}),
        blocks: Some(
            blocks
                .iter()
                .map(|block| serde_json::to_value(block).expect("replay block is JSON"))
                .collect(),
        ),
        wire: Default::default(),
    }
}

pub fn read_replay(
    message: &AssistantMessage,
    model: &str,
    mut on_degrade: Option<&mut dyn FnMut(&str)>,
) -> Option<Vec<ReplayBlock>> {
    match validate_replay(message, model) {
        Ok(value) => value,
        Err(error) => {
            if let Some(callback) = &mut on_degrade {
                callback(error.message());
            }
            None
        }
    }
}

fn validate_replay(
    message: &AssistantMessage,
    model: &str,
) -> Result<Option<Vec<ReplayBlock>>, HarnessError> {
    let Some(envelope) = &message.source.metadata.replay_state else {
        return Ok(None);
    };
    let envelope = object(envelope, Code::InvalidReplayState)?;
    let response = object(
        envelope
            .get("response")
            .ok_or_else(|| invalid("unsupported kind or version"))?,
        Code::InvalidReplayState,
    )?;
    if response.get("kind").and_then(Value::as_str) != Some("deepseek-messages")
        || response.get("version").and_then(Value::as_u64) != Some(1)
    {
        return Err(invalid("unsupported kind or version"));
    }
    let source_model = response.get("model").and_then(Value::as_str);
    if source_model != Some(message.source.metadata.model.as_str()) {
        return Err(invalid("model does not match assistant source model"));
    }
    let blocks = envelope
        .get("blocks")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("block count mismatch"))?;
    if blocks.len() != message.content.len() {
        return Err(invalid("block count mismatch"));
    }
    let mut decoded = Vec::with_capacity(blocks.len());
    for (value, content) in blocks.iter().zip(&message.content) {
        let block: ReplayBlock =
            serde_json::from_value(value.clone()).map_err(|_| invalid("block type mismatch"))?;
        if block.r#type != content.r#type
            || !matches!(block.r#type.as_str(), "text" | "reasoning" | "tool-call")
        {
            return Err(invalid("block type mismatch"));
        }
        if block.signature.is_some() && block.r#type != "reasoning" {
            return Err(invalid("invalid signature"));
        }
        decoded.push(block);
    }
    Ok((source_model == Some(model)).then_some(decoded))
}
