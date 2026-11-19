use hoh_llm::{ContentBlock, Message};
use hoh_session::EpochHeader;
use hoh_util_values::js_json_stringify;
use serde_json::Value;

const CHARS_PER_TOKEN: usize = 4;
const BLOCK_OVERHEAD: u64 = 4;
pub const ROLE_OVERHEAD: u64 = 4;

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn density(characters: usize) -> u64 {
    characters.div_ceil(CHARS_PER_TOKEN) as u64
}

fn block_value(block: &ContentBlock) -> Value {
    serde_json::to_value(block).expect("ContentBlock is JSON")
}

/// Structural JSON price for a merge-extended block or image reference.
pub fn estimate_structural_block(block: &ContentBlock) -> u64 {
    let mut value = block_value(block);
    if block.r#type == "image" {
        value
            .as_object_mut()
            .map(|object| object.shift_remove("offloaded"));
    }
    BLOCK_OVERHEAD + density(utf16_len(&js_json_stringify(&value)))
}

/// Price content recursively under the fixed density heuristic.
pub fn estimate_content(blocks: &[ContentBlock]) -> u64 {
    blocks.iter().fold(0_u64, |tokens, block| {
        let price = match block.r#type.as_str() {
            "text" | "reasoning" => {
                density(utf16_len(block.text_value().unwrap_or_default())) + BLOCK_OVERHEAD
            }
            "tool-call" => {
                let name = block
                    .data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = block
                    .data
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                density(utf16_len(name)) + density(utf16_len(arguments)) + BLOCK_OVERHEAD
            }
            _ => estimate_structural_block(block),
        };
        tokens.saturating_add(price)
    })
}

/// Price a rendered system prompt without block framing overhead.
pub fn estimate_system_message(message: &Message) -> u64 {
    if message.content().is_empty() {
        return 0;
    }
    let characters = message.content().iter().fold(0, |total, block| {
        total
            + if block.r#type == "text" {
                utf16_len(block.text_value().unwrap_or_default())
            } else {
                utf16_len(&js_json_stringify(&block_value(block)))
            }
    });
    density(characters) + ROLE_OVERHEAD
}

/// Heuristically price one model-visible message.
pub fn estimate_message(message: &Message) -> u64 {
    if message.role() == "system" {
        estimate_system_message(message)
    } else {
        estimate_content(message.content()) + ROLE_OVERHEAD
    }
}

/// Price tool schemas from a canonical request envelope.
pub fn estimate_tools_tokens(header: Option<&EpochHeader>) -> u64 {
    let Some(tools) = header
        .and_then(|header| header.get("tools"))
        .and_then(Value::as_array)
        .filter(|tools| !tools.is_empty())
    else {
        return 0;
    };
    density(utf16_len(&js_json_stringify(&Value::Array(tools.clone())))) + BLOCK_OVERHEAD
}
