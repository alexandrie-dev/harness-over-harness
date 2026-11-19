//! Token-budgeted tool content with recoverable text and image addresses.

pub mod notice;
pub mod retention;
pub mod shim;
pub mod types;

use hoh_attachment::Attachments;
use hoh_error::BoxError;
use hoh_fs::Fs;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{
    ContentBlock, ImageBlock, Llm, MessageSource, NewUserMessage, ToolCallId, UserMessage,
    create_user_message, resolve_image_attachment_access,
};
use hoh_output_retention::Omitted;
use hoh_spill::{SaveTextSpill, SpillOwner, SpillSource, SpillStore};
use hoh_token_meter::estimate_content;
use hoh_tools::{PostToolDecision, ToolExecution, Tools, ToolsPostExecute};
use hoh_util_values::{js_json_stringify, js_number_string};
use notice::format_spill_notice;
use retention::{RetainableBlock, retain_content};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shim::PtcDispatchLog;
use types::SpillPolicyExec;

/// Cordis-compatible plugin identity.
pub const NAME: &str = "spill-policy";
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Optional result-retention budget. Omission disables the policy.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default, deserialize_with = "deserialize_max_inline_tokens")]
    #[schemars(with = "Option<f64>")]
    pub max_inline_tokens: Option<Value>,
}

fn deserialize_max_inline_tokens<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl Config {
    fn max_tokens(&self) -> Result<Option<u64>, BoxError> {
        let Some(raw) = &self.max_inline_tokens else {
            return Ok(None);
        };
        let Some(value) = raw.as_f64() else {
            return Err(format!(
                "spill-policy: maxInlineTokens must be a non-negative integer (got {})",
                js_json_stringify(raw)
            )
            .into());
        };
        if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > MAX_SAFE_INTEGER {
            return Err(format!(
                "spill-policy: maxInlineTokens must be a non-negative integer (got {})",
                js_number_string(value)
            )
            .into());
        }
        Ok(Some(value as u64))
    }
}

/// Required source dependency. All recovery and pricing services stay optional.
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
}

#[derive(Clone)]
struct Policy {
    ctx: Context,
    max_tokens: u64,
}

#[derive(Clone, Default)]
struct Prices {
    images: Vec<(ImageBlock, u64)>,
}

impl Prices {
    fn price(&self, block: &RetainableBlock) -> u64 {
        match block {
            RetainableBlock::Text { text } => estimate_content(&[ContentBlock::text(text)]),
            RetainableBlock::Image {
                attachment,
                offloaded,
            } => self
                .images
                .iter()
                .find(|(image, _)| image.attachment == *attachment && image.offloaded == *offloaded)
                .map(|(_, price)| *price)
                .unwrap_or(0),
        }
    }
}

impl Policy {
    fn route(&self, exec: &ToolExecution) -> Option<(String, String)> {
        let agent = exec.agent.as_ref()?;
        let header = agent.session().request_header();
        let provider = header
            .as_ref()
            .and_then(|header| header["config"]["provider"].as_str())
            .map(str::to_owned)
            .or_else(|| agent.options().provider())?;
        let model = header
            .as_ref()
            .and_then(|header| header["config"]["model"].as_str())
            .map(str::to_owned)
            .or_else(|| agent.options().model())?;
        Some((provider, model))
    }

    fn prices(&self, exec: &ToolExecution, images: &[ImageBlock]) -> Result<Prices, String> {
        if images.is_empty() {
            return Ok(Prices::default());
        }
        let (provider, model) = self
            .route(exec)
            .ok_or_else(|| "the current model has no image token calculator".to_owned())?;
        let calculator = self
            .ctx
            .get::<Llm>()
            .and_then(|llm| llm.image_request_pricing(&provider, &model))
            .ok_or_else(|| "the current model has no image token calculator".to_owned())?;
        let quoted = calculator.price_images(images);
        if quoted.len() != images.len() {
            return Err("image token calculator returned an inconsistent occurrence count".into());
        }
        let mut prices = Prices::default();
        for (image, quote) in images.iter().zip(quoted) {
            let text = estimate_content(&[ContentBlock::text(quote.text)]);
            prices
                .images
                .push((image.clone(), quote.visual_tokens.saturating_add(text)));
        }
        Ok(prices)
    }

    fn full_text(&self, content: &[RetainableBlock]) -> Result<String, String> {
        let mut result = String::new();
        for block in content {
            match block {
                RetainableBlock::Text { text } => result.push_str(text),
                RetainableBlock::Image { attachment, .. } => {
                    let access = match (self.ctx.get::<Attachments>(), self.ctx.get::<Fs>()) {
                        (Some(attachments), Some(fs)) => resolve_image_attachment_access(
                            &*attachments,
                            |path| fs.process_path_from_host_path(path),
                            attachment,
                        )
                        .map_err(|error| error.to_string())?,
                        _ => None,
                    }
                    .ok_or_else(|| {
                        format!(
                            "image {} has no readable attachment path",
                            attachment.attachment_id
                        )
                    })?;
                    result.push_str("\n[Image: ");
                    result.push_str(&js_json_stringify(&Value::String(access.readonly_path)));
                    result.push_str(&format!(
                        "; {}; {}x{}. Use read_image to view it.]\n",
                        attachment.media_type, attachment.width, attachment.height
                    ));
                }
            }
        }
        Ok(result)
    }

    fn retainable(content: &[ContentBlock]) -> Option<Vec<RetainableBlock>> {
        content.iter().map(RetainableBlock::from_content).collect()
    }

    async fn try_bound(
        &self,
        exec: &ToolExecution,
        content: &[ContentBlock],
        tool_name: &str,
        call_id: &ToolCallId,
        label: &'static str,
    ) -> Result<Option<Vec<ContentBlock>>, String> {
        let Some(content) = Self::retainable(content) else {
            return Ok(None);
        };
        let images = content
            .iter()
            .filter_map(RetainableBlock::image)
            .collect::<Vec<_>>();
        let prices = self.prices(exec, &images)?;
        let total = content
            .iter()
            .fold(0_u64, |sum, block| sum.saturating_add(prices.price(block)));
        if total <= self.max_tokens {
            return Ok(None);
        }
        let owner = exec
            .spill_owner()
            .ok_or_else(|| format!("no session owner for {tool_name} {label}"))?;
        let store = self
            .ctx
            .get::<SpillStore>()
            .ok_or_else(|| "no ctx.spillStore backend loaded".to_owned())?;
        let full_text = self.full_text(&content)?;
        let reference = store
            .save_text(SaveTextSpill {
                owner: SpillOwner { session_id: owner },
                source: SpillSource::Tool {
                    tool_name: tool_name.to_owned(),
                    call_id: call_id.clone(),
                    label: label.to_owned(),
                },
                suggested_name: format!("{tool_name}.txt"),
                content: full_text,
            })
            .await
            .map_err(|error| error.to_string())?;

        let total_bytes = content.iter().fold(0_u64, |sum, block| {
            sum.saturating_add(block.text_ref().map_or(0, |text| text.len() as u64))
        });
        let gap = RetainableBlock::text("\n\n[...]\n\n");
        let worst = RetainableBlock::text(format_spill_notice(
            &Omitted::Exact { count: total_bytes },
            &reference,
            images.len() as u64,
        ));
        let reserved = prices
            .price(&gap)
            .saturating_add(prices.price(&RetainableBlock::text(format!(
                "\n\n{}",
                worst.text_value().unwrap_or_default()
            ))));
        if prices.price(&worst) > self.max_tokens {
            return Err(format!(
                "spill notice for {tool_name} exceeds maxInlineTokens"
            ));
        }
        let retained = retain_content(
            &content,
            self.max_tokens.saturating_sub(reserved),
            |block| prices.price(block),
        );
        let footer = RetainableBlock::text(format_spill_notice(
            &Omitted::Exact {
                count: retained.omitted_bytes,
            },
            &reference,
            retained.omitted_images,
        ));
        let mut result = if retained.head.is_empty() && retained.tail.is_empty() {
            vec![footer]
        } else {
            let mut result = retained.head;
            result.push(gap);
            result.extend(retained.tail);
            result.push(RetainableBlock::text(format!(
                "\n\n{}",
                footer.text_value().unwrap_or_default()
            )));
            result
        };
        let mut merged: Vec<ContentBlock> = Vec::new();
        for block in result.drain(..) {
            if let Some(text) = block.text_value()
                && let Some(previous) = merged.last_mut()
                && previous.r#type == "text"
            {
                let combined = format!("{}{}", previous.text_value().unwrap_or_default(), text);
                *previous = ContentBlock::text(combined);
            } else {
                merged.push(block.into_content());
            }
        }
        Ok(Some(merged))
    }

    async fn bound(
        &self,
        exec: &ToolExecution,
        content: &[ContentBlock],
        tool_name: &str,
        call_id: &ToolCallId,
        label: &'static str,
    ) -> Option<Vec<ContentBlock>> {
        match self
            .try_bound(exec, content, tool_name, call_id, label)
            .await
        {
            Ok(content) => content,
            Err(error) => {
                self.ctx.logger().warn(format_args!(
                    "spill-policy: Error: {error}; keeping the inline content"
                ));
                None
            }
        }
    }
}

fn ptc_context(content: Vec<ContentBlock>) -> UserMessage {
    create_user_message(NewUserMessage {
        content,
        source: MessageSource::new("ptc-mode"),
    })
}

/// Listener-only spill policy.
pub struct SpillPolicy;

impl Plugin for SpillPolicy {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let _tools = deps.tools;
        let Some(max_tokens) = config.get().max_tokens()? else {
            return Ok(());
        };
        let policy = Policy {
            ctx: ctx.clone(),
            max_tokens,
        };
        let post = policy.clone();
        ctx.listen()
            .prepend()
            .on_waterfall::<ToolsPostExecute, PostToolDecision, _>(move |event, next| {
                let policy = post.clone();
                async move {
                    let decision = next.run().await?;
                    let PostToolDecision::Accept {
                        content,
                        value,
                        additional_contexts,
                    } = decision
                    else {
                        return Ok(decision);
                    };
                    if value.is_some() || event.exec.name == "read" {
                        return Ok(PostToolDecision::Accept {
                            content,
                            value,
                            additional_contexts,
                        });
                    }
                    let projected = content.as_deref().unwrap_or_else(|| event.result.content());
                    let has_images = projected.iter().any(|block| block.r#type == "image");
                    if event.exec.parent.is_some() && !has_images {
                        return Ok(PostToolDecision::Accept {
                            content,
                            value,
                            additional_contexts,
                        });
                    }
                    let label = if event.exec.parent.is_some() {
                        "dispatch"
                    } else {
                        "result"
                    };
                    let Some(retained) = policy
                        .bound(
                            &event.exec,
                            projected,
                            &event.exec.name,
                            &event.exec.call_id,
                            label,
                        )
                        .await
                    else {
                        return Ok(PostToolDecision::Accept {
                            content,
                            value,
                            additional_contexts,
                        });
                    };
                    let mut contexts = additional_contexts.unwrap_or_default();
                    if event.exec.parent.is_some()
                        && !event.result.is_error()
                        && has_images
                        && !retained.iter().any(|block| block.r#type == "image")
                    {
                        contexts.push(ptc_context(retained.clone()));
                    }
                    Ok(PostToolDecision::Accept {
                        content: Some(retained),
                        value: None,
                        additional_contexts: (!contexts.is_empty()).then_some(contexts),
                    })
                }
            })?;

        ctx.listen()
            .prepend()
            .on_waterfall::<PtcDispatchLog, Vec<ContentBlock>, _>(move |event, next| {
                let policy = policy.clone();
                async move {
                    let content = next.run().await?;
                    Ok(policy
                        .bound(
                            &event.exec,
                            &content,
                            &event.name,
                            &event.sub_call_id,
                            "dispatch",
                        )
                        .await
                        .unwrap_or(content))
                }
            })?;
        Ok(())
    }
}
