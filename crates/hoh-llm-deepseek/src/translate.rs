//! Translate Messages events while preserving block order and cumulative usage.

use crate::replay::{ReplayBlock, replay_state};
use hoh_error::HarnessError;
use hoh_llm::{
    BuiltinLlmFailureCode as Code, ContentBlock, FinishReason, LlmError, StreamChunk, TokenUsage,
    ToolCallBlock, ToolCallId,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

struct Block {
    index: u64,
    content: ContentBlock,
    replay: ReplayBlock,
    closed: bool,
    json: String,
}

fn malformed(detail: impl Into<String>) -> HarnessError {
    LlmError::new(
        format!("DeepSeek Messages stream: {}", detail.into()),
        Code::MalformedResponse,
    )
    .expect("stream diagnostic is valid")
}
fn required_object(value: Option<&Value>) -> Result<&Map<String, Value>, HarnessError> {
    value.and_then(Value::as_object).ok_or_else(|| {
        LlmError::new(
            "DeepSeek Messages expected a JSON object",
            Code::MalformedResponse,
        )
        .expect("object diagnostic is valid")
    })
}
fn required_string(value: Option<&Value>) -> Result<&str, HarnessError> {
    value.and_then(Value::as_str).ok_or_else(|| {
        LlmError::new(
            "DeepSeek Messages expected a string field",
            Code::MalformedResponse,
        )
        .expect("string diagnostic is valid")
    })
}
fn index(event: &Map<String, Value>) -> Result<u64, HarnessError> {
    event
        .get("index")
        .and_then(Value::as_u64)
        .filter(|value| *value <= 9_007_199_254_740_991)
        .ok_or_else(|| malformed("invalid block index"))
}

#[derive(Default)]
pub struct Translator {
    model: String,
    blocks: HashMap<u64, Block>,
    order: Vec<u64>,
    usage: TokenUsage,
    started: bool,
    reason: Option<FinishReason>,
    finished: bool,
}

impl Translator {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Default::default()
        }
    }

    fn update_usage(&mut self, raw: Option<&Value>) -> Result<(), HarnessError> {
        let Some(raw) = raw else { return Ok(()) };
        let fields = raw.as_object().ok_or_else(|| {
            LlmError::new(
                "DeepSeek Messages expected a JSON object",
                Code::MalformedResponse,
            )
            .expect("usage diagnostic is valid")
        })?;
        for (wire, target) in [
            ("input_tokens", 0),
            ("output_tokens", 1),
            ("cache_read_input_tokens", 2),
            ("cache_creation_input_tokens", 3),
        ] {
            let Some(value) = fields.get(wire) else {
                continue;
            };
            let Some(value) = value.as_u64() else {
                return Err(malformed(format!("invalid {wire}")));
            };
            if value > 9_007_199_254_740_991 {
                return Err(malformed(format!("invalid {wire}")));
            }
            match target {
                0 => self.usage.input_tokens = value,
                1 => self.usage.output_tokens = value,
                2 => self.usage.cache_read_tokens = Some(value),
                _ => self.usage.cache_write_tokens = Some(value),
            }
        }
        Ok(())
    }

    pub fn feed(&mut self, raw: &Value) -> Result<Vec<StreamChunk>, HarnessError> {
        if self.finished {
            return Err(malformed("event follows message_stop"));
        }
        let event = raw.as_object().ok_or_else(|| {
            LlmError::new(
                "DeepSeek Messages expected a JSON object",
                Code::MalformedResponse,
            )
            .expect("event diagnostic is valid")
        })?;
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        let mut out = Vec::new();
        if event_type == "message_start" {
            if self.started {
                return Err(malformed("duplicate message_start"));
            }
            let message = required_object(event.get("message"))?;
            let usage = required_object(message.get("usage"))?;
            self.update_usage(Some(&Value::Object(usage.clone())))?;
            self.started = true;
            return Ok(out);
        }
        if !matches!(
            event_type,
            "content_block_start"
                | "content_block_delta"
                | "content_block_stop"
                | "message_delta"
                | "message_stop"
        ) {
            return Ok(out);
        }
        if !self.started {
            return Err(malformed("event precedes message_start"));
        }
        match event_type {
            "content_block_start" => {
                let wire_index = index(event)?;
                if self.blocks.contains_key(&wire_index) || self.reason.is_some() {
                    return Err(malformed(
                        "block starts after settlement or repeats an index",
                    ));
                }
                let native = required_object(event.get("content_block"))?;
                let block_index = self.blocks.len() as u64;
                let native_type = native.get("type").and_then(Value::as_str).unwrap_or("");
                let (content, replay) = match native_type {
                    "text" => (
                        ContentBlock::text(required_string(native.get("text"))?),
                        ReplayBlock {
                            r#type: "text".into(),
                            signature: None,
                        },
                    ),
                    "thinking" => (
                        ContentBlock::reasoning(required_string(native.get("thinking"))?),
                        ReplayBlock {
                            r#type: "reasoning".into(),
                            signature: native
                                .get("signature")
                                .map(|value| required_string(Some(value)).map(str::to_owned))
                                .transpose()?,
                        },
                    ),
                    "tool_use" => {
                        let id = required_string(native.get("id"))?;
                        let name = required_string(native.get("name"))?;
                        if id.is_empty() || name.is_empty() {
                            return Err(malformed("empty tool identity"));
                        }
                        let input = required_object(native.get("input"))?;
                        (
                            ToolCallBlock {
                                id: ToolCallId::from(id),
                                name: name.into(),
                                arguments: Value::Object(input.clone()).to_string(),
                            }
                            .into(),
                            ReplayBlock {
                                r#type: "tool-call".into(),
                                signature: None,
                            },
                        )
                    }
                    other => {
                        return Err(LlmError::new(
                            format!("DeepSeek Messages does not support response block {other}"),
                            Code::UnsupportedContent,
                        )
                        .expect("block diagnostic is valid"));
                    }
                };
                out.push(StreamChunk::BlockStart {
                    index: block_index,
                    block_type: content.r#type.clone(),
                    wire: Default::default(),
                });
                match content.r#type.as_str() {
                    "text" | "reasoning" => {
                        if let Some(text) = content.text_value().filter(|text| !text.is_empty()) {
                            out.push(if content.r#type == "text" {
                                StreamChunk::TextDelta {
                                    index: block_index,
                                    text: text.into(),
                                    wire: Default::default(),
                                }
                            } else {
                                StreamChunk::ReasoningDelta {
                                    index: block_index,
                                    text: text.into(),
                                    wire: Default::default(),
                                }
                            });
                        }
                    }
                    "tool-call" => out.push(StreamChunk::ToolCallDelta {
                        index: block_index,
                        id: content.data["id"].as_str().unwrap_or_default().into(),
                        name: content.data["name"].as_str().map(str::to_owned),
                        arguments_delta: String::new(),
                        wire: Default::default(),
                    }),
                    _ => unreachable!(),
                }
                self.order.push(wire_index);
                self.blocks.insert(
                    wire_index,
                    Block {
                        index: block_index,
                        content,
                        replay,
                        closed: false,
                        json: String::new(),
                    },
                );
            }
            "content_block_delta" => {
                let wire_index = index(event)?;
                let block = self
                    .blocks
                    .get_mut(&wire_index)
                    .filter(|block| !block.closed)
                    .ok_or_else(|| malformed("delta/stop without an open block"))?;
                let delta = required_object(event.get("delta"))?;
                let delta_type = delta.get("type").and_then(Value::as_str).unwrap_or("");
                match (delta_type, block.content.r#type.as_str()) {
                    ("text_delta", "text") => {
                        let text = required_string(delta.get("text"))?.to_owned();
                        let mut all = block.content.text_value().unwrap_or_default().to_owned();
                        all.push_str(&text);
                        block.content.data.insert("text".into(), Value::String(all));
                        out.push(StreamChunk::TextDelta {
                            index: block.index,
                            text,
                            wire: Default::default(),
                        });
                    }
                    ("thinking_delta", "reasoning") => {
                        let text = required_string(delta.get("thinking"))?.to_owned();
                        let mut all = block.content.text_value().unwrap_or_default().to_owned();
                        all.push_str(&text);
                        block.content.data.insert("text".into(), Value::String(all));
                        out.push(StreamChunk::ReasoningDelta {
                            index: block.index,
                            text,
                            wire: Default::default(),
                        });
                    }
                    ("signature_delta", "reasoning") => {
                        let text = required_string(delta.get("signature"))?;
                        block
                            .replay
                            .signature
                            .get_or_insert_with(String::new)
                            .push_str(text);
                    }
                    ("input_json_delta", "tool-call") => {
                        let text = required_string(delta.get("partial_json"))?.to_owned();
                        block.json.push_str(&text);
                        out.push(StreamChunk::ToolCallDelta {
                            index: block.index,
                            id: block.content.data["id"].as_str().unwrap_or_default().into(),
                            name: None,
                            arguments_delta: text,
                            wire: Default::default(),
                        });
                    }
                    _ => {
                        return Err(malformed(format!(
                            "unsupported delta {delta_type} for {}",
                            block.content.r#type
                        )));
                    }
                }
            }
            "content_block_stop" => {
                let wire_index = index(event)?;
                let block = self
                    .blocks
                    .get_mut(&wire_index)
                    .filter(|block| !block.closed)
                    .ok_or_else(|| malformed("delta/stop without an open block"))?;
                block.closed = true;
                if block.content.r#type == "tool-call" && !block.json.is_empty() {
                    block
                        .content
                        .data
                        .insert("arguments".into(), Value::String(block.json.clone()));
                }
                out.push(StreamChunk::BlockEnd {
                    index: block.index,
                    block: block.content.clone(),
                    wire: Default::default(),
                });
            }
            "message_delta" => {
                let delta = required_object(event.get("delta"))?;
                if let Some(raw_reason) = delta.get("stop_reason").filter(|value| !value.is_null())
                {
                    let Some(reason) = raw_reason.as_str() else {
                        return Err(malformed(format!("unsupported stop reason {raw_reason}")));
                    };
                    let kind = match reason {
                        "end_turn" | "stop_sequence" => "stop",
                        "tool_use" => "tool-calls",
                        "max_tokens" => "max-tokens",
                        other => return Err(malformed(format!("unsupported stop reason {other}"))),
                    };
                    self.reason = Some(FinishReason::new(kind));
                }
                self.update_usage(event.get("usage"))?;
            }
            "message_stop" => {
                let reason = self.reason.clone().ok_or_else(|| {
                    malformed("message_stop without settled blocks and stop reason")
                })?;
                if self.blocks.values().any(|block| !block.closed) {
                    return Err(malformed(
                        "message_stop without settled blocks and stop reason",
                    ));
                }
                if self.blocks.is_empty() && reason.kind == "stop" {
                    return Err(LlmError::new(
                        "DeepSeek Messages returned no content",
                        Code::EmptyResponse,
                    )
                    .expect("empty diagnostic is valid"));
                }
                if reason.kind != "max-tokens" {
                    for block in self
                        .blocks
                        .values()
                        .filter(|block| block.content.r#type == "tool-call")
                    {
                        let value: Value = serde_json::from_str(
                            block.content.data["arguments"].as_str().unwrap_or_default(),
                        )
                        .map_err(|_| malformed("tool input is invalid JSON"))?;
                        if !value.is_object() {
                            return Err(malformed("tool input is invalid JSON"));
                        }
                    }
                }
                self.usage.total_tokens = Some(
                    self.usage.input_tokens
                        + self.usage.output_tokens
                        + self.usage.cache_read_tokens.unwrap_or(0)
                        + self.usage.cache_write_tokens.unwrap_or(0),
                );
                out.push(StreamChunk::Usage {
                    usage: self.usage.clone(),
                    wire: Default::default(),
                });
                let replay = self
                    .order
                    .iter()
                    .filter_map(|index| self.blocks.get(index).map(|block| block.replay.clone()))
                    .collect::<Vec<_>>();
                out.push(StreamChunk::Finish {
                    reason,
                    replay_state: Some(replay_state(&self.model, &replay)),
                    wire: Default::default(),
                });
                self.finished = true;
            }
            _ => unreachable!(),
        }
        Ok(out)
    }

    pub fn finish(&self) -> Result<(), HarnessError> {
        if self.finished {
            Ok(())
        } else {
            Err(LlmError::new(
                "DeepSeek Messages stream ended before message_stop",
                Code::StreamClosed,
            )
            .expect("stream close diagnostic is valid"))
        }
    }
}
