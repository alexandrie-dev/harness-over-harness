use hoh_llm::{ContentBlock, ImageBlock, LlmImageRequestPrice, LlmImageRequestPricing};
use hoh_session::SessionSeq;
use hoh_token_meter::{
    MeterSurfaceNode, estimate_content, estimate_structural_block, price_surface,
};
use serde_json::json;

fn image() -> ImageBlock {
    let block: ContentBlock = serde_json::from_value(json!({
        "type":"image",
        "attachment":{"attachmentId":"same","mediaType":"image/png","bytes":8,"width":1,"height":1}
    }))
    .unwrap();
    block.image().unwrap()
}

struct OffloadPricing;

impl LlmImageRequestPricing for OffloadPricing {
    fn price_images(&self, images: &[ImageBlock]) -> Vec<LlmImageRequestPrice> {
        images
            .iter()
            .map(|image| {
                if image.offloaded == Some(true) {
                    LlmImageRequestPrice {
                        visual_tokens: 0,
                        text: "[image offloaded]".into(),
                    }
                } else {
                    LlmImageRequestPrice {
                        visual_tokens: 100,
                        text: String::new(),
                    }
                }
            })
            .collect()
    }
}

#[test]
fn reprices_one_of_two_equal_attachments_by_occurrence_without_rewriting_node_identity() {
    let normal = image();
    let mut offloaded = normal.clone();
    offloaded.offloaded = Some(true);
    let structural = estimate_structural_block(&ContentBlock::from(normal.clone()));
    let node = MeterSurfaceNode {
        seq: SessionSeq::new(7).unwrap(),
        heuristic_tokens: structural * 2 + 4,
        image_structural_tokens: structural * 2,
        file_structural_tokens: 0,
        images: vec![normal, offloaded],
        files: vec![],
    };
    let priced = price_surface(std::slice::from_ref(&node), Some(&OffloadPricing), None).unwrap();
    assert_eq!(priced.nodes[0].seq, node.seq);
    assert_eq!(priced.nodes[0].heuristic_tokens, node.heuristic_tokens);
    assert_eq!(
        priced.nodes[0].tokens,
        4 + 100
            + estimate_content(&[ContentBlock::text("")])
            + estimate_content(&[ContentBlock::text("[image offloaded]")])
    );
}

#[test]
fn routes_without_declared_pricing_keep_the_fixed_heuristic() {
    let image = image();
    let structural = estimate_structural_block(&ContentBlock::from(image.clone()));
    let node = MeterSurfaceNode {
        seq: SessionSeq::new(0).unwrap(),
        heuristic_tokens: structural + 4,
        image_structural_tokens: structural,
        file_structural_tokens: 0,
        images: vec![image],
        files: vec![],
    };
    let priced = price_surface(std::slice::from_ref(&node), None, None).unwrap();
    assert_eq!(priced.nodes[0].tokens, node.heuristic_tokens);
    assert_eq!(priced.surface_tokens, node.heuristic_tokens);
}

#[test]
fn tool_result_images_use_the_same_occurrence_pricing_path() {
    let image = image();
    let structural = estimate_structural_block(&ContentBlock::from(image.clone()));
    let tool_result_node = MeterSurfaceNode {
        seq: SessionSeq::new(3).unwrap(),
        heuristic_tokens: structural + 4,
        image_structural_tokens: structural,
        file_structural_tokens: 0,
        images: vec![image],
        files: vec![],
    };
    let priced = price_surface(&[tool_result_node], Some(&OffloadPricing), None).unwrap();
    assert_eq!(priced.nodes[0].tokens, 108);
}
