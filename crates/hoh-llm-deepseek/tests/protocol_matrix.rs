use hoh_llm::StreamChunk;
use hoh_llm_deepseek::sse::{SseEvent, SseParser, decode_event};
use hoh_llm_deepseek::translate::Translator;
use hoh_llm_deepseek::transport::provider_error;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};

fn feed_all(values: &[Value]) -> Result<Vec<StreamChunk>, hoh_error::HarnessError> {
    let mut translator = Translator::new("deepseek-flash");
    let mut chunks = Vec::new();
    for value in values {
        chunks.extend(translator.feed(value)?);
    }
    translator.finish()?;
    Ok(chunks)
}

fn start() -> Value {
    json!({"type":"message_start","message":{"usage":{"input_tokens":12,"output_tokens":1,"cache_read_input_tokens":30,"cache_creation_input_tokens":7}}})
}

fn finish(reason: &str) -> [Value; 2] {
    [
        json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":5}}),
        json!({"type":"message_stop"}),
    ]
}

#[test]
fn translates_initial_text_thinking_tools_usage_and_replay() {
    let mut values = vec![
        start(),
        json!({"type":"content_block_start","index":4,"content_block":{"type":"thinking","thinking":"first","signature":""}}),
        json!({"type":"content_block_delta","index":4,"delta":{"type":"thinking_delta","thinking":" thought"}}),
        json!({"type":"content_block_delta","index":4,"delta":{"type":"signature_delta","signature":"signature"}}),
        json!({"type":"content_block_stop","index":4}),
        json!({"type":"content_block_start","index":9,"content_block":{"type":"tool_use","id":"call_1","name":"read","input":{}}}),
        json!({"type":"content_block_delta","index":9,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
        json!({"type":"content_block_delta","index":9,"delta":{"type":"input_json_delta","partial_json":" \"a\"}"}}),
        json!({"type":"content_block_stop","index":9}),
        json!({"type":"future_event"}),
    ];
    values.extend(finish("tool_use"));
    let chunks = feed_all(&values).unwrap();

    let starts = chunks
        .iter()
        .filter_map(|chunk| match chunk {
            StreamChunk::BlockStart { index, .. } => Some(*index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(starts, [0, 1]);
    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        StreamChunk::ReasoningDelta { text, .. } if text == "first"
    )));
    assert!(chunks.iter().any(|chunk| matches!(
        chunk,
        StreamChunk::BlockEnd { block, .. }
            if block.r#type == "tool-call"
                && block.data.get("id").and_then(Value::as_str) == Some("call_1")
                && block.data.get("name").and_then(Value::as_str) == Some("read")
                && block.data.get("arguments").and_then(Value::as_str) == Some("{\"path\": \"a\"}")
    )));
    let usage = chunks.iter().find_map(|chunk| match chunk {
        StreamChunk::Usage { usage, .. } => Some(usage),
        _ => None,
    });
    assert_eq!(usage.and_then(|usage| usage.total_tokens), Some(54));
    assert!(matches!(
        chunks.last(),
        Some(StreamChunk::Finish { reason, .. }) if reason.kind == "tool-calls"
    ));
}

#[test]
fn preserves_initial_tool_input_without_deltas() {
    for input in [json!({}), json!({"key":"initial"})] {
        let mut values = vec![
            start(),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"read","input":input.clone()}}),
            json!({"type":"content_block_stop","index":0}),
        ];
        values.extend(finish("tool_use"));
        let chunks = feed_all(&values).unwrap();
        let arguments = chunks.iter().find_map(|chunk| match chunk {
            StreamChunk::BlockEnd { block, .. } if block.r#type == "tool-call" => {
                block.data.get("arguments").and_then(Value::as_str)
            }
            _ => None,
        });
        let expected = input.to_string();
        assert_eq!(arguments, Some(expected.as_str()));
    }
}

#[test]
fn rejects_malformed_event_ordering_and_fields() {
    let text_start =
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}});
    let cases = vec![
        vec![start(), start()],
        vec![text_start.clone()],
        vec![
            start(),
            json!({"type":"content_block_start","index":-1,"content_block":{"type":"text","text":""}}),
        ],
        vec![start(), text_start.clone(), text_start.clone()],
        vec![start(), json!({"type":"content_block_stop","index":0})],
        vec![
            start(),
            text_start.clone(),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_stop","index":0}),
        ],
        vec![
            start(),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":2}}),
        ],
        vec![
            start(),
            text_start.clone(),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"bad"}}),
        ],
        vec![
            start(),
            json!({"type":"message_delta","delta":{"stop_reason":"mystery"}}),
        ],
        vec![start(), json!({"type":"message_stop"})],
        vec![
            start(),
            json!({"type":"message_delta","delta":{},"usage":{"input_tokens":-1}}),
        ],
        vec![
            start(),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
            text_start,
        ],
        vec![
            start(),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"","name":"read","input":{}}}),
        ],
        vec![
            start(),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"x","name":"read","input":[]}}),
        ],
    ];
    for (case, values) in cases.into_iter().enumerate() {
        let mut translator = Translator::new("deepseek-flash");
        let error = values
            .iter()
            .find_map(|value| translator.feed(value).err())
            .unwrap_or_else(|| translator.finish().unwrap_err());
        assert_eq!(error.code(), "MALFORMED_RESPONSE", "case {case}");
    }
}

#[test]
fn rejects_completed_non_object_tool_json() {
    for partial_json in ["{", "[]"] {
        let mut values = vec![
            start(),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"read","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":partial_json}}),
            json!({"type":"content_block_stop","index":0}),
        ];
        values.extend(finish("tool_use"));
        assert_eq!(feed_all(&values).unwrap_err().code(), "MALFORMED_RESPONSE");
    }
}

#[test]
fn maps_terminal_reasons_and_response_failures() {
    for (native, expected) in [
        ("end_turn", "stop"),
        ("stop_sequence", "stop"),
        ("tool_use", "tool-calls"),
        ("max_tokens", "max-tokens"),
    ] {
        let mut values = vec![
            start(),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"ok"}}),
            json!({"type":"content_block_stop","index":0}),
        ];
        values.extend(finish(native));
        assert!(matches!(
            feed_all(&values).unwrap().last(),
            Some(StreamChunk::Finish { reason, .. }) if reason.kind == expected
        ));
    }

    let unsupported = [
        start(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"x"}}),
    ];
    let mut translator = Translator::new("m");
    translator.feed(&unsupported[0]).unwrap();
    assert_eq!(
        translator.feed(&unsupported[1]).unwrap_err().code(),
        "UNSUPPORTED_CONTENT"
    );

    let mut empty = vec![start()];
    empty.extend(finish("end_turn"));
    assert_eq!(feed_all(&empty).unwrap_err().code(), "EMPTY_RESPONSE");
}

#[test]
fn frames_utf8_bytewise_bom_comments_and_crlf() {
    let source = "\u{feff}: heartbeat\r\n\r\nevent: ping\r\ndata: {\"type\":\"ping\"}\r\n\r\nevent: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"text\":\"世界\"}\r\n\r\n";
    let mut parser = SseParser::new();
    let mut events = Vec::new();
    for byte in source.as_bytes() {
        events.extend(parser.push(std::slice::from_ref(byte)).unwrap());
    }
    assert_eq!(events.len(), 2);
    assert_eq!(decode_event(events[0].clone()).unwrap()["type"], "ping");
    assert_eq!(decode_event(events[1].clone()).unwrap()["text"], "世界");
    assert!(parser.finish().is_none());
}

#[test]
fn rejects_malformed_sse_frames_and_normalizes_in_band_errors() {
    for frame in [
        SseEvent {
            event: None,
            data: "not-json".into(),
        },
        SseEvent {
            event: None,
            data: "[]".into(),
        },
        SseEvent {
            event: Some("ping".into()),
            data: "{\"type\":\"other\"}".into(),
        },
        SseEvent {
            event: None,
            data: "{}".into(),
        },
    ] {
        assert_eq!(
            decode_event(frame).unwrap_err().code(),
            "MALFORMED_RESPONSE"
        );
    }
    let error = decode_event(SseEvent {
        event: None,
        data: json!({"type":"error","error":{"type":"overloaded_error","message":"busy"}})
            .to_string(),
    })
    .unwrap_err();
    assert_eq!(error.code(), "SERVER");
}

#[test]
fn classifies_provider_status_and_error_fields() {
    let cases = [
        (Some(401), json!({}), "AUTH"),
        (Some(403), json!({}), "AUTH"),
        (Some(402), json!({}), "QUOTA"),
        (Some(429), json!({}), "RATE_LIMIT"),
        (Some(400), json!({}), "INVALID_REQUEST"),
        (Some(413), json!({}), "INVALID_REQUEST"),
        (Some(503), json!({}), "SERVER"),
        (Some(404), json!({}), "HTTP_404"),
        (None, json!({"type":"authentication_error"}), "AUTH"),
        (None, json!({"type":"rate_limit_error"}), "RATE_LIMIT"),
        (
            None,
            json!({"type":"invalid_request_error"}),
            "INVALID_REQUEST",
        ),
        (None, json!({}), "SERVER"),
        (
            Some(400),
            json!({"message":"maximum context length exceeded"}),
            "CONTEXT_WINDOW_EXCEEDED",
        ),
        (
            Some(400),
            json!({"message":"insufficient balance"}),
            "QUOTA",
        ),
    ];
    for (status, fields, code) in cases {
        let error = provider_error(&json!({"error":fields}), status, None);
        assert_eq!(error.code(), code, "status {status:?}");
    }
}

#[test]
fn retains_request_id_and_valid_retry_after() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("2"));
    headers.insert("request-id", HeaderValue::from_static("r1"));
    let error = provider_error(&Value::Null, Some(429), Some(&headers));
    let failure = error.detail::<hoh_llm::LlmFailure>().unwrap();
    assert_eq!(
        failure
            .request_id
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("r1")
    );
    assert_eq!(failure.provider_retry_after_ms, Some(2_000.0));

    for value in ["0", "-1", "NaN", "invalid", "1e2", "+1"] {
        headers.insert("retry-after", HeaderValue::from_str(value).unwrap());
        let error = provider_error(&json!({}), Some(503), Some(&headers));
        assert_eq!(
            error
                .detail::<hoh_llm::LlmFailure>()
                .unwrap()
                .provider_retry_after_ms,
            None,
            "{value}"
        );
    }
}
