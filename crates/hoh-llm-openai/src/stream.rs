//! Chat Completions stream events → `hoh-llm` `StreamChunk`s.
//!
//! Block order follows arrival: a reasoning block, a text block and each tool call are separate
//! blocks, numbered in order of first appearance. Tool calls are assembled by the server's
//! `index`; their arguments are the concatenation of the argument deltas.
use crate::compat::{Compat, ToolCallIds};
use crate::config::{OpenAiConfig, ReasoningFormat};
use crate::error::{classify, error_fields, failure, truncate};
use crate::think::{Segment, ThinkTagParser};
use crate::tool_check::{check_tool_call, normalize_arguments};
use hoh_error::HarnessError;
use hoh_llm::{
    BuiltinLlmFailureCode as Code, ContentBlock, FinishReason, StreamChunk, TokenUsage,
    ToolCallBlock, ToolCallId, ToolSchema,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Text,
    Reasoning,
}

enum Kind {
    Text(TextKind),
    Tool {
        wire_index: i64,
        server_id: Option<String>,
        id: String,
        name: String,
    },
}

struct Block {
    kind: Kind,
    /// Text of a text/reasoning block, arguments of a tool call.
    content: String,
}

pub struct Translator {
    compat: &'static Compat,
    strict: bool,
    tools: Option<Vec<ToolSchema>>,
    think: Option<ThinkTagParser>,
    blocks: Vec<Block>,
    open_text: Option<usize>,
    tool_by_wire_index: HashMap<i64, usize>,
    last_tool: Option<usize>,
    used_ids: HashSet<String>,
    finish_reason: Option<String>,
    usage: Option<TokenUsage>,
}

impl Translator {
    pub fn new(config: &OpenAiConfig, tools: Option<Vec<ToolSchema>>) -> Self {
        let think = match config.reasoning_format {
            ReasoningFormat::Auto | ReasoningFormat::ThinkTags => Some(ThinkTagParser::new(false)),
            ReasoningFormat::ThinkTagsImplicitOpen => Some(ThinkTagParser::new(true)),
            ReasoningFormat::ReasoningField => None,
        };
        Self {
            compat: config.compat(),
            strict: config.strict_tool_arguments,
            tools,
            think,
            blocks: Vec::new(),
            open_text: None,
            tool_by_wire_index: HashMap::new(),
            last_tool: None,
            used_ids: HashSet::new(),
            finish_reason: None,
            usage: None,
        }
    }

    /// One decoded `data:` payload (or, for a non-streaming reply, the whole response object).
    pub fn feed(&mut self, event: &Value) -> Result<Vec<StreamChunk>, HarnessError> {
        let mut out = Vec::new();
        if event.get("choices").is_none()
            && let Some(fields) = error_fields(event)
        {
            return Err(classify(&fields, None, None, None));
        }
        if let Some(usage) = event.get("usage").and_then(parse_usage) {
            self.usage = Some(usage);
        }
        let Some(choice) = event
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| {
                choices
                    .iter()
                    .find(|c| c.get("index").and_then(Value::as_u64).unwrap_or(0) == 0)
            })
        else {
            return Ok(out);
        };
        if let Some(delta) = choice.get("delta").or_else(|| choice.get("message")) {
            self.delta(delta, &mut out)?;
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_owned());
        }
        Ok(out)
    }

    fn delta(&mut self, delta: &Value, out: &mut Vec<StreamChunk>) -> Result<(), HarnessError> {
        let reasoning = self
            .compat
            .reasoning_fields
            .iter()
            .filter_map(|field| delta.get(*field).and_then(Value::as_str))
            .find(|text| !text.is_empty());
        if let Some(text) = reasoning {
            self.push_text(TextKind::Reasoning, text, out);
        }
        if let Some(content) = content_text(delta.get("content")) {
            match &mut self.think {
                Some(parser) => {
                    for segment in parser.push(&content) {
                        self.push_segment(segment, out);
                    }
                }
                None => self.push_text(TextKind::Text, &content, out),
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                self.tool_delta(call, out)?;
            }
        }
        if let Some(call) = delta.get("function_call").filter(|c| c.is_object()) {
            // Legacy single function call: one call, index 0.
            let call = serde_json::json!({"index": 0, "function": call});
            self.tool_delta(&call, out)?;
        }
        Ok(())
    }

    fn push_segment(&mut self, segment: Segment, out: &mut Vec<StreamChunk>) {
        match segment {
            Segment::Text(text) => self.push_text(TextKind::Text, &text, out),
            Segment::Reasoning(text) => self.push_text(TextKind::Reasoning, &text, out),
        }
    }

    fn push_text(&mut self, kind: TextKind, text: &str, out: &mut Vec<StreamChunk>) {
        if text.is_empty() {
            return;
        }
        let reuse = self
            .open_text
            .filter(|pos| matches!(&self.blocks[*pos].kind, Kind::Text(k) if *k == kind));
        let pos = match reuse {
            Some(pos) => pos,
            None => {
                self.close_open(out);
                let pos = self.blocks.len();
                self.blocks.push(Block {
                    kind: Kind::Text(kind),
                    content: String::new(),
                });
                self.open_text = Some(pos);
                out.push(StreamChunk::BlockStart {
                    index: pos as u64,
                    block_type: block_type(kind).into(),
                    wire: Default::default(),
                });
                pos
            }
        };
        self.blocks[pos].content.push_str(text);
        let (index, text) = (pos as u64, text.to_owned());
        out.push(match kind {
            TextKind::Text => StreamChunk::TextDelta {
                index,
                text,
                wire: Default::default(),
            },
            TextKind::Reasoning => StreamChunk::ReasoningDelta {
                index,
                text,
                wire: Default::default(),
            },
        });
    }

    fn close_open(&mut self, out: &mut Vec<StreamChunk>) {
        let Some(pos) = self.open_text.take() else {
            return;
        };
        let Kind::Text(kind) = &self.blocks[pos].kind else {
            return;
        };
        let text = self.blocks[pos].content.clone();
        out.push(StreamChunk::BlockEnd {
            index: pos as u64,
            block: match kind {
                TextKind::Text => ContentBlock::text(text),
                TextKind::Reasoning => ContentBlock::reasoning(text),
            },
            wire: Default::default(),
        });
    }

    fn tool_delta(&mut self, call: &Value, out: &mut Vec<StreamChunk>) -> Result<(), HarnessError> {
        let function = call.get("function");
        let name = function
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let arguments = match function.and_then(|f| f.get("arguments")) {
            Some(Value::String(text)) => text.clone(),
            // Some servers deliver a whole arguments object instead of a string.
            Some(object @ Value::Object(_)) => object.to_string(),
            None | Some(Value::Null) => String::new(),
            // An array, number or boolean is not what any server documents: the call is damaged.
            Some(other) if self.strict => {
                return Err(failure(
                    Code::MalformedResponse,
                    format!(
                        "Tool call {name:?} has arguments that are a JSON {}, not a string of JSON. Arguments received: {}",
                        json_kind(other),
                        truncate(&other.to_string(), 500)
                    ),
                ));
            }
            Some(other) => other.to_string(),
        };
        let server_id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let wire_index = call
            .get("index")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| self.guess_wire_index(server_id));
        let Some(&pos) = self.tool_by_wire_index.get(&wire_index) else {
            self.start_tool(wire_index, server_id, name, &arguments, out);
            return Ok(());
        };
        let Kind::Tool {
            id, name: known, ..
        } = &mut self.blocks[pos].kind
        else {
            return Ok(());
        };
        // The name is the concatenation of its fragments, like the arguments. A server that
        // repeats the whole name on every delta adds nothing; only the first fragment is announced.
        let announce = if known.is_empty() && !name.is_empty() {
            known.push_str(name);
            Some(name.to_owned())
        } else {
            if !name.is_empty() && name != known.as_str() {
                known.push_str(name);
            }
            None
        };
        if announce.is_none() && arguments.is_empty() {
            return Ok(());
        }
        let id = ToolCallId::from(id.clone());
        self.blocks[pos].content.push_str(&arguments);
        out.push(StreamChunk::ToolCallDelta {
            index: pos as u64,
            id,
            name: announce,
            arguments_delta: arguments,
            wire: Default::default(),
        });
        Ok(())
    }

    /// A tool call without `index`: a new id starts a new call, otherwise it continues the last.
    fn guess_wire_index(&self, server_id: Option<&str>) -> i64 {
        let last = self.last_tool.and_then(|pos| match &self.blocks[pos].kind {
            Kind::Tool {
                wire_index,
                server_id,
                ..
            } => Some((*wire_index, server_id.clone())),
            Kind::Text(_) => None,
        });
        match (last, server_id) {
            (None, _) => 0,
            (Some((index, previous)), Some(id)) if previous.as_deref() != Some(id) => index + 1,
            (Some((index, _)), _) => index,
        }
    }

    fn start_tool(
        &mut self,
        wire_index: i64,
        server_id: Option<&str>,
        name: &str,
        arguments: &str,
        out: &mut Vec<StreamChunk>,
    ) {
        self.close_open(out);
        let trusted = match self.compat.tool_call_ids {
            ToolCallIds::Trust => server_id.filter(|id| !self.used_ids.contains(*id)),
            ToolCallIds::Synthesize => None,
        };
        let id = trusted.map(str::to_owned).unwrap_or_else(|| {
            loop {
                let candidate = format!(
                    "call_{}",
                    &hoh_util_crypto::random_uuid().as_str().replace('-', "")[..24]
                );
                if !self.used_ids.contains(&candidate) {
                    break candidate;
                }
            }
        });
        self.used_ids.insert(id.clone());
        let pos = self.blocks.len();
        self.blocks.push(Block {
            kind: Kind::Tool {
                wire_index,
                server_id: server_id.map(str::to_owned),
                id: id.clone(),
                name: name.to_owned(),
            },
            content: arguments.to_owned(),
        });
        self.tool_by_wire_index.insert(wire_index, pos);
        self.last_tool = Some(pos);
        out.push(StreamChunk::BlockStart {
            index: pos as u64,
            block_type: "tool-call".into(),
            wire: Default::default(),
        });
        out.push(StreamChunk::ToolCallDelta {
            index: pos as u64,
            id: ToolCallId::from(id),
            name: (!name.is_empty()).then(|| name.to_owned()),
            arguments_delta: arguments.to_owned(),
            wire: Default::default(),
        });
    }

    /// End of input. `done` is true when `[DONE]` (or a complete non-streaming reply) was seen.
    pub fn finish(&mut self, done: bool) -> Result<Vec<StreamChunk>, HarnessError> {
        let mut out = Vec::new();
        if let Some(mut parser) = self.think.take() {
            for segment in parser.finish() {
                self.push_segment(segment, &mut out);
            }
            self.think = Some(parser);
        }
        let reason = match self.finish_reason.take() {
            Some(reason) => reason,
            None if done => "stop".to_owned(),
            None => {
                return Err(failure(
                    Code::StreamClosed,
                    "OpenAI-compatible stream ended before the response finished".into(),
                ));
            }
        };
        self.close_open(&mut out);
        let has_tools = self
            .blocks
            .iter()
            .any(|b| matches!(b.kind, Kind::Tool { .. }));
        let kind = match reason.as_str() {
            "length" => "max-tokens",
            "tool_calls" | "function_call" | "stop" | "eos" | "end_turn" | "stop_sequence" => {
                if has_tools {
                    "tool-calls"
                } else {
                    "stop"
                }
            }
            "content_filter" => {
                return Err(failure(
                    Code::InvalidResponse,
                    "OpenAI-compatible server stopped the response with a content filter".into(),
                ));
            }
            "abort" => {
                return Err(failure(
                    Code::StreamClosed,
                    "OpenAI-compatible server aborted the generation".into(),
                ));
            }
            other => {
                return Err(failure(
                    Code::MalformedResponse,
                    format!("OpenAI-compatible stream: unsupported finish reason {other:?}"),
                ));
            }
        };
        if self.blocks.is_empty() && kind == "stop" {
            return Err(failure(
                Code::EmptyResponse,
                "OpenAI-compatible server returned no content".into(),
            ));
        }
        for pos in 0..self.blocks.len() {
            let Kind::Tool { id, name, .. } = &self.blocks[pos].kind else {
                continue;
            };
            let arguments = normalize_arguments(&self.blocks[pos].content);
            // A truncated call is pruned by the shared assembler; checking it would only hide `length`.
            if self.strict
                && kind != "max-tokens"
                && let Err(reason) = check_tool_call(name, &arguments, self.tools.as_deref())
            {
                return Err(failure(
                    Code::MalformedResponse,
                    format!(
                        "Tool call {name:?} (id {id}) has {reason}. Arguments received: {}",
                        truncate(&self.blocks[pos].content, 500)
                    ),
                ));
            }
            out.push(StreamChunk::BlockEnd {
                index: pos as u64,
                block: ToolCallBlock {
                    id: ToolCallId::from(id.clone()),
                    name: name.clone(),
                    arguments,
                }
                .into(),
                wire: Default::default(),
            });
        }
        if let Some(usage) = self.usage.take() {
            out.push(StreamChunk::Usage {
                usage,
                wire: Default::default(),
            });
        }
        out.push(StreamChunk::finish(FinishReason::new(kind)));
        Ok(out)
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn block_type(kind: TextKind) -> &'static str {
    match kind {
        TextKind::Text => "text",
        TextKind::Reasoning => "reasoning",
    }
}

/// `content` is a string; some servers send an array of `{type: "text", text}` parts.
fn content_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect(),
        ),
        _ => None,
    }
}

fn parse_usage(usage: &Value) -> Option<TokenUsage> {
    let object = usage.as_object()?;
    let number = |value: Option<&Value>| value.and_then(Value::as_u64);
    let input = number(object.get("prompt_tokens"));
    let output = number(object.get("completion_tokens"));
    if input.is_none() && output.is_none() {
        return None;
    }
    let (input_tokens, output_tokens) = (input.unwrap_or(0), output.unwrap_or(0));
    Some(TokenUsage {
        input_tokens,
        output_tokens,
        total_tokens: Some(
            number(object.get("total_tokens")).unwrap_or(input_tokens + output_tokens),
        ),
        cache_read_tokens: number(
            object
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens")),
        ),
        reasoning_tokens: number(
            object
                .get("completion_tokens_details")
                .and_then(|d| d.get("reasoning_tokens")),
        ),
        ..Default::default()
    })
}
