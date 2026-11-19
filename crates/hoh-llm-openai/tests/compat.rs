use hoh_llm_openai::compat::{COMPAT_TABLE, ThinkingControl, ToolCallIds, effort_ids};
use hoh_llm_openai::{ServerKind, config::normalize_base_url};

#[test]
fn table_is_indexed_by_server_kind() {
    for kind in ServerKind::ALL {
        assert_eq!(COMPAT_TABLE[kind as usize].kind, kind);
        assert_eq!(kind.compat().kind, kind);
    }
}

#[test]
fn documented_quirks() {
    let c = |k: ServerKind| k.compat();
    assert_eq!(
        c(ServerKind::Generic).thinking_control,
        ThinkingControl::Unsupported
    );
    assert_eq!(
        c(ServerKind::Vllm).reasoning_fields,
        ["reasoning_content", "reasoning"]
    );
    assert_eq!(
        c(ServerKind::LlamaCpp).reasoning_fields,
        ["reasoning_content"]
    );
    assert_eq!(c(ServerKind::Ollama).reasoning_fields[0], "reasoning");
    assert_eq!(
        c(ServerKind::Ollama).thinking_control,
        ThinkingControl::ReasoningEffort
    );
    assert!(!c(ServerKind::Ollama).tool_choice_required);
    assert_eq!(c(ServerKind::Sglang).tool_call_ids, ToolCallIds::Synthesize);
    for kind in ServerKind::ALL {
        assert!(c(kind).stream_usage_option);
        let expected = if kind == ServerKind::OpenAi {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        assert_eq!(c(kind).max_tokens_field, expected);
        assert!(!c(kind).reasoning_fields.is_empty());
    }
}

#[test]
fn efforts_follow_the_control() {
    assert!(effort_ids(ThinkingControl::Unsupported).is_empty());
    let ids = |c| effort_ids(c).iter().map(|(id, _)| *id).collect::<Vec<_>>();
    assert_eq!(ids(ThinkingControl::ChatTemplateKwargs), ["off", "on"]);
    assert_eq!(
        ids(ThinkingControl::ReasoningEffort),
        ["off", "low", "medium", "high"]
    );
}

#[test]
fn base_url_normalization() {
    assert_eq!(
        normalize_base_url(" http://127.0.0.1:8080/v1/ ").unwrap(),
        "http://127.0.0.1:8080/v1"
    );
    assert_eq!(
        normalize_base_url("https://llm.example/").unwrap(),
        "https://llm.example"
    );
    assert!(normalize_base_url("127.0.0.1:8080").is_err());
}
