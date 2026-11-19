//! Message and session helpers shared by the test files.
#![allow(dead_code)]

use hoh_llm::{
    AssistantProviderMetadata, ContentBlock, MessageSource, NewAssistantMessage, NewUserMessage,
    ToolCallId, ToolResultMessageInput, create_assistant_message, create_tool_result_message,
    create_user_message,
};
use serde_json::{Value, json};

/// `image(name)` of the source tests: an input image with a fixed attachment.
pub fn image_named(name: &str) -> Value {
    json!({
        "type": "image",
        "attachment": {
            "attachmentId": format!("sha256:{}", "a".repeat(64)),
            "name": name,
            "mediaType": "image/png",
            "bytes": 1,
            "width": 1,
            "height": 1
        }
    })
}

/// `image` of `projection.spec.ts`: no name.
pub fn image() -> Value {
    json!({
        "type": "image",
        "attachment": {
            "attachmentId": format!("sha256:{}", "a".repeat(64)),
            "mediaType": "image/png",
            "bytes": 1,
            "width": 1,
            "height": 1
        }
    })
}

pub fn text(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn blocks(content: Vec<Value>) -> Vec<ContentBlock> {
    content
        .into_iter()
        .map(|block| serde_json::from_value(block).expect("content block"))
        .collect()
}

/// `createUserMessage({ content, source: { kind: 'user' } })` as JSON.
pub fn user_message(content: Vec<Value>) -> Value {
    serde_json::to_value(create_user_message(NewUserMessage {
        content: blocks(content),
        source: MessageSource::new("user"),
    }))
    .expect("user message")
}

/// `createToolResultMessage({ callId, isError: false, content })` as JSON.
pub fn tool_result_message(call_id: &str, content: Vec<Value>) -> Value {
    serde_json::to_value(create_tool_result_message(ToolResultMessageInput {
        call_id: ToolCallId::from(call_id),
        content: blocks(content),
        is_error: false,
    }))
    .expect("tool result message")
}

/// `createAssistantMessage({ content, source: { provider: 'mock', model: 'mock' } })` as JSON.
pub fn assistant_message(content: Vec<Value>) -> Value {
    serde_json::to_value(create_assistant_message(NewAssistantMessage {
        content: blocks(content),
        source: AssistantProviderMetadata {
            provider: "mock".into(),
            model: "mock".into(),
            replay_state: None,
        },
    }))
    .expect("assistant message")
}
