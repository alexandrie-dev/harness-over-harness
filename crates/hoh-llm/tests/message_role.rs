//! `Message` is read by its `role`, never by the first variant whose fields fit.
use hoh_llm::*;
use serde_json::{Value, json};

fn every_role() -> Vec<(&'static str, Message)> {
    let assistant = create_assistant_message(NewAssistantMessage {
        content: vec![ContentBlock::text("answer")],
        source: AssistantProviderMetadata {
            provider: "mock".into(),
            model: "mock".into(),
            replay_state: None,
        },
    });
    vec![
        ("system", create_system_message("rules").into_message()),
        (
            "developer",
            create_developer_message(NewDeveloperMessage {
                content: vec![ContentBlock::text("note")],
                source: MessageSource::new("test"),
            })
            .into_message(),
        ),
        (
            "user",
            create_user_message(NewUserMessage {
                content: vec![ContentBlock::text("question")],
                source: MessageSource::new("user"),
            })
            .into_message(),
        ),
        ("assistant", assistant.into_message()),
        (
            "tool",
            create_tool_result_message(ToolResultMessageInput {
                call_id: "call-1".into(),
                content: vec![ContentBlock::text("result")],
                is_error: false,
            })
            .into_message(),
        ),
    ]
}

trait IntoMessage {
    fn into_message(self) -> Message;
}
macro_rules! into_message {
    ($($type:ident => $variant:ident),*) => {$(
        impl IntoMessage for $type {
            fn into_message(self) -> Message { Message::$variant(self) }
        }
    )*};
}
into_message!(SystemMessage => System, DeveloperMessage => Developer, UserMessage => User,
    AssistantMessage => Assistant, ToolResultMessage => Tool);

#[test]
fn reads_each_role_as_its_own_variant() {
    for (role, message) in every_role() {
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["role"], role);
        let read: Message = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(read.role(), role, "{json}");
        assert_eq!(read, message);
    }
}

#[test]
fn reads_each_role_through_a_request_message() {
    for (role, message) in every_role() {
        let json = serde_json::to_value(&message).unwrap();
        let read: RequestMessage = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(read.role(), role, "{json}");
        assert!(matches!(read, RequestMessage::Durable(_)));
    }
}

#[test]
fn a_user_message_is_not_read_as_a_developer_message() {
    let json = json!({"role": "user", "content": [{"type": "text", "text": "q"}],
        "source": {"kind": "user"}, "id": "m1"});
    assert_eq!(
        serde_json::from_value::<Message>(json).unwrap().role(),
        "user"
    );
}

#[test]
fn rejects_an_unknown_or_malformed_role() {
    let base = json!({"content": [], "source": {"kind": "user"}, "id": "m1"});
    for (role, expected) in [
        (json!("robot"), "unknown message role \"robot\""),
        (json!(7), "message role must be a string"),
    ] {
        let mut value = base.clone();
        value["role"] = role;
        let error = serde_json::from_value::<Message>(value).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    let error = serde_json::from_value::<Message>(base).unwrap_err();
    assert!(
        error.to_string().contains("message role is missing"),
        "{error}"
    );
    let missing_id: Value = json!({"role": "user", "content": [], "source": {"kind": "user"}});
    assert!(serde_json::from_value::<Message>(missing_id).is_err());
}

#[test]
fn a_bare_user_input_is_still_a_request_input() {
    let read: RequestMessage =
        serde_json::from_value(json!({"role": "user", "content": [{"type": "text", "text": "q"}]}))
            .unwrap();
    assert!(matches!(read, RequestMessage::Input(_)));
}
