use hoh_llm::{ContentBlock, LlmImageRequestPrice, LlmImageRequestPricing, Message};
use hoh_session::{SessionEvent, SessionSeq};
use hoh_token_meter::{
    MeterSurfaceNode, commit_surface_tokens, estimate_content, estimate_message,
    estimate_structural_block, estimate_system_message, estimate_tools_tokens,
    fold_surface_projection, plan_surface_tokens, price_surface,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn message(value: Value) -> Message {
    serde_json::from_value(value).unwrap()
}

fn event(seq: u64, event_type: &str, data: Value, surface_op: Option<Value>) -> SessionEvent {
    let mut value = json!({"seq":seq,"time":seq,"type":event_type,"data":data});
    if let Some(surface_op) = surface_op {
        value["surfaceOp"] = surface_op;
    }
    SessionEvent::from_value(value)
}

fn user(text: &str) -> Value {
    json!({
        "role":"user",
        "content":[{"type":"text","text":text}],
        "source":{"kind":"user"},
        "id":"user-message"
    })
}

#[test]
fn prices_text_reasoning_tool_calls_and_merge_extended_blocks_like_javascript() {
    let blocks: Vec<ContentBlock> = serde_json::from_value(json!([
        {"type":"text","text":"12345"},
        {"type":"reasoning","text":"1234"},
        {"type":"tool-call","id":"call","name":"abcde","arguments":"123456789"},
        {"type":"future","payload":"abc"}
    ]))
    .unwrap();
    assert_eq!(estimate_content(&blocks[..1]), 6);
    assert_eq!(estimate_content(&blocks[1..2]), 5);
    assert_eq!(estimate_content(&blocks[2..3]), 2 + 3 + 4);
    assert_eq!(
        estimate_content(&blocks[3..]),
        4 + hoh_util_values::js_json_stringify(&serde_json::to_value(&blocks[3]).unwrap())
            .encode_utf16()
            .count()
            .div_ceil(4) as u64
    );
}

#[test]
fn counts_utf16_code_units_instead_of_rust_scalar_values() {
    let priced = message(json!({
        "role":"user",
        "content":[{"type":"text","text":"😀😀😀"}],
        "source":{"kind":"user"},
        "id":"utf16"
    }));
    assert_eq!(estimate_message(&priced), 4 + 4 + 2);
}

#[test]
fn prices_system_text_without_block_overhead_and_empty_system_as_zero() {
    let system = message(json!({
        "role":"system",
        "content":[{"type":"text","text":"You are terse."}],
        "source":{"kind":"system-prompt"},
        "id":"system"
    }));
    let empty = message(json!({
        "role":"system","content":[],"source":{"kind":"system-prompt"},"id":"empty"
    }));
    assert_eq!(estimate_system_message(&system), 8);
    assert_eq!(estimate_message(&system), 8);
    assert_eq!(estimate_system_message(&empty), 0);
}

#[test]
fn prices_tools_from_the_envelope_and_omits_absent_or_empty_tools() {
    let header = json!({"tools":[{"name":"read","description":"Read a file"}]});
    let expected = hoh_util_values::js_json_stringify(&header["tools"])
        .encode_utf16()
        .count()
        .div_ceil(4) as u64
        + 4;
    assert_eq!(estimate_tools_tokens(Some(&header)), expected);
    assert_eq!(estimate_tools_tokens(Some(&json!({}))), 0);
    assert_eq!(estimate_tools_tokens(Some(&json!({"tools":[]}))), 0);
    assert_eq!(estimate_tools_tokens(None), 0);
}

#[test]
fn image_structural_price_excludes_the_offloaded_marker() {
    let plain: ContentBlock = serde_json::from_value(json!({
        "type":"image",
        "attachment":{"attachmentId":"image","mediaType":"image/png","bytes":8,"width":1,"height":1}
    }))
    .unwrap();
    let offloaded: ContentBlock = serde_json::from_value(json!({
        "type":"image",
        "attachment":{"attachmentId":"image","mediaType":"image/png","bytes":8,"width":1,"height":1},
        "offloaded":true
    })).unwrap();
    assert_eq!(
        estimate_structural_block(&plain),
        estimate_structural_block(&offloaded)
    );
}

#[test]
fn surface_plan_is_transactional_and_replaces_an_inclusive_range() {
    let first = event(0, "user/message", user("one"), Some(json!("append")));
    let second = event(1, "user/message", user("two"), Some(json!("append")));
    let replacement = event(
        2,
        "user/message",
        user("summary"),
        Some(json!({"op":"replace","startSeq":0,"endSeq":1})),
    );
    let mut nodes = vec![];
    let first_plan = plan_surface_tokens(&nodes, &first).unwrap();
    assert!(first_plan.delta_tokens > 0);
    commit_surface_tokens(&mut nodes, first_plan);
    let second_plan = plan_surface_tokens(&nodes, &second).unwrap();
    commit_surface_tokens(&mut nodes, second_plan);
    let before = nodes.clone();
    let plan = plan_surface_tokens(&nodes, &replacement).unwrap();
    assert_eq!(nodes, before);
    commit_surface_tokens(&mut nodes, plan);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].seq, SessionSeq::new(2).unwrap());
}

#[test]
fn corrupt_surface_ranges_fail_without_mutating_the_prior_surface() {
    let first = event(0, "user/message", user("one"), Some(json!("append")));
    let mut nodes = vec![];
    let first_plan = plan_surface_tokens(&nodes, &first).unwrap();
    commit_surface_tokens(&mut nodes, first_plan);
    let bad = event(
        2,
        "user/message",
        user("bad"),
        Some(json!({"op":"replace","startSeq":0,"endSeq":1})),
    );
    let before = nodes.clone();
    let error = plan_surface_tokens(&nodes, &bad).unwrap_err().to_string();
    assert_eq!(
        error,
        "token surface: replace at seq 2 has invalid current range 0-1"
    );
    assert_eq!(nodes, before);
}

struct FixedPricing {
    prices: Vec<LlmImageRequestPrice>,
}
impl LlmImageRequestPricing for FixedPricing {
    fn price_images(&self, _: &[hoh_llm::ImageBlock]) -> Vec<LlmImageRequestPrice> {
        self.prices.clone()
    }
}

struct CountingPricing(Arc<AtomicUsize>);

impl LlmImageRequestPricing for CountingPricing {
    fn price_images(&self, images: &[hoh_llm::ImageBlock]) -> Vec<LlmImageRequestPrice> {
        self.0.fetch_add(1, Ordering::SeqCst);
        images
            .iter()
            .map(|_| LlmImageRequestPrice {
                visual_tokens: 1,
                text: String::new(),
            })
            .collect()
    }
}

fn image_node() -> MeterSurfaceNode {
    let block: ContentBlock = serde_json::from_value(json!({
        "type":"image",
        "attachment":{"attachmentId":"image","mediaType":"image/png","bytes":8,"width":1,"height":1}
    }))
    .unwrap();
    MeterSurfaceNode {
        seq: SessionSeq::new(0).unwrap(),
        heuristic_tokens: estimate_structural_block(&block) + 4,
        image_structural_tokens: estimate_structural_block(&block),
        file_structural_tokens: 0,
        images: vec![block.image().unwrap()],
        files: vec![],
    }
}

#[test]
fn route_pricing_replaces_image_structure_with_visual_and_visible_text_price() {
    let node = image_node();
    let pricing = FixedPricing {
        prices: vec![LlmImageRequestPrice {
            visual_tokens: 100,
            text: "image".into(),
        }],
    };
    let priced = price_surface(std::slice::from_ref(&node), Some(&pricing), None).unwrap();
    assert_eq!(
        priced.surface_tokens,
        node.heuristic_tokens - node.image_structural_tokens
            + 100
            + estimate_content(&[ContentBlock::text("image")])
    );
    assert_eq!(priced.nodes[0].heuristic_tokens, node.heuristic_tokens);
}

#[test]
fn route_pricing_rejects_a_misaligned_image_price_vector() {
    let error = price_surface(
        &[image_node()],
        Some(&FixedPricing { prices: vec![] }),
        None,
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        error,
        "token meter: route image pricing answered 0 prices for 1 occurrences"
    );
}

#[test]
fn empty_or_text_only_surfaces_do_not_call_the_image_pricer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let pricing = CountingPricing(calls.clone());
    assert_eq!(
        price_surface(&[], Some(&pricing), None)
            .unwrap()
            .surface_tokens,
        0
    );
    let text = MeterSurfaceNode {
        seq: SessionSeq::new(0).unwrap(),
        heuristic_tokens: 7,
        image_structural_tokens: 0,
        file_structural_tokens: 0,
        images: vec![],
        files: vec![],
    };
    assert_eq!(
        price_surface(&[text], Some(&pricing), None)
            .unwrap()
            .surface_tokens,
        7
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn file_occurrences_use_the_exact_request_handle_text() {
    let block: ContentBlock = serde_json::from_value(json!({
        "type":"file",
        "attachment":{"attachmentId":"file","name":"notes.txt","bytes":12}
    }))
    .unwrap();
    let structural = estimate_structural_block(&block);
    let file = block.file().unwrap().attachment;
    let node = MeterSurfaceNode {
        seq: SessionSeq::new(0).unwrap(),
        heuristic_tokens: structural + 4,
        image_structural_tokens: 0,
        file_structural_tokens: structural,
        images: vec![],
        files: vec![file],
    };
    let render = |_file: &hoh_attachment::FileAttachmentRef| {
        "File notes.txt is available at /workspace/notes.txt".to_owned()
    };
    let priced = price_surface(std::slice::from_ref(&node), None, Some(&render)).unwrap();
    assert_eq!(
        priced.surface_tokens,
        node.heuristic_tokens - structural
            + estimate_content(&[ContentBlock::text(render(&node.files[0]))])
    );
}

#[test]
fn bounded_surface_projection_consumes_only_an_adjacent_exact_claim() {
    let claim = event(
        1,
        "compaction/summary",
        json!({"shadowedRange":{"start":0,"end":0},"shadowedTokenCount":7,"shadowedSeqs":[0]}),
        None,
    );
    let folded = fold_surface_projection(None, &claim).unwrap();
    let replacement = event(
        2,
        "user/message",
        user("x"),
        Some(json!({"op":"replace","startSeq":0,"endSeq":0})),
    );
    assert!(fold_surface_projection(folded.claim.as_ref(), &replacement).is_ok());
    let mismatched = event(
        2,
        "user/message",
        user("x"),
        Some(json!({"op":"replace","startSeq":0,"endSeq":1})),
    );
    assert!(
        fold_surface_projection(folded.claim.as_ref(), &mismatched)
            .unwrap_err()
            .to_string()
            .contains("armed claim covers 0-0")
    );
    let unmetered = fold_surface_projection(None, &replacement).unwrap();
    assert_eq!(unmetered.delta_tokens, 0);
}
