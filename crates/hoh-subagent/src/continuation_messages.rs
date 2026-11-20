//! Durable, model-visible messages owned by continuable-subagent orchestration.

use hoh_agent::Agent;
use hoh_llm::{
    ContentBlock, MessageSource, NewUserMessage, UserMessage, bound_context_summary,
    create_user_message,
};
use hoh_session::SessionId;
use hoh_util_values::js_json_stringify;
use serde_json::{Value, json};

use crate::types::{SubagentResult, SubagentStopReason};

pub fn create_agent_message(
    sender: &dyn Agent,
    sender_name: &str,
    content: Vec<ContentBlock>,
) -> UserMessage {
    let mut source = MessageSource::new("agent-message");
    source
        .data
        .insert("form".into(), Value::String("relay".into()));
    source.data.insert(
        "senderSessionId".into(),
        Value::String(sender.id().as_str().into()),
    );
    let mut combined = vec![ContentBlock::text(format!(
        "Agent {sender_name} sent a message: "
    ))];
    combined.extend(content);
    create_user_message(NewUserMessage {
        content: combined,
        source,
    })
}

pub fn with_result_delivery_guidance(mut prompt: Vec<ContentBlock>) -> Vec<ContentBlock> {
    prompt.push(ContentBlock::text(
        "Your final answer is delivered automatically if your parent is still online. Include the result and relevant evidence in that answer; the parent does not receive your full transcript, tool output, or reasoning. If your last action is a message to your parent, the completion notice omits your final answer; include the result and relevant evidence in that message.",
    ));
    prompt
}

fn settlement_summary(
    child_name: &str,
    stop_reason: SubagentStopReason,
    continuable: bool,
) -> String {
    let subject = format!("Background subagent {child_name}");
    match stop_reason {
        SubagentStopReason::Completed if continuable => {
            format!("{subject} finished and will do no further work unless you send it more.")
        }
        SubagentStopReason::Completed => {
            format!("{subject} finished. It cannot receive follow-up messages.")
        }
        SubagentStopReason::Aborted => format!("{subject} was stopped before it finished."),
        SubagentStopReason::MaxTokens => {
            format!("{subject} ran out of room before it finished.")
        }
        SubagentStopReason::Refusal => format!("{subject} declined the task."),
        SubagentStopReason::Error => format!("{subject} failed before it finished."),
        SubagentStopReason::Other(reason) => {
            format!("{subject} ended abnormally ({reason}) before it finished.")
        }
    }
}

pub fn create_settlement_message(
    child_id: &SessionId,
    child_name: &str,
    terminal: &SubagentResult,
    continuable: bool,
    omit_closing_text: bool,
) -> UserMessage {
    let summary = settlement_summary(child_name, terminal.stop_reason.clone(), continuable);
    let closing_text = terminal
        .output
        .iter()
        .filter(|block| block.r#type == "text" && block.text_value().is_some_and(|v| !v.is_empty()))
        .cloned()
        .collect::<Vec<_>>();
    let mut content = vec![ContentBlock::text(summary.clone())];
    if omit_closing_text {
        content.push(ContentBlock::text(
            "It sent you a message just before finishing, so its closing message is omitted.",
        ));
    } else if closing_text.is_empty() {
        content.push(ContentBlock::text("It left no closing message."));
    } else {
        content.push(ContentBlock::text("Its closing message:"));
        content.extend(closing_text);
    }
    if let Some(structured) = &terminal.structured {
        content.push(ContentBlock::text(format!(
            "Structured result: {}",
            js_json_stringify(structured)
        )));
    }
    if let Some(diagnostic) = &terminal.diagnostic {
        content.push(ContentBlock::text(diagnostic));
    }
    let mut source = MessageSource::new("subagent-settled");
    source
        .data
        .insert("form".into(), Value::String("notice".into()));
    source.data.insert(
        "summary".into(),
        Value::String(bound_context_summary(&summary)),
    );
    source.data.insert(
        "stopReason".into(),
        Value::String(terminal.stop_reason.as_str().into()),
    );
    source
        .data
        .insert("senderSessionId".into(), json!(child_id.as_str()));
    create_user_message(NewUserMessage { content, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_guidance_is_byte_exact() {
        let value = with_result_delivery_guidance(vec![]);
        assert_eq!(value.len(), 1);
        assert!(
            value[0]
                .text_value()
                .unwrap()
                .ends_with("include the result and relevant evidence in that message.")
        );
    }

    #[test]
    fn settlement_filters_non_text_closing_blocks() {
        let result = SubagentResult {
            output: vec![ContentBlock::new("image", Default::default())],
            structured: None,
            diagnostic: None,
            stop_reason: SubagentStopReason::Completed,
        };
        let message = create_settlement_message(&"child".into(), "agent-1", &result, true, false);
        assert_eq!(
            message.content[1].text_value(),
            Some("It left no closing message.")
        );
    }
}
