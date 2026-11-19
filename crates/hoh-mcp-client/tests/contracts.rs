use hoh_mcp_client::connection::{RECONNECT_DEFAULTS, reconnect_delay};
use hoh_mcp_client::diagnostic::{
    describe_error, disconnected_message, mcp_request_error, redact_secrets,
};
use hoh_mcp_client::server_context::instruction_text;
use hoh_mcp_client::templates::{credential_refs_in, resolve_template_string};
use hoh_mcp_client::tools::extract_text;
use hoh_mcp_client::{
    Config, McpConnectionStatus, McpConnectionStatusHandle, McpFailureReason, McpLocalError,
    McpLocalErrorCode, McpResult, McpStatusCell, ReconnectConfig, assert_template_syntax,
    public_tool_name, resolve_reconnect_policy, valid_server_name,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn http_config(expand_templates: bool) -> Config {
    Config::StreamableHttp {
        server_name: "docs".into(),
        url: "https://example.invalid/mcp".into(),
        headers: Default::default(),
        tool_call_timeout_ms: 60_000,
        startup_timeout_ms: None,
        close_timeout_ms: 5_000,
        expand_templates,
        fail_on_startup_error: false,
        max_instruction_bytes: None,
        reconnect: None,
    }
}

#[test]
fn server_name_contract_accepts_documented_alphabet() {
    assert!(valid_server_name("A_1-z"));
    assert!(valid_server_name(&"x".repeat(32)));
}

#[test]
fn server_name_contract_rejects_empty_long_and_punctuation() {
    assert!(!valid_server_name(""));
    assert!(!valid_server_name(&"x".repeat(33)));
    assert!(!valid_server_name("a.b"));
}

#[test]
fn clean_public_tool_name_is_verbatim() {
    assert_eq!(
        public_tool_name("memory", "search_docs"),
        "mcp__memory__search_docs"
    );
}

#[test]
fn lossy_public_tool_name_is_bounded_and_hashed() {
    let name = public_tool_name("server", &format!("bad name/{}", "x".repeat(100)));
    assert_eq!(name.len(), 64);
    assert!(
        name.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    );
}

#[test]
fn lossy_public_tool_names_do_not_collapse() {
    assert_ne!(public_tool_name("s", "a b"), public_tool_name("s", "a/b"));
}

#[test]
fn template_reference_order_and_duplicates_are_retained() {
    assert_eq!(
        credential_refs_in("${credential:A}/${credential:B}/${credential:A}"),
        ["A", "B", "A"]
    );
}

#[test]
fn template_syntax_accepts_literals_and_valid_tokens() {
    assert_template_syntax("https://${credential:HOST}/literal", "url").unwrap();
    assert_template_syntax("pre${unterminated", "url").unwrap();
}

#[test]
fn template_syntax_rejects_every_suspect_occurrence() {
    let error = assert_template_syntax("${credential:bad-name}", "url").unwrap_err();
    assert_eq!(
        error.to_string(),
        "url: \"${credential:bad-name}\" is not a template token — use ${credential:REF}"
    );
    assert_template_syntax("${x${credential:A}", "url").unwrap_err();
}

#[tokio::test]
async fn template_resolution_preserves_surrounding_text() {
    let value = resolve_template_string("a${credential:X}b", "url", |name| {
        let name = name.to_owned();
        Box::pin(async move { Ok(Some(format!("<{name}>"))) })
    })
    .await
    .unwrap();
    assert_eq!(value, "a<X>b");
}

#[tokio::test]
async fn missing_template_value_has_machine_code() {
    let error = resolve_template_string("${credential:X}", "url", |_| Box::pin(async { Ok(None) }))
        .await
        .unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        "MCP_CREDENTIAL_NOT_FOUND"
    );
}

#[test]
fn redaction_prefers_longer_secrets_and_ignores_empty() {
    assert_eq!(
        redact_secrets("abc ab abc", ["ab", "abc", ""]),
        "[redacted] [redacted] [redacted]"
    );
}

#[test]
fn describe_error_flattens_cause_chain() {
    let error = McpLocalError::with_cause(
        "outer",
        McpLocalErrorCode::Timeout,
        std::io::Error::other("inner"),
    );
    assert_eq!(describe_error(&error), "outer, cause: inner");
}

#[test]
fn reconnect_defaults_match_source() {
    assert_eq!(
        resolve_reconnect_policy(None, "reconnect").unwrap(),
        RECONNECT_DEFAULTS
    );
}

#[test]
fn reconnect_rejects_inverted_delays() {
    let config = ReconnectConfig {
        enabled: None,
        initial_delay_ms: Some(10),
        max_delay_ms: Some(9),
        max_attempts: None,
    };
    assert_eq!(
        resolve_reconnect_policy(Some(&config), "x")
            .unwrap_err()
            .to_string(),
        "x.initialDelayMs must be less than or equal to maxDelayMs"
    );
}

#[test]
fn reconnect_backoff_doubles_and_caps() {
    let mut policy = RECONNECT_DEFAULTS;
    policy.initial_delay_ms = 5;
    policy.max_delay_ms = 17;
    assert_eq!(
        [1, 2, 3, 4, 99].map(|n| reconnect_delay(policy, n)),
        [5, 10, 17, 17, 17]
    );
}

#[test]
fn config_defaults_are_applied_by_serde() {
    let parsed: Config = serde_json::from_value(
        json!({"transport":"streamable-http","serverName":"docs","url":"http://localhost"}),
    )
    .unwrap();
    assert_eq!(parsed.tool_call_timeout_ms(), 60_000);
    assert_eq!(parsed.close_timeout_ms(), 5_000);
    assert!(!parsed.expand_templates());
}

#[test]
fn config_round_trip_keeps_transport_tag() {
    let value = serde_json::to_value(http_config(false)).unwrap();
    assert_eq!(value["transport"], "streamable-http");
    assert_eq!(value["serverName"], "docs");
}

#[test]
fn reconnect_config_rejects_unknown_options() {
    let error = serde_json::from_value::<Config>(json!({
        "transport":"streamable-http",
        "serverName":"docs",
        "url":"http://localhost",
        "reconnect":{"foo":1}
    }))
    .unwrap_err();
    assert!(error.to_string().contains("unknown field `foo`"), "{error}");
}

#[test]
fn config_runtime_bounds_reject_zero_and_timer_overflow() {
    let max = hoh_timeout::MAX_TIMER_DELAY_MS;
    for value in [0, max + 1] {
        let mut config = http_config(false);
        if let Config::StreamableHttp {
            startup_timeout_ms, ..
        } = &mut config
        {
            *startup_timeout_ms = Some(value);
        }
        assert!(config.validate().unwrap_err().contains("startupTimeoutMs"));
    }
    for value in [0, max + 1] {
        let mut config = http_config(false);
        if let Config::StreamableHttp {
            close_timeout_ms, ..
        } = &mut config
        {
            *close_timeout_ms = value;
        }
        assert!(config.validate().unwrap_err().contains("closeTimeoutMs"));
    }

    let mut config = http_config(false);
    if let Config::StreamableHttp {
        max_instruction_bytes,
        ..
    } = &mut config
    {
        *max_instruction_bytes = Some(0);
    }
    assert!(
        config
            .validate()
            .unwrap_err()
            .contains("maxInstructionBytes")
    );
}

#[test]
fn not_connected_request_is_server_unavailable() {
    let error = mcp_request_error(
        Box::new(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "closed",
        )),
        "tool \"x\"",
        None,
    );
    assert_eq!(error.code(), "MCP_SERVER_UNAVAILABLE");
}

#[test]
fn disconnected_diagnostic_names_the_server() {
    assert_eq!(
        disconnected_message("docs"),
        "mcp-client(docs): server is disconnected"
    );
}

#[test]
fn status_cell_tracks_open_and_close_separately() {
    let cell = McpStatusCell::new(|_| {});
    assert!(cell.closure().close_confirmed);
    cell.opened();
    assert!(!cell.closure().close_confirmed);
    cell.record_closure(hoh_mcp_client::McpCloseResult {
        close_confirmed: true,
        cleanup_error: None,
    });
    assert!(cell.closure().close_confirmed);
}

#[test]
fn status_cell_notifies_in_publish_order() {
    let cell = McpStatusCell::new(|_| {});
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let _listener = cell.on_change(Arc::new(move |status| sink.lock().unwrap().push(status)));
    cell.update(McpConnectionStatus::Connecting { attempt: 2 });
    cell.update(McpConnectionStatus::Failed {
        reason: McpFailureReason::GaveUp,
        last_error: None,
    });
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[test]
fn status_cell_contains_panicking_observer() {
    let warnings = Arc::new(Mutex::new(0));
    let sink = warnings.clone();
    let cell = McpStatusCell::new(move |_| *sink.lock().unwrap() += 1);
    let _listener = cell.on_change(Arc::new(|_| panic!("observer")));
    cell.update(McpConnectionStatus::Connecting { attempt: 1 });
    assert_eq!(*warnings.lock().unwrap(), 1);
}

#[test]
fn instruction_text_is_attributed_and_literal() {
    assert_eq!(
        instruction_text("docs", "Use {{literal}}", 100).unwrap(),
        "### MCP server: docs\n\nUse {{literal}}"
    );
}

#[test]
fn instruction_text_enforces_utf8_bytes() {
    assert!(instruction_text("docs", "яя", 3).is_err());
}

#[test]
fn mcp_text_projection_preserves_order() {
    let content = vec![
        json!({"type":"text","text":"one"}),
        json!({"type":"resource_link","name":"guide","uri":"docs://guide"}),
        json!({"type":"text","text":"two"}),
    ];
    assert_eq!(
        extract_text(&content, "read"),
        "one\nResource link: guide (docs://guide)\ntwo"
    );
}

#[test]
fn mcp_text_projection_has_stable_placeholders() {
    let content = vec![
        json!({"type":"audio","mimeType":"audio/wav"}),
        json!({"type":"resource"}),
        Value::Null,
    ];
    let text = extract_text(&content, "x");
    assert!(text.contains("audio result unsupported"));
    assert!(text.contains("embedded resource unsupported"));
    assert!(text.contains("expected an object"));
}

#[test]
fn empty_mcp_result_has_visible_fallback() {
    assert_eq!(
        extract_text(&[], "empty"),
        "(empty returned no model-visible content)"
    );
}

#[test]
fn canonical_mcp_result_omits_absent_structured_content() {
    let value = serde_json::to_value(McpResult {
        content: vec![],
        structured_content: None,
    })
    .unwrap();
    assert_eq!(value, json!({"content":[]}));
}

#[test]
fn probe_outcome_uses_boolean_discriminator() {
    let outcome = hoh_mcp_client::McpProbeOutcome::Failure {
        stage: hoh_mcp_client::McpProbeStage::Connect,
        message: "no".into(),
        duration_ms: 4,
    };
    let value = serde_json::to_value(&outcome).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(
        serde_json::from_value::<hoh_mcp_client::McpProbeOutcome>(value).unwrap(),
        outcome
    );
}
