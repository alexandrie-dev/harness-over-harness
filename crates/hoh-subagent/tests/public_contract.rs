use hoh_attachment::{AttachmentError, AttachmentErrorCode};
use hoh_error::{BoxError, HarnessError};
use hoh_llm::{ContentBlock, MessageSource};
use hoh_session::SessionId;
use hoh_subagent::continuation_messages::{
    create_settlement_message, with_result_delivery_guidance,
};
use hoh_subagent::{
    SubagentDepthError, SubagentDepthErrorDetail, SubagentError, SubagentErrorCode, SubagentResult,
    SubagentStopReason, assert_subagent_max_depth, assert_usable_cwd, limit_subagent_diagnostic,
    normalize_subagent_diagnostic, reject_prompt, validate_control_request,
};
use hoh_timeout::AbortController;
use serde_json::{Value, json};

fn texts(blocks: &[ContentBlock]) -> Vec<&str> {
    blocks.iter().filter_map(ContentBlock::text_value).collect()
}

fn completed(output: Vec<ContentBlock>) -> SubagentResult {
    SubagentResult {
        output,
        structured: None,
        diagnostic: None,
        stop_reason: SubagentStopReason::Completed,
    }
}

#[test]
fn stop_reason_wire_vocabulary_and_extension_are_stable() {
    let cases = [
        (SubagentStopReason::Completed, "completed"),
        (SubagentStopReason::Aborted, "aborted"),
        (SubagentStopReason::Error, "error"),
        (SubagentStopReason::MaxTokens, "max-tokens"),
        (SubagentStopReason::Refusal, "refusal"),
        (
            SubagentStopReason::Other("provider-stop".into()),
            "provider-stop",
        ),
    ];
    for (reason, wire) in cases {
        assert_eq!(serde_json::to_value(&reason).unwrap(), json!(wire));
        assert_eq!(
            serde_json::from_value::<SubagentStopReason>(json!(wire)).unwrap(),
            reason
        );
    }
}

#[test]
fn all_subagent_error_codes_keep_the_typescript_wire_spelling() {
    let cases = [
        (
            SubagentErrorCode::UnsupportedCapability,
            "UNSUPPORTED_CAPABILITY",
        ),
        (SubagentErrorCode::DuplicateProvider, "DUPLICATE_PROVIDER"),
        (SubagentErrorCode::NoProvider, "NO_PROVIDER"),
        (SubagentErrorCode::ProviderChanged, "PROVIDER_CHANGED"),
        (SubagentErrorCode::PresetUnavailable, "PRESET_UNAVAILABLE"),
        (
            SubagentErrorCode::PresetSelectionDisabled,
            "PRESET_SELECTION_DISABLED",
        ),
        (SubagentErrorCode::PresetNotAllowed, "PRESET_NOT_ALLOWED"),
        (
            SubagentErrorCode::ModelSelectionDisabled,
            "MODEL_SELECTION_DISABLED",
        ),
        (
            SubagentErrorCode::ModelRouteNotAllowed,
            "MODEL_ROUTE_NOT_ALLOWED",
        ),
        (
            SubagentErrorCode::LlmProviderNotFound,
            "LLM_PROVIDER_NOT_FOUND",
        ),
        (
            SubagentErrorCode::ModelDoesNotSupportImages,
            "MODEL_DOES_NOT_SUPPORT_IMAGES",
        ),
        (SubagentErrorCode::DepthExceeded, "DEPTH_EXCEEDED"),
        (SubagentErrorCode::InvalidDepth, "INVALID_DEPTH"),
        (SubagentErrorCode::InvalidCwd, "INVALID_CWD"),
        (
            SubagentErrorCode::WorkingDirectoryUnavailable,
            "WORKING_DIRECTORY_UNAVAILABLE",
        ),
        (SubagentErrorCode::Unauthorized, "UNAUTHORIZED"),
        (
            SubagentErrorCode::ActivationLimitReached,
            "ACTIVATION_LIMIT_REACHED",
        ),
        (SubagentErrorCode::ActivationClosing, "ACTIVATION_CLOSING"),
        (
            SubagentErrorCode::ActivationTeardownFailed,
            "ACTIVATION_TEARDOWN_FAILED",
        ),
        (SubagentErrorCode::DuplicateChild, "DUPLICATE_CHILD"),
        (SubagentErrorCode::Draining, "DRAINING"),
        (SubagentErrorCode::InputClosed, "INPUT_CLOSED"),
        (
            SubagentErrorCode::StructuredOutputPendingInput,
            "STRUCTURED_OUTPUT_PENDING_INPUT",
        ),
        (SubagentErrorCode::Cancelled, "CANCELLED"),
        (SubagentErrorCode::NotResumable, "NOT_RESUMABLE"),
        (SubagentErrorCode::NotContinuable, "NOT_CONTINUABLE"),
        (
            SubagentErrorCode::ContinuationUnavailable,
            "CONTINUATION_UNAVAILABLE",
        ),
        (
            SubagentErrorCode::PersistenceUnavailable,
            "PERSISTENCE_UNAVAILABLE",
        ),
        (SubagentErrorCode::InvalidAgentName, "INVALID_AGENT_NAME"),
        (
            SubagentErrorCode::DuplicateAgentName,
            "DUPLICATE_AGENT_NAME",
        ),
        (SubagentErrorCode::AgentNameNotFound, "AGENT_NAME_NOT_FOUND"),
        (
            SubagentErrorCode::SubagentControlQueryUnavailable,
            "SUBAGENT_CONTROL_QUERY_UNAVAILABLE",
        ),
        (
            SubagentErrorCode::SubagentControlProjectionsUnavailable,
            "SUBAGENT_CONTROL_PROJECTIONS_UNAVAILABLE",
        ),
        (
            SubagentErrorCode::SubagentControlSessionStoreUnavailable,
            "SUBAGENT_CONTROL_SESSION_STORE_UNAVAILABLE",
        ),
    ];
    for (code, wire) in cases {
        assert_eq!(code.as_str(), wire);
        let error = SubagentError::error("message", code);
        assert_eq!(error.name(), SubagentError::NAME);
        assert_eq!(error.code(), wire);
    }
}

#[test]
fn public_name_grammar_and_paths_match_the_model_contract() {
    use hoh_subagent::names::{
        ROOT_AGENT_NAME, agent_address, is_agent_name, join_agent_path, parent_agent_path,
    };

    assert_eq!(ROOT_AGENT_NAME, "lead");
    for accepted in ["a", "agent-1", "z9", "a-b-c"] {
        assert!(is_agent_name(accepted), "{accepted}");
    }
    for rejected in ["", "lead", "A", "1a", "a_b", "a/child", "é"] {
        assert!(!is_agent_name(rejected), "{rejected}");
    }
    assert_eq!(join_agent_path(Some(""), Some("one")), Some("one".into()));
    assert_eq!(
        join_agent_path(Some("one"), Some("two")),
        Some("one/two".into())
    );
    assert_eq!(join_agent_path(None, Some("two")), None);
    assert_eq!(agent_address(Some(""), "root-id"), "lead");
    assert_eq!(agent_address(None, "anonymous-id"), "anonymous-id");
    assert_eq!(parent_agent_path("one/two/three"), "one/two");
}

#[test]
fn max_depth_uses_javascript_safe_integer_rules() {
    for accepted in [None, Some(0.0), Some(1.0), Some(9_007_199_254_740_991.0)] {
        assert_subagent_max_depth(accepted).unwrap();
    }
    for rejected in [
        -0.0,
        -1.0,
        0.5,
        f64::NAN,
        f64::INFINITY,
        9_007_199_254_740_992.0,
    ] {
        assert!(
            assert_subagent_max_depth(Some(rejected)).is_err(),
            "{rejected:?}"
        );
    }
}

#[test]
fn control_validation_accepts_only_the_two_declared_methods() {
    validate_control_request(
        "subagent.prompt",
        &json!({"childSessionId":"child", "delivery":"queue"}),
    )
    .unwrap();
    validate_control_request(
        "subagent.prompt",
        &json!({"childSessionId":"child", "delivery":"steer"}),
    )
    .unwrap();
    validate_control_request("subagent.interrupt", &json!({"childSessionId":"child"})).unwrap();
    assert!(
        validate_control_request(
            "subagent.prompt",
            &json!({"childSessionId":"child", "delivery":"later"}),
        )
        .is_err()
    );
    assert!(
        validate_control_request("subagent.unknown", &json!({"childSessionId":"child"})).is_err()
    );
}

#[test]
fn invalid_control_payload_exposes_zod_compatible_issues() {
    let error = validate_control_request(
        "subagent.prompt",
        &json!({"childSessionId":"", "delivery":"later"}),
    )
    .unwrap_err();
    assert_eq!(error.name(), "RemoteError");
    assert_eq!(error.code(), "gateway/bad-request");
    assert_eq!(error.message(), "invalid payload for subagent.prompt");
    let detail = error.detail::<Value>().unwrap();
    assert_eq!(detail["issues"][0]["path"], json!(["childSessionId"]));
    assert_eq!(detail["issues"][0]["minimum"], 1);
    assert_eq!(detail["issues"][1]["path"], json!(["delivery"]));
    assert_eq!(detail["issues"][1]["values"], json!(["queue", "steer"]));
}

fn rejected(error: BoxError, signal: &hoh_timeout::AbortSignal) -> HarnessError {
    reject_prompt(error, &SessionId::from("child"), signal)
}

#[test]
fn prompt_rejection_maps_actionable_subagent_codes_and_hides_the_rest() {
    let signal = AbortController::new().signal();
    let cases = [
        (SubagentErrorCode::NotResumable, "subagent/not-resumable"),
        (SubagentErrorCode::Unauthorized, "subagent/unauthorized"),
        (SubagentErrorCode::Draining, "subagent/delivery-unavailable"),
        (
            SubagentErrorCode::ActivationClosing,
            "subagent/delivery-unavailable",
        ),
        (
            SubagentErrorCode::ActivationLimitReached,
            "subagent/delivery-unavailable",
        ),
        (
            SubagentErrorCode::ContinuationUnavailable,
            "subagent/delivery-unavailable",
        ),
        (
            SubagentErrorCode::PersistenceUnavailable,
            "subagent/delivery-unavailable",
        ),
        (SubagentErrorCode::NoProvider, "gateway/internal"),
    ];
    for (thrown, expected) in cases {
        let error = rejected(Box::new(SubagentError::error("refused", thrown)), &signal);
        assert_eq!(error.code(), expected);
    }
    let internal = rejected("inbox exploded".into(), &signal);
    assert_eq!(internal.code(), "gateway/internal");
    assert_eq!(internal.message(), "subagent prompt failed");
}

#[test]
fn prompt_rejection_preserves_cancellation_from_signal_or_operation() {
    let controller = AbortController::new();
    controller.abort();
    let error = rejected(
        Box::new(SubagentError::error(
            "gone",
            SubagentErrorCode::NotResumable,
        )),
        &controller.signal(),
    );
    assert_eq!(error.code(), "gateway/cancelled");

    let error = rejected(
        Box::new(SubagentError::error(
            "stopped",
            SubagentErrorCode::Cancelled,
        )),
        &AbortController::new().signal(),
    );
    assert_eq!(error.code(), "gateway/cancelled");
}

#[test]
fn prompt_rejection_preserves_attachment_reason_and_model_image_refusal() {
    let signal = AbortController::new().signal();
    let attachment = rejected(
        Box::new(AttachmentError::new(
            "too many images",
            AttachmentErrorCode::TooManyImages,
        )),
        &signal,
    );
    assert_eq!(attachment.code(), "subagent/attachment-invalid");
    assert_eq!(attachment.message(), "too many images");
    assert_eq!(
        attachment.detail::<Value>().unwrap()["reason"],
        "TOO_MANY_IMAGES"
    );

    let model = rejected(
        Box::new(SubagentError::error(
            "model is text-only",
            SubagentErrorCode::ModelDoesNotSupportImages,
        )),
        &signal,
    );
    assert_eq!(model.code(), "subagent/attachment-invalid");
    assert_eq!(
        model.detail::<Value>().unwrap()["reason"],
        "MODEL_DOES_NOT_SUPPORT_IMAGES"
    );
}

#[test]
fn delivery_guidance_is_appended_without_changing_existing_prompt() {
    let prompt = vec![ContentBlock::text("task")];
    let with_guidance = with_result_delivery_guidance(prompt.clone());
    assert_eq!(with_guidance[0], prompt[0]);
    assert_eq!(with_guidance.len(), 2);
    assert!(
        with_guidance[1]
            .text_value()
            .unwrap()
            .contains("the parent does not receive your full transcript")
    );
}

#[test]
fn completed_settlement_preserves_only_nonempty_text_output() {
    let result = completed(vec![
        ContentBlock::text("first"),
        ContentBlock::text(""),
        ContentBlock::new("image", Default::default()),
        ContentBlock::text("last"),
    ]);
    let message =
        create_settlement_message(&SessionId::from("child"), "agent-1", &result, false, false);
    assert_eq!(
        texts(&message.content),
        vec![
            "Background subagent agent-1 finished. It cannot receive follow-up messages.",
            "Its closing message:",
            "first",
            "last",
        ]
    );
    assert_eq!(message.source.kind, "subagent-settled");
    assert_eq!(message.source.data["form"], "notice");
    assert_eq!(message.source.data["stopReason"], "completed");
    assert_eq!(message.source.data["senderSessionId"], "child");
}

#[test]
fn settlement_omits_closing_text_after_a_last_moment_message() {
    let result = completed(vec![ContentBlock::text("already relayed")]);
    let message =
        create_settlement_message(&SessionId::from("child"), "agent-1", &result, true, true);
    assert_eq!(
        texts(&message.content),
        vec![
            "Background subagent agent-1 finished and will do no further work unless you send it more.",
            "It sent you a message just before finishing, so its closing message is omitted.",
        ]
    );
}

#[test]
fn settlement_stop_reason_summaries_are_model_stable() {
    let cases = [
        (
            SubagentStopReason::Aborted,
            "was stopped before it finished.",
        ),
        (
            SubagentStopReason::MaxTokens,
            "ran out of room before it finished.",
        ),
        (SubagentStopReason::Refusal, "declined the task."),
        (SubagentStopReason::Error, "failed before it finished."),
        (
            SubagentStopReason::Other("provider-stop".into()),
            "ended abnormally (provider-stop) before it finished.",
        ),
    ];
    for (stop_reason, suffix) in cases {
        let result = SubagentResult {
            output: vec![],
            structured: None,
            diagnostic: None,
            stop_reason,
        };
        let message =
            create_settlement_message(&SessionId::from("child"), "agent-1", &result, false, false);
        assert!(message.content[0].text_value().unwrap().ends_with(suffix));
        assert_eq!(
            message.content[1].text_value(),
            Some("It left no closing message.")
        );
    }
}

#[test]
fn structured_settlement_uses_javascript_json_stringify_semantics() {
    let result = SubagentResult {
        output: vec![],
        structured: Some(json!({"answer": 1.0, "missing": null})),
        diagnostic: None,
        stop_reason: SubagentStopReason::Completed,
    };
    let message =
        create_settlement_message(&SessionId::from("child"), "agent-1", &result, false, false);
    assert_eq!(
        message.content[2].text_value(),
        Some("Structured result: {\"answer\":1,\"missing\":null}")
    );
}

#[test]
fn diagnostic_is_delivered_after_structured_result() {
    let result = SubagentResult {
        output: vec![],
        structured: Some(json!({"ok": true})),
        diagnostic: Some("provider diagnostic".into()),
        stop_reason: SubagentStopReason::Error,
    };
    let message =
        create_settlement_message(&SessionId::from("child"), "agent-1", &result, false, false);
    assert_eq!(
        message.content[2].text_value(),
        Some("Structured result: {\"ok\":true}")
    );
    assert_eq!(message.content[3].text_value(), Some("provider diagnostic"));
}

#[test]
fn diagnostic_limit_is_byte_bounded_and_utf8_safe() {
    let exact = "x".repeat(4_096);
    assert_eq!(limit_subagent_diagnostic(&exact), exact);
    let limited = limit_subagent_diagnostic(&"é".repeat(3_000));
    assert!(limited.len() <= 4_096);
    assert!(limited.ends_with("\n[diagnostic truncated]"));
    assert!(limited.is_char_boundary(limited.len()));
}

#[test]
fn usable_cwd_accepts_an_absolute_directory_and_rejects_relative_or_missing_paths() {
    let base = std::env::temp_dir();
    assert_eq!(
        assert_usable_cwd("provider", "config cwd", base.to_str().unwrap()).unwrap(),
        base.to_str().unwrap()
    );

    let relative = assert_usable_cwd("provider", "config cwd", "relative/path").unwrap_err();
    let relative = HarnessError::of(relative.as_ref()).unwrap();
    assert_eq!(relative.code(), "INVALID_CWD");
    assert!(relative.message().contains("must be an absolute path"));

    let missing = base.join(format!("hoh-subagent-missing-{}", std::process::id()));
    let missing =
        assert_usable_cwd("provider", "config cwd", missing.to_str().unwrap()).unwrap_err();
    let missing = HarnessError::of(missing.as_ref()).unwrap();
    assert_eq!(missing.code(), "INVALID_CWD");
    assert!(missing.message().contains("not an accessible directory"));
}

#[test]
fn usable_cwd_rejects_an_existing_file() {
    let path = std::env::temp_dir().join(format!(
        "hoh-subagent-cwd-file-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::write(&path, b"not a directory\n").unwrap();
    let error = assert_usable_cwd("provider", "config cwd", path.to_str().unwrap()).unwrap_err();
    std::fs::remove_file(&path).unwrap();
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "INVALID_CWD");
    assert!(error.message().contains("not an accessible directory"));
}

#[cfg(unix)]
#[test]
fn usable_cwd_rejects_a_directory_without_search_permission() {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::temp_dir().join(format!("hoh-subagent-cwd-noexec-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let error = assert_usable_cwd("provider", "config cwd", path.to_str().unwrap()).unwrap_err();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir(&path).unwrap();
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "INVALID_CWD");
    assert!(error.message().contains("not an accessible directory"));
}

#[cfg(unix)]
#[test]
fn usable_cwd_uses_effective_owner_search_permission_not_any_execute_bit() {
    use std::os::unix::fs::PermissionsExt;

    for mode in [0o010, 0o001] {
        let path = std::env::temp_dir().join(format!(
            "hoh-subagent-cwd-wrong-class-{}-{mode:o}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        let error =
            assert_usable_cwd("provider", "config cwd", path.to_str().unwrap()).unwrap_err();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir(&path).unwrap();
        let error = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), "INVALID_CWD");
        assert!(error.message().contains("not an accessible directory"));
    }
}

#[test]
fn normalization_changes_only_an_oversized_diagnostic() {
    let original = SubagentResult {
        output: vec![ContentBlock::text("partial")],
        structured: Some(json!({"ok": false})),
        diagnostic: Some("x".repeat(5_000)),
        stop_reason: SubagentStopReason::Error,
    };
    let normalized = normalize_subagent_diagnostic(original.clone());
    assert_eq!(normalized.output, original.output);
    assert_eq!(normalized.structured, original.structured);
    assert_eq!(normalized.stop_reason, original.stop_reason);
    assert_eq!(normalized.diagnostic.unwrap().len(), 4_096);
}

#[test]
fn subagent_error_preserves_name_message_code_and_cause() {
    let error = SubagentError::with_cause(
        "activation failed",
        SubagentErrorCode::ActivationTeardownFailed,
        "teardown failed",
    );
    assert_eq!(error.name(), "SubagentError");
    assert_eq!(error.message(), "activation failed");
    assert_eq!(error.code(), "ACTIVATION_TEARDOWN_FAILED");
    assert_eq!(error.cause().unwrap().to_string(), "teardown failed");
    let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(error);
    assert_eq!(
        HarnessError::of(boxed.as_ref()).unwrap().code(),
        "ACTIVATION_TEARDOWN_FAILED"
    );
}

#[test]
fn subagent_depth_error_constructor_preserves_source_identity_and_fields() {
    let error = SubagentDepthError::new(2, 1);
    assert_eq!(error.name(), "SubagentDepthError");
    assert_eq!(error.code(), "DEPTH_EXCEEDED");
    assert_eq!(error.message(), "subagent depth 2 exceeds maxDepth 1");
    assert_eq!(
        error.detail::<SubagentDepthErrorDetail>(),
        Some(&SubagentDepthErrorDetail {
            attempted_depth: 2,
            max_depth: 1,
        })
    );
}

#[test]
fn message_source_shape_remains_flattened_for_browser_consumers() {
    let mut source = MessageSource::new("subagent-settled");
    source.data.insert("form".into(), json!("notice"));
    source.data.insert("summary".into(), json!("done"));
    assert_eq!(
        serde_json::to_value(source).unwrap(),
        json!({"kind":"subagent-settled", "form":"notice", "summary":"done"})
    );
}
