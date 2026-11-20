//! Port of @deepseek-ai/dsh-compaction-tool-result-pruner (packages/compaction/compaction-tool-result-pruner).
//!
//! Replay-safe, model-free tool-result pruning service.

mod config;
mod types;

pub use config::{DEFAULTS, PRUNE_MARKER, code_point_length, resolve_config};
pub use types::{PruneResult, PrunedEntry, ResolvedConfig, ToolResultPruneConfig};

use hoh_compaction::{CompactionPruneEvent, ShadowedRange};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{ContentBlock, Message, ToolCallId};
use hoh_session::{Session, SessionSeq, SurfaceIntent};
use hoh_token_meter::TokenMeter;
use serde_json::{Map, Value};
use std::sync::Arc;

/// Plain `Error` of the source: a message and nothing else.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ToolResultPruneError(String);

impl ToolResultPruneError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

hoh_kernel::service!(pub ToolResultPruner: ToolResultPrunerRuntime = "toolResultPruner");

/// Deterministic head/middle/tail pruning for current tool-result surface nodes.
pub struct ToolResultPrunerRuntime {
    // The token meter prices each shadowed node for its logged shadow-price
    // event, so pruning genuinely requires the pricing capability.
    token_meter: Service<TokenMeter>,
    /// Resolved and immutable character budgets.
    pub config: ResolvedConfig,
}

impl ToolResultPrunerRuntime {
    /// `new ToolResultPruner(ctx, config)`: resolves the budgets, rejecting a stale or invalid config.
    pub fn new(
        token_meter: Service<TokenMeter>,
        config: &ToolResultPruneConfig,
    ) -> Result<Self, ToolResultPruneError> {
        Ok(Self {
            token_meter,
            config: resolve_config(config)?,
        })
    }

    /// Measure text content in Unicode code points; non-text blocks cost zero.
    pub fn measure_content(&self, blocks: &[ContentBlock]) -> u64 {
        blocks
            .iter()
            .filter(|block| block.r#type == "text")
            .map(|block| code_point_length(block.text_value().unwrap_or("")))
            .sum()
    }

    /// Replace an over-budget text middle while retaining rich-block order.
    /// Text slicing is by Unicode code point, so a retained boundary cannot split a
    /// surrogate pair. Returns `None` when the text is within budget.
    pub fn prune_content(
        &self,
        blocks: &[ContentBlock],
    ) -> Result<Option<Vec<ContentBlock>>, ToolResultPruneError> {
        let total_chars = self.measure_content(blocks);
        if total_chars <= self.config.threshold_chars {
            return Ok(None);
        }

        let removed_start = self.config.head_chars;
        let removed_end = total_chars - self.config.tail_chars;
        let mut pruned: Vec<ContentBlock> = Vec::new();
        let mut consumed: u64 = 0;
        let mut marker_inserted = false;

        for block in blocks {
            if block.r#type != "text" {
                pruned.push(block.clone());
                continue;
            }

            let points: Vec<char> = block.text_value().unwrap_or("").chars().collect();
            let length = points.len() as u64;
            let block_start = consumed;
            let block_end = block_start + length;
            let head_end = length.min(removed_start.saturating_sub(block_start)) as usize;
            let tail_start = length.min(removed_end.saturating_sub(block_start)) as usize;
            let intersects_removed = block_start < removed_end && block_end > removed_start;
            let marker = if intersects_removed && !marker_inserted {
                PRUNE_MARKER
            } else {
                ""
            };
            if !marker.is_empty() {
                marker_inserted = true;
            }
            let mut text: String = points[..head_end].iter().collect();
            text.push_str(marker);
            text.extend(points[tail_start..].iter());
            if !text.is_empty() {
                let mut replaced = block.clone();
                replaced.data.insert("text".into(), Value::String(text));
                pruned.push(replaced);
            }
            consumed = block_end;
        }

        if !marker_inserted {
            return Err(ToolResultPruneError::new(
                "tool-result prune: failed to locate the removed text span",
            ));
        }
        let chars_after = self.measure_content(&pruned);
        if chars_after > self.config.threshold_chars || chars_after >= total_chars {
            return Err(ToolResultPruneError::new(
                "tool-result prune: replacement must be smaller and within threshold",
            ));
        }
        Ok(Some(pruned))
    }

    /// Prune every over-budget tool result from one stable current-surface snapshot.
    /// Each replacement preserves the complete event data except for `content`, cites the
    /// shadowed node, and is immediately preceded by a `compaction/prune` shadow-price event.
    /// Replacements committed before a rejected one remain durable.
    pub fn prune_session(&self, session: &Session) -> Result<PruneResult, BoxError> {
        let mut candidates: Vec<(SessionSeq, Arc<hoh_session::SessionEvent>)> = Vec::new();
        for seq in session.surface()?.nodes()? {
            if let Some(event) = session.event_at(seq)
                && event.event_type() == "tool/result"
            {
                candidates.push((seq, event));
            }
        }

        let mut pruned: Vec<PrunedEntry> = Vec::new();
        let mut chars_removed: u64 = 0;
        for (seq, event) in candidates {
            let original = session
                .derive_event_message(&event)?
                .ok_or_else(|| ToolResultPruneError::new("tool-result prune: no message"))?;
            let original_content = content_of(&original)?;
            let Some(content) = self.prune_content(&original_content)? else {
                continue;
            };
            let chars_before = self.measure_content(&original_content);
            let chars_after = self.measure_content(&content);
            let mut message = (*original).clone();
            set_key(&mut message, "content", serde_json::to_value(&content)?)?;
            // Shadow-price protocol: the metering event and its replacement are
            // appended synchronously adjacent, so pure consumers subtract the
            // shadowed node's heuristic price without retaining per-node state.
            let priced: Message = serde_json::from_value((*original).clone())?;
            session.append(
                CompactionPruneEvent::TYPE,
                serde_json::to_value(CompactionPruneEvent {
                    shadowed_range: ShadowedRange {
                        start: seq,
                        end: seq,
                    },
                    shadowed_seqs: vec![seq],
                    shadowed_token_count: self.token_meter.estimate_message(&priced),
                })?,
                None,
            )?;
            let mut data = event.data().clone();
            set_key(&mut data, "message", message)?;
            let replacement = session.append(
                "tool/result",
                data,
                Some(SurfaceIntent::replace(seq, seq, vec![seq])),
            )?;
            let call_id: ToolCallId =
                serde_json::from_value(event.data()["message"]["source"]["callId"].clone())?;
            pruned.push(PrunedEntry {
                original_seq: seq,
                replacement_seq: replacement.seq(),
                call_id,
                chars_before,
                chars_after,
            });
            chars_removed += chars_before - chars_after;
        }
        Ok(PruneResult {
            pruned,
            chars_removed,
        })
    }
}

fn content_of(message: &Value) -> Result<Vec<ContentBlock>, BoxError> {
    Ok(serde_json::from_value(message["content"].clone())?)
}

/// `{ ...object, key: value }`: an existing key keeps its position, a new one goes last.
fn set_key(object: &mut Value, key: &str, value: Value) -> Result<(), ToolResultPruneError> {
    match object {
        Value::Object(map) => {
            let map: &mut Map<String, Value> = map;
            map.insert(key.into(), value);
            Ok(())
        }
        _ => Err(ToolResultPruneError::new(
            "tool-result prune: event data is not an object",
        )),
    }
}

/// `export const inject = ['tokenMeter']`.
#[derive(Inject)]
pub struct Deps {
    token_meter: Service<TokenMeter>,
}

/// The `ToolResultPruner` service class of the source as one plugin.
pub struct ToolResultPrunerPlugin;

impl Plugin for ToolResultPrunerPlugin {
    const NAME: &'static str = "ToolResultPruner";
    type Config = ToolResultPruneConfig;
    type Deps = Deps;

    async fn apply(
        ctx: Context,
        deps: Deps,
        config: Cfg<ToolResultPruneConfig>,
    ) -> Result<(), BoxError> {
        let runtime = ToolResultPrunerRuntime::new(deps.token_meter, &config.get())?;
        ctx.provide::<ToolResultPruner>(Arc::new(runtime)).await?;
        Ok(())
    }
}
