//! Canonical selection of a child's final assistant output.

use hoh_llm::{AssistantStreamRecord, ContentBlock, join_assistant_stream_text};
use hoh_session::SessionEvent;

#[derive(Default)]
pub struct AssistantOutputFold {
    message: Option<Vec<ContentBlock>>,
    partial: String,
}

impl AssistantOutputFold {
    pub fn push(&mut self, event: &SessionEvent) {
        if event.event_type() == "assistant/message"
            && let Some(content) = event
                .data()
                .get("message")
                .and_then(|message| message.get("content"))
            && let Ok(content) = serde_json::from_value::<Vec<ContentBlock>>(content.clone())
            && !content.is_empty()
        {
            self.message = Some(content);
        }
        if matches!(
            event.event_type(),
            "assistant/message" | "assistant/attempt"
        ) && let Some(stream) = event.data().get("stream")
            && let Ok(stream) = serde_json::from_value::<Vec<AssistantStreamRecord>>(stream.clone())
        {
            self.push_text(&join_assistant_stream_text(&stream));
        }
    }

    pub fn push_text(&mut self, text: &str) {
        self.partial.push_str(text);
    }

    pub fn collect(&self) -> Option<Vec<ContentBlock>> {
        self.message.clone().or_else(|| {
            (!self.partial.is_empty()).then(|| vec![ContentBlock::text(self.partial.clone())])
        })
    }
}

pub fn final_assistant_output(
    events: &[std::sync::Arc<SessionEvent>],
) -> Option<Vec<ContentBlock>> {
    let mut fold = AssistantOutputFold::default();
    for event in events {
        fold.push(event);
    }
    fold.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    fn event(seq: u64, kind: &str, data: serde_json::Value) -> Arc<SessionEvent> {
        Arc::new(SessionEvent::from_value(json!({
            "seq": seq, "time": seq, "type": kind, "data": data
        })))
    }

    #[test]
    fn last_non_empty_assistant_message_wins() {
        let events = vec![
            event(
                0,
                "assistant/message",
                json!({ "message": { "content": [{ "type": "text", "text": "first" }] } }),
            ),
            event(
                1,
                "assistant/message",
                json!({ "message": { "content": [] } }),
            ),
            event(
                2,
                "assistant/message",
                json!({ "message": { "content": [{ "type": "text", "text": "last" }] } }),
            ),
        ];
        assert_eq!(
            final_assistant_output(&events).unwrap()[0].text_value(),
            Some("last")
        );
    }

    #[test]
    fn stream_text_is_fallback_and_absence_stays_absent() {
        let mut fold = AssistantOutputFold::default();
        assert_eq!(fold.collect(), None);
        fold.push_text("partial");
        assert_eq!(fold.collect().unwrap()[0].text_value(), Some("partial"));
    }

    #[test]
    fn non_empty_message_beats_stream_text_before_and_after_it() {
        let mut fold = AssistantOutputFold::default();
        fold.push_text("earlier partial");
        fold.push(&event(
            0,
            "assistant/message",
            json!({
                "stream": [],
                "message": { "content": [{ "type": "text", "text": "complete answer" }] }
            }),
        ));
        fold.push_text("later partial");
        assert_eq!(
            fold.collect().unwrap()[0].text_value(),
            Some("complete answer")
        );
    }

    #[test]
    fn textless_content_is_still_a_non_empty_message() {
        let events = vec![event(
            0,
            "assistant/message",
            json!({
                "stream": [],
                "message": { "content": [{ "type": "reasoning", "text": "complete reasoning" }] }
            }),
        )];
        assert_eq!(
            final_assistant_output(&events).unwrap()[0].r#type,
            "reasoning"
        );
    }

    #[test]
    fn empty_text_chunks_are_no_ops() {
        let mut fold = AssistantOutputFold::default();
        fold.push_text("partial ");
        fold.push_text("");
        fold.push_text("answer");
        assert_eq!(
            fold.collect().unwrap()[0].text_value(),
            Some("partial answer")
        );
    }
}
