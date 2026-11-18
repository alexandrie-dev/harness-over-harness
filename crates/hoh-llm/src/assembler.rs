use crate::{error::ValueError, message::*, types::*};
use indexmap::IndexMap;
use serde_json::Value;
#[derive(Default)]
struct PartialBlock {
    block_type: String,
    text: String,
    tool_call_id: Option<ToolCallId>,
    tool_call_name: Option<String>,
    tool_call_arguments: String,
    block: Option<ContentBlock>,
}
#[derive(Default)]
pub struct BlockAssembler {
    partials: IndexMap<u64, PartialBlock>,
    usage: Option<TokenUsage>,
    finish: Option<FinishReason>,
    replay_state: Option<ReplayEnvelope>,
}
impl BlockAssembler {
    pub fn new() -> Self {
        Self::default()
    }
    fn ensure(&mut self, index: u64, kind: &str) -> &mut PartialBlock {
        self.partials.entry(index).or_insert_with(|| PartialBlock {
            block_type: kind.into(),
            ..Default::default()
        })
    }
    pub fn push(&mut self, chunk: StreamChunk) {
        match chunk {
            StreamChunk::BlockStart { index, block_type } => {
                self.ensure(index, &block_type);
            }
            StreamChunk::TextDelta { index, text } => {
                let p = self.ensure(index, "text");
                if p.block.is_none() {
                    p.text += &text;
                }
            }
            StreamChunk::ReasoningDelta { index, text } => {
                let p = self.ensure(index, "reasoning");
                if p.block.is_none() {
                    p.text += &text;
                }
            }
            StreamChunk::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                let p = self.ensure(index, "tool-call");
                if p.block.is_some() {
                    return;
                }
                p.tool_call_id = Some(id);
                if name.as_ref().is_some_and(|s| !s.is_empty()) {
                    p.tool_call_name = name;
                }
                p.tool_call_arguments += &arguments_delta;
            }
            StreamChunk::BlockEnd { index, block } => {
                let p = self.ensure(index, &block.r#type);
                if p.block.is_none() {
                    p.block = Some(block);
                }
            }
            StreamChunk::Usage { usage } => self.usage = Some(usage),
            StreamChunk::Finish {
                reason,
                replay_state,
            } => {
                self.finish = Some(reason);
                self.replay_state = replay_state;
            }
        }
    }
    fn assemble(p: &PartialBlock, index: u64) -> Result<ContentBlock, ValueError> {
        if let Some(block) = &p.block {
            return Ok(block.clone());
        }
        Ok(match p.block_type.as_str() {
            "text" => ContentBlock::text(&p.text),
            "reasoning" => ContentBlock::reasoning(&p.text),
            "tool-call" => ToolCallBlock {
                id: p
                    .tool_call_id
                    .clone()
                    .unwrap_or_else(|| format!("call-{index}").into()),
                name: p.tool_call_name.clone().unwrap_or_default(),
                arguments: p.tool_call_arguments.clone(),
            }
            .into(),
            kind => {
                return Err(ValueError::Message(format!(
                    "cannot assemble incomplete block of type \"{kind}\""
                )));
            }
        })
    }
    fn assembled(&self) -> Result<(Vec<ContentBlock>, Option<ReplayEnvelope>), ValueError> {
        let all = self
            .partials
            .iter()
            .map(|(i, p)| Self::assemble(p, *i))
            .collect::<Result<Vec<_>, _>>()?;
        let kept: Vec<_> = all
            .iter()
            .map(|b| self.finish().kind != "max-tokens" || b.r#type != "tool-call")
            .collect();
        let replay = self.replay_state.clone().and_then(|mut envelope| {
            if let Some(entries) = envelope.blocks.take() {
                if entries.len() != all.len() {
                    return None;
                }
                envelope.blocks = Some(
                    entries
                        .into_iter()
                        .zip(&kept)
                        .filter_map(|(b, k)| k.then_some(b))
                        .collect(),
                );
            }
            Some(envelope)
        });
        Ok((
            all.into_iter()
                .zip(kept)
                .filter_map(|(b, k)| k.then_some(b))
                .collect(),
            replay,
        ))
    }
    pub fn blocks(&self) -> Result<Vec<ContentBlock>, ValueError> {
        Ok(self.assembled()?.0)
    }
    pub fn interrupted_blocks(&self) -> Vec<ContentBlock> {
        self.partials
            .iter()
            .filter(|(_, p)| {
                matches!(
                    p.block
                        .as_ref()
                        .map_or(p.block_type.as_str(), |b| b.r#type.as_str()),
                    "text" | "reasoning"
                )
            })
            .filter_map(|(i, p)| Self::assemble(p, *i).ok())
            .filter(|b| {
                b.text_value()
                    .is_some_and(|s| !hoh_util_values::js_trim(s).is_empty())
            })
            .collect()
    }
    pub fn usage(&self) -> Option<&TokenUsage> {
        self.usage.as_ref()
    }
    pub fn finish(&self) -> FinishReason {
        self.finish
            .clone()
            .unwrap_or_else(|| FinishReason::new("stop"))
    }
    pub fn replay_state(&self) -> Result<Option<ReplayEnvelope>, ValueError> {
        Ok(self.assembled()?.1)
    }
    pub fn message(
        &self,
        source: AssistantProviderMetadata,
    ) -> Result<AssistantMessage, ValueError> {
        Ok(create_assistant_message(NewAssistantMessage {
            content: self.blocks()?,
            source,
        }))
    }
    /// Raw adapter data is validated by the closed stream vocabulary before mutation.
    pub fn push_value(&mut self, mut chunk: Value) -> Result<(), ValueError> {
        // The source assembler tolerates a missing tool id and synthesizes call-{index}.
        if chunk.get("type").and_then(Value::as_str) == Some("tool-call-delta")
            && chunk.get("id").is_none()
            && let Some(index) = chunk.get("index").and_then(Value::as_u64)
        {
            chunk["id"] = Value::String(format!("call-{index}"));
        }
        let parsed = serde_json::from_value(chunk.clone()).map_err(|_| {
            ValueError::Message(format!(
                "unreachable variant in BlockAssembler.push: {}",
                hoh_util_values::js_json_stringify(&chunk)
            ))
        })?;
        self.push(parsed);
        Ok(())
    }
}
