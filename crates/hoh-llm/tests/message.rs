mod common;
use common::*;
use hoh_llm::*;
use serde_json::json;
#[test]
fn detaches_and_freezes_developer_tool_names_without_copying_their_definitions() {
    let mut content = vec![block(json!({"type":"tool-addition","toolName":"search"}))];
    let m = create_developer_message(NewDeveloperMessage {
        content: content.clone(),
        source: MessageSource::new("test"),
    });
    content[0].data["toolName"] = json!("changed");
    assert_eq!(m.role(), "developer");
    assert_eq!(
        value(m.content()),
        json!([{"type":"tool-addition","toolName":"search"}])
    );
    assert_eq!(freeze_message(&m).id(), m.id());
}
#[test]
fn bounds_producer_summaries_while_preserving_summaries_at_the_exact_limit() {
    let exact = "x".repeat(CONTEXT_SUMMARY_MAX_CHARS);
    assert_eq!(bound_context_summary(&exact), exact);
    assert_eq!(
        bound_context_summary(&(exact + "x")),
        format!("{}…", "x".repeat(119))
    );
    assert!(
        bound_context_summary(&"😀".repeat(61))
            .encode_utf16()
            .count()
            <= 120
    );
}
#[test]
fn preserves_system_prompt_attribution_on_immutable_system_messages() {
    let m = create_system_message("rule");
    assert_eq!(m.role(), "system");
    assert_eq!(value(&m)["source"], json!({"kind":"system-prompt"}));
    assert_eq!(m.content(), [ContentBlock::text("rule")]);
    assert!(create_system_message("").content().is_empty());
}
#[test]
fn assigns_identity_immediately_and_returns_a_detached_deep_frozen_message() {
    let mut content = vec![ContentBlock::text("original")];
    let m = create_user_message(NewUserMessage {
        content: content.clone(),
        source: MessageSource::new("test"),
    });
    assert_eq!(m.role(), "user");
    assert!(hoh_util_values::js_json_stringify(&value(&m)).starts_with(r#"{"content":[{"type":"text","text":"original"}],"source":{"kind":"test"},"role":"user","id":"#));
    assert!(!m.id().as_str().is_empty());
    content[0] = ContentBlock::text("changed");
    assert_eq!(m.content(), [ContentBlock::text("original")]);
}
#[test]
fn freezes_an_existing_identity_without_minting_a_replacement() {
    let m: Message = decode(
        json!({"id":"existing","role":"assistant","content":[{"type":"text","text":"answer"}],"source":{"kind":"model","provider":"test","model":"test"}}),
    );
    let frozen = freeze_message(&m);
    assert_eq!(frozen.id(), "existing");
    assert_eq!(m, frozen);
}
#[test]
fn fixes_the_assistant_role_and_model_source_kind_at_creation() {
    let m = create_assistant_message(NewAssistantMessage {
        content: vec![ContentBlock::text("answer")],
        source: decode(
            json!({"provider":"test-provider","model":"test-model","replayState":{"request":1}}),
        ),
    });
    let v = value(&m);
    assert_eq!(v["role"], "assistant");
    assert_eq!(
        v["source"],
        json!({"kind":"model","provider":"test-provider","model":"test-model","replayState":{"request":1}})
    );
    assert!(!m.id().as_str().is_empty());
}
#[test]
fn couples_tool_result_content_and_its_cited_call_seq_to_one_call_identity() {
    let m = create_tool_result_message(ToolResultMessageInput {
        call_id: "call-1".into(),
        content: vec![ContentBlock::text("result")],
        is_error: false,
    });
    let v = value(&m);
    assert_eq!(v["role"], "tool");
    assert_eq!(v["toolCallId"], "call-1");
    assert_eq!(v["isError"], false);
    assert_eq!(v["source"], json!({"kind":"tool","callId":"call-1"}));
    assert!(!m.id().as_str().is_empty());
}
#[test]
fn preserves_unknown_tags_explicit_null_and_field_order() {
    let s: MessageSource =
        decode(json!({"kind":"plugin","form":"notice","summary":"x","extra":null}));
    assert_eq!(value(s)["extra"], json!(null));
    let metadata: AssistantProviderMetadata =
        decode(json!({"provider":"p","model":"m","replayState":null}));
    assert_eq!(metadata.replay_state, Some(json!(null)));
    assert_eq!(value(metadata)["replayState"], json!(null));
    let block: ContentBlock = decode(json!({"type":"plugin-block","z":0,"a":1}));
    assert_eq!(
        hoh_util_values::js_json_stringify(&value(block)),
        r#"{"type":"plugin-block","z":0,"a":1}"#
    );
}
