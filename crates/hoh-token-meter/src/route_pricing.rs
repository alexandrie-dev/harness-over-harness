use crate::{MeterSurfaceNode, TokenSurfaceNode, error, estimate_content};
use hoh_attachment::FileAttachmentRef;
use hoh_error::BoxError;
use hoh_llm::{ContentBlock, LlmImageRequestPricing};

/// One surface priced for a request route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PricedSurface {
    pub nodes: Vec<TokenSurfaceNode>,
    pub surface_tokens: u64,
}

/// Price one ordered surface under its model-request attachment projection.
pub fn price_surface(
    nodes: &[MeterSurfaceNode],
    pricing: Option<&dyn LlmImageRequestPricing>,
    file_text: Option<&(dyn Fn(&FileAttachmentRef) -> String + Send + Sync)>,
) -> Result<PricedSurface, BoxError> {
    let images = pricing.map_or_else(Vec::new, |_| {
        nodes
            .iter()
            .flat_map(|node| node.images.iter().cloned())
            .collect()
    });
    let has_files = file_text.is_some() && nodes.iter().any(|node| !node.files.is_empty());
    if (pricing.is_none() || images.is_empty()) && !has_files {
        let mut surface_tokens = 0_u64;
        let nodes = nodes
            .iter()
            .map(|node| {
                surface_tokens = surface_tokens.saturating_add(node.heuristic_tokens);
                TokenSurfaceNode {
                    seq: node.seq,
                    tokens: node.heuristic_tokens,
                    heuristic_tokens: node.heuristic_tokens,
                }
            })
            .collect();
        return Ok(PricedSurface {
            nodes,
            surface_tokens,
        });
    }
    let prices = pricing.map_or_else(Vec::new, |pricing| pricing.price_images(&images));
    if pricing.is_some() && prices.len() != images.len() {
        return Err(error(format!(
            "token meter: route image pricing answered {} prices for {} occurrences",
            prices.len(),
            images.len()
        )));
    }
    let mut cursor = 0_usize;
    let mut surface_tokens = 0_u64;
    let mut public = Vec::with_capacity(nodes.len());
    for node in nodes {
        let mut tokens = node.heuristic_tokens;
        if let Some(file_text) = file_text.filter(|_| !node.files.is_empty()) {
            tokens = tokens.saturating_sub(node.file_structural_tokens);
            for file in &node.files {
                tokens =
                    tokens.saturating_add(estimate_content(&[ContentBlock::text(file_text(file))]));
            }
        }
        if pricing.is_some() && !node.images.is_empty() {
            tokens = tokens.saturating_sub(node.image_structural_tokens);
            for _ in &node.images {
                let price = &prices[cursor];
                cursor += 1;
                tokens = tokens
                    .saturating_add(price.visual_tokens)
                    .saturating_add(estimate_content(&[ContentBlock::text(&price.text)]));
            }
        }
        surface_tokens = surface_tokens.saturating_add(tokens);
        public.push(TokenSurfaceNode {
            seq: node.seq,
            tokens,
            heuristic_tokens: node.heuristic_tokens,
        });
    }
    Ok(PricedSurface {
        nodes: public,
        surface_tokens,
    })
}
