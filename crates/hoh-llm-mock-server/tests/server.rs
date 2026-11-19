use std::sync::{Arc, Mutex};
use std::time::Duration;

use hoh_llm_mock_server::{
    MockLlmBehavior as B, MockLlmRequestOutcome, MockLlmScriptBehavior, MockLlmSelectedBehavior,
    MockLlmServer, MockLlmServerEvent, MockLlmServerOptions, start_mock_llm_server,
};
use indexmap::IndexMap;
use reqwest::{Client, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn loopback_available() -> bool {
    match std::net::TcpListener::bind(("127.0.0.1", 0)) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("loopback probe failed: {error}"),
    }
}

async fn start(sequence: Vec<B>) -> MockLlmServer {
    start_mock_llm_server(MockLlmServerOptions {
        sequence,
        ..Default::default()
    })
    .await
    .expect("mock server starts")
}

async fn post(server: &MockLlmServer) -> reqwest::Response {
    Client::new()
        .post(format!("{}/v1/messages", server.base_url))
        .header("content-type", "application/json")
        .body(r#"{"model":"mock","messages":[],"stream":true}"#)
        .send()
        .await
        .expect("request succeeds")
}

#[tokio::test]
async fn success_stream_captures_request_and_events() {
    if !loopback_available() {
        return;
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::clone(&events);
    let mut server = start_mock_llm_server(MockLlmServerOptions {
        sequence: vec![B::Success],
        api_key: Some("mock-key".to_owned()),
        success_text: Some("recovered".to_owned()),
        chunk_size: Some(3.0),
        on_event: Some(Arc::new(move |event| {
            observer.lock().expect("lock").push(event)
        })),
        ..Default::default()
    })
    .await
    .expect("server starts");
    let response = Client::new()
        .post(format!("{}/v1/messages", server.base_url))
        .header("x-api-key", "mock-key")
        .body(r#"{"model":"mock","messages":[],"stream":true}"#)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .expect("header")
            .contains("text/event-stream")
    );
    let body = response.text().await.expect("body");
    for marker in [
        r#""text":"rec""#,
        r#""text":"ove""#,
        r#""text":"red""#,
        r#""stop_reason":"end_turn""#,
    ] {
        assert!(body.contains(marker), "missing {marker} in {body}");
    }
    let records = server.requests().await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].attempt, 1);
    assert_eq!(
        records[0].script_behavior,
        MockLlmScriptBehavior::Behavior(B::Success)
    );
    assert_eq!(
        records[0].behavior,
        MockLlmSelectedBehavior::Behavior(B::Success)
    );
    assert_eq!(records[0].path, "/v1/messages");
    assert_eq!(
        records[0].body,
        Some(serde_json::json!({"model":"mock","messages":[],"stream":true}))
    );
    assert_eq!(records[0].chunks_sent, 8);
    assert_eq!(records[0].outcome, Some(MockLlmRequestOutcome::Completed));
    assert_eq!(
        *events.lock().expect("lock"),
        vec![
            MockLlmServerEvent::Request {
                attempt: 1,
                script_behavior: MockLlmScriptBehavior::Behavior(B::Success),
                behavior: MockLlmSelectedBehavior::Behavior(B::Success),
                path: "/v1/messages".to_owned(),
            },
            MockLlmServerEvent::Result {
                attempt: 1,
                script_behavior: MockLlmScriptBehavior::Behavior(B::Success),
                behavior: MockLlmSelectedBehavior::Behavior(B::Success),
                outcome: MockLlmRequestOutcome::Completed,
                chunks_sent: 8,
            },
        ]
    );
    server.close().await;
}

#[tokio::test]
async fn gateway_prefix_and_panicking_observer_do_not_change_wire_behavior() {
    if !loopback_available() {
        return;
    }
    let mut server = start_mock_llm_server(MockLlmServerOptions {
        sequence: vec![B::Empty],
        on_event: Some(Arc::new(|_| panic!("observer failed"))),
        ..Default::default()
    })
    .await
    .expect("server starts");
    let response = Client::new()
        .post(format!("{}/anthropic/v1/messages", server.base_url))
        .body("{}")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 200);
    assert!(
        response
            .text()
            .await
            .expect("body")
            .contains("message_stop")
    );
    let record = server.requests().await.remove(0);
    assert_eq!(record.path, "/anthropic/v1/messages");
    assert_eq!(record.outcome, Some(MockLlmRequestOutcome::Completed));
    server.close().await;
}

#[tokio::test]
async fn incomplete_and_malformed_behaviors_match_wire_shapes() {
    if !loopback_available() {
        return;
    }
    let mut server = start_mock_llm_server(MockLlmServerOptions {
        sequence: vec![
            B::EmptyBody,
            B::StreamEof,
            B::PartialEof,
            B::MalformedJson,
            B::MalformedEvent,
        ],
        chunk_size: Some(100.0),
        ..Default::default()
    })
    .await
    .expect("server starts");
    let expected = [
        (B::EmptyBody, "", 0, true),
        (B::StreamEof, "mock-message", 1, true),
        (B::PartialEof, "discarded partial response", 3, true),
        (B::MalformedJson, "{not-json", 2, false),
        (B::MalformedEvent, "content_block\":null", 2, false),
    ];
    for (behavior, marker, chunks, lacks_terminal) in expected {
        let body = post(&server).await.text().await.expect("body");
        assert!(body.contains(marker));
        if lacks_terminal {
            assert!(!body.contains("message_stop"));
        }
        let record = server.requests().await.pop().expect("record");
        assert_eq!(record.behavior, MockLlmSelectedBehavior::Behavior(behavior));
        assert_eq!(record.chunks_sent, chunks);
        assert_eq!(record.outcome, Some(MockLlmRequestOutcome::Completed));
    }
    server.close().await;
}

#[tokio::test]
async fn reasoning_tool_max_slow_and_wrong_content_type() {
    if !loopback_available() {
        return;
    }
    let mut server = start_mock_llm_server(MockLlmServerOptions {
        sequence: vec![
            B::ReasoningSuccess,
            B::ToolCallSuccess,
            B::MaxTokens,
            B::SlowSuccess,
            B::WrongContentType,
        ],
        success_text: Some("answer".to_owned()),
        reasoning_text: Some("think".to_owned()),
        tool_name: Some("lookup".to_owned()),
        tool_arguments: Some(r#"{"id":7}"#.to_owned()),
        chunk_size: Some(2.0),
        chunk_delay_ms: Some(1.0),
        ..Default::default()
    })
    .await
    .expect("server starts");
    let one = post(&server).await.text().await.expect("body");
    let two = post(&server).await.text().await.expect("body");
    let three = post(&server).await.text().await.expect("body");
    let four = post(&server).await.text().await.expect("body");
    let five = post(&server).await;
    assert!(one.contains(r#""thinking":"th""#));
    assert!(two.contains(r#""name":"lookup""#));
    assert!(two.contains(r#""partial_json":"{\"id""#));
    assert!(two.contains(r#""stop_reason":"tool_use""#));
    assert!(three.contains(r#""stop_reason":"max_tokens""#));
    assert!(four.contains(r#""stop_reason":"end_turn""#));
    assert_eq!(five.headers()["content-type"], "application/json");
    let _ = five.text().await.expect("body");
    assert!(
        server
            .requests()
            .await
            .iter()
            .all(|record| record.outcome == Some(MockLlmRequestOutcome::Completed))
    );
    server.close().await;
}

#[tokio::test]
async fn structured_http_errors_have_provider_headers() {
    if !loopback_available() {
        return;
    }
    let behaviors = [
        B::RateLimit,
        B::ServerError,
        B::ServiceUnavailable,
        B::AuthError,
        B::InvalidRequest,
        B::ContextOverflow,
        B::QuotaExceeded,
    ];
    let mut server = start_mock_llm_server(MockLlmServerOptions {
        sequence: behaviors.to_vec(),
        retry_after_ms: Some(1_001.0),
        request_id: Some("request-1".to_owned()),
        ..Default::default()
    })
    .await
    .expect("server starts");
    let expected = [
        (B::RateLimit, 429, "mock rate limit"),
        (B::ServerError, 500, "mock server error"),
        (B::ServiceUnavailable, 503, "mock service unavailable"),
        (B::AuthError, 401, "mock authentication failed"),
        (B::InvalidRequest, 400, "mock invalid request"),
        (B::ContextOverflow, 400, "context_length_exceeded"),
        (B::QuotaExceeded, 429, "insufficient_quota"),
    ];
    for (behavior, status, marker) in expected {
        let response = post(&server).await;
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()["x-request-id"], "request-1");
        if behavior == B::RateLimit {
            assert_eq!(response.headers()["retry-after"], "2");
        } else {
            assert!(response.headers().get("retry-after").is_none());
        }
        assert!(response.text().await.expect("body").contains(marker));
        assert_eq!(
            server.requests().await.last().expect("record").outcome,
            Some(MockLlmRequestOutcome::Completed)
        );
    }
    server.close().await;
}

#[tokio::test]
async fn forced_reset_behaviors_cross_the_expected_transport_boundary() {
    if !loopback_available() {
        return;
    }
    for behavior in [
        B::ConnectionReset,
        B::StreamDisconnect,
        B::PartialDisconnect,
    ] {
        let mut server = start_mock_llm_server(MockLlmServerOptions {
            sequence: vec![behavior],
            disconnect_delay_ms: Some(1.0),
            partial_text: Some("half".to_owned()),
            chunk_size: Some(100.0),
            ..Default::default()
        })
        .await
        .expect("server starts");
        let result = Client::new()
            .post(format!("{}/v1/messages", server.base_url))
            .body("{}")
            .send()
            .await;
        match (behavior, result) {
            (B::ConnectionReset, Err(_)) => {}
            (B::StreamDisconnect | B::PartialDisconnect, Ok(response)) => {
                assert!(
                    response.text().await.is_err(),
                    "disconnect must truncate chunked body"
                );
            }
            (_, other) => panic!("unexpected transport result for {behavior:?}: {other:?}"),
        }
        let record = server.requests().await.remove(0);
        assert_eq!(record.behavior, MockLlmSelectedBehavior::Behavior(behavior));
        assert_eq!(
            record.chunks_sent,
            if behavior == B::PartialDisconnect {
                3
            } else {
                0
            }
        );
        assert_eq!(record.outcome, Some(MockLlmRequestOutcome::Reset));
        server.close().await;
    }
}

async fn open_raw_request(server: &MockLlmServer) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", server.port))
        .await
        .expect("connect");
    stream
        .write_all(b"POST /v1/messages HTTP/1.1\r\nhost: localhost\r\ncontent-length: 2\r\n\r\n{}")
        .await
        .expect("request");
    let mut received = Vec::new();
    let mut byte = [0_u8; 1];
    while !received.ends_with(b"\r\n\r\n") {
        tokio::time::timeout(Duration::from_secs(1), stream.read_exact(&mut byte))
            .await
            .expect("headers timeout")
            .expect("headers");
        received.push(byte[0]);
    }
    stream
}

async fn wait_for_outcome(server: &MockLlmServer) -> MockLlmRequestOutcome {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(outcome) = server
                .requests()
                .await
                .first()
                .and_then(|record| record.outcome)
            {
                return outcome;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("outcome timeout")
}

#[tokio::test]
async fn client_close_interrupts_stream_delays_and_emits_one_result() {
    if !loopback_available() {
        return;
    }
    for behavior in [B::SlowSuccess, B::StreamDisconnect, B::PartialDisconnect] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let observer = Arc::clone(&events);
        let mut server = start_mock_llm_server(MockLlmServerOptions {
            sequence: vec![behavior],
            chunk_delay_ms: Some(500.0),
            disconnect_delay_ms: Some(500.0),
            chunk_size: Some(1.0),
            on_event: Some(Arc::new(move |event| {
                observer.lock().expect("lock").push(event)
            })),
            ..Default::default()
        })
        .await
        .expect("server starts");
        let stream = open_raw_request(&server).await;
        drop(stream);
        assert_eq!(
            wait_for_outcome(&server).await,
            MockLlmRequestOutcome::ClientClosed
        );
        let results = events
            .lock()
            .expect("lock")
            .iter()
            .filter(|event| matches!(event, MockLlmServerEvent::Result { .. }))
            .count();
        assert_eq!(results, 1);
        server.close().await;
    }
}

#[tokio::test]
async fn server_close_finalizes_active_stream_before_aborting_task() {
    if !loopback_available() {
        return;
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::clone(&events);
    let mut server = start_mock_llm_server(MockLlmServerOptions {
        sequence: vec![B::SlowSuccess],
        chunk_delay_ms: Some(10_000.0),
        chunk_size: Some(1.0),
        on_event: Some(Arc::new(move |event| {
            observer.lock().expect("lock").push(event)
        })),
        ..Default::default()
    })
    .await
    .expect("server starts");
    let _stream = open_raw_request(&server).await;
    server.close().await;
    let record = server.requests().await.remove(0);
    assert_eq!(record.outcome, Some(MockLlmRequestOutcome::ClientClosed));
    let event_chunks = events
        .lock()
        .expect("lock")
        .iter()
        .find_map(|event| match event {
            MockLlmServerEvent::Result {
                outcome: MockLlmRequestOutcome::ClientClosed,
                chunks_sent,
                ..
            } => Some(*chunks_sent),
            _ => None,
        })
        .expect("client-closed event");
    assert_eq!(event_chunks, record.chunks_sent);
}

#[tokio::test]
async fn invalid_requests_do_not_consume_script() {
    if !loopback_available() {
        return;
    }
    let mut server = start_mock_llm_server(MockLlmServerOptions {
        sequence: vec![B::Success],
        api_key: Some("expected".to_owned()),
        ..Default::default()
    })
    .await
    .expect("server starts");
    let client = Client::new();
    let method = client
        .get(format!("{}/v1/messages", server.base_url))
        .send()
        .await
        .expect("method");
    assert_eq!(method.status(), 405);
    assert_eq!(method.headers()["allow"], "POST");
    assert_eq!(
        client
            .post(format!("{}/chat/completions", server.base_url))
            .body("{}")
            .send()
            .await
            .expect("route")
            .status(),
        404
    );
    assert_eq!(
        client
            .post(format!("{}/v1/messages", server.base_url))
            .header("x-api-key", "wrong")
            .body("{}")
            .send()
            .await
            .expect("auth")
            .status(),
        401
    );
    assert_eq!(
        client
            .post(format!("{}/v1/messages", server.base_url))
            .header("x-api-key", "expected")
            .body("{")
            .send()
            .await
            .expect("json")
            .status(),
        400
    );
    assert!(server.requests().await.is_empty());
    let response = client
        .post(format!("{}/v1/messages", server.base_url))
        .header("x-api-key", "expected")
        .send()
        .await
        .expect("empty");
    assert_eq!(response.status(), 200);
    assert_eq!(server.requests().await[0].body, None);
    server.close().await;
}

#[tokio::test]
async fn exhaustion_and_repeat_last_are_explicit() {
    if !loopback_available() {
        return;
    }
    let mut exhausted = start(vec![B::Success]).await;
    let _ = post(&exhausted).await.text().await;
    let response = post(&exhausted).await;
    assert_eq!(response.status(), 500);
    assert!(
        response
            .text()
            .await
            .expect("body")
            .contains("mock script exhausted")
    );
    assert_eq!(
        exhausted
            .requests()
            .await
            .into_iter()
            .map(|record| record.behavior)
            .collect::<Vec<_>>(),
        vec![
            MockLlmSelectedBehavior::Behavior(B::Success),
            MockLlmSelectedBehavior::ScriptExhausted
        ],
    );
    exhausted.close().await;
    let mut repeating = start_mock_llm_server(MockLlmServerOptions {
        sequence: vec![B::Empty],
        repeat_last: Some(true),
        ..Default::default()
    })
    .await
    .expect("server starts");
    let _ = post(&repeating).await.text().await;
    let _ = post(&repeating).await.text().await;
    assert_eq!(
        repeating
            .requests()
            .await
            .into_iter()
            .map(|record| record.behavior)
            .collect::<Vec<_>>(),
        vec![MockLlmSelectedBehavior::Behavior(B::Empty); 2],
    );
    repeating.close().await;
}

#[tokio::test]
async fn seeded_weighted_random_is_reproducible() {
    if !loopback_available() {
        return;
    }
    let weights: IndexMap<String, f64> = [("success".to_owned(), 1.0), ("empty".to_owned(), 1.0)]
        .into_iter()
        .collect();
    let options = || MockLlmServerOptions {
        sequence: vec![B::Random],
        repeat_last: Some(true),
        random_seed: Some(42.0),
        random_weights: Some(weights.clone()),
        ..Default::default()
    };
    let mut first = start_mock_llm_server(options()).await.expect("first");
    let mut second = start_mock_llm_server(options()).await.expect("second");
    for _ in 0..12 {
        let _ = post(&first).await.text().await;
        let _ = post(&second).await.text().await;
    }
    let a: Vec<_> = first
        .requests()
        .await
        .into_iter()
        .map(|record| record.behavior)
        .collect();
    let b: Vec<_> = second
        .requests()
        .await
        .into_iter()
        .map(|record| record.behavior)
        .collect();
    assert_eq!(a, b);
    assert!(a.windows(2).any(|pair| pair[0] != pair[1]));
    assert!(
        first
            .requests()
            .await
            .iter()
            .all(|record| { record.script_behavior == MockLlmScriptBehavior::Behavior(B::Random) })
    );
    first.close().await;
    second.close().await;
}

async fn read_chunked_response(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 2_048];
    loop {
        let count = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("response timeout")
            .expect("response read");
        assert_ne!(count, 0, "unexpected EOF");
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.ends_with(b"0\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(bytes).expect("UTF-8 response")
}

#[tokio::test]
async fn chunked_request_preserves_utf8_split_across_http_chunks() {
    if !loopback_available() {
        return;
    }
    let mut server = start(vec![B::Success]).await;
    let mut stream = TcpStream::connect(("127.0.0.1", server.port))
        .await
        .expect("connect");
    let body = r#"{"messages":[{"role":"user","content":"你好"}]}"#.as_bytes();
    let split = body
        .windows(3)
        .position(|window| window == "你".as_bytes())
        .expect("character")
        + 1;
    let head = b"POST /v1/messages HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n";
    stream.write_all(head).await.expect("head");
    for chunk in [&body[..split], &body[split..]] {
        stream
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await
            .expect("size");
        stream.write_all(chunk).await.expect("chunk");
        stream.write_all(b"\r\n").await.expect("ending");
    }
    stream.write_all(b"0\r\n\r\n").await.expect("terminal");
    let _ = read_chunked_response(&mut stream).await;
    assert_eq!(
        server.requests().await[0].body.as_ref().expect("body")["messages"][0]["content"],
        "你好"
    );
    server.close().await;
}

async fn send_raw_body(server: &MockLlmServer, body: &[u8]) {
    let mut stream = TcpStream::connect(("127.0.0.1", server.port))
        .await
        .expect("connect");
    stream
        .write_all(
            format!(
                "POST /v1/messages HTTP/1.1\r\nhost: localhost\r\ncontent-length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("head");
    stream.write_all(body).await.expect("body");
    let _ = read_chunked_response(&mut stream).await;
}

#[tokio::test]
async fn request_body_uses_node_utf8_decoding_and_javascript_number_semantics() {
    if !loopback_available() {
        return;
    }
    let mut server = start(vec![B::Success, B::Success]).await;
    send_raw_body(&server, br#"{"number":9007199254740993}"#).await;
    send_raw_body(&server, b"{\"text\":\"\xff\"}").await;
    let records = server.requests().await;
    assert_eq!(
        records[0].body,
        Some(
            hoh_util_values::js_json_parse(r#"{"number":9007199254740993}"#,)
                .expect("reference parse")
        )
    );
    assert_eq!(records[1].body.as_ref().expect("body")["text"], "�");
    server.close().await;
}

#[tokio::test]
async fn one_http11_connection_serves_multiple_requests() {
    if !loopback_available() {
        return;
    }
    let mut server = start(vec![B::Empty, B::Empty]).await;
    let mut stream = TcpStream::connect(("127.0.0.1", server.port))
        .await
        .expect("connect");
    let request = b"POST /v1/messages HTTP/1.1\r\nhost: localhost\r\ncontent-length: 2\r\n\r\n{}";
    stream.write_all(request).await.expect("first");
    assert!(read_chunked_response(&mut stream).await.contains("200 OK"));
    stream.write_all(request).await.expect("second");
    assert!(read_chunked_response(&mut stream).await.contains("200 OK"));
    assert_eq!(server.requests().await.len(), 2);
    server.close().await;
}

#[tokio::test]
async fn close_is_idempotent_and_terminates_stall() {
    if !loopback_available() {
        return;
    }
    let mut server = start(vec![B::Stall]).await;
    let mut stream = open_raw_request(&server).await;
    let mut byte = [0_u8; 1];
    assert!(
        tokio::time::timeout(Duration::from_millis(25), stream.read(&mut byte))
            .await
            .is_err(),
        "stall must leave the chunked response body open without a terminal chunk",
    );
    drop(stream);
    assert_eq!(
        wait_for_outcome(&server).await,
        MockLlmRequestOutcome::Stalled
    );
    server.close().await;
    server.close().await;
    assert_eq!(
        server.requests().await[0].outcome,
        Some(MockLlmRequestOutcome::Stalled)
    );
}

#[tokio::test]
async fn ipv6_listener_is_advertised_with_brackets() {
    let probe = match std::net::TcpListener::bind(("::1", 0)) {
        Ok(listener) => listener,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::AddrNotAvailable
            ) =>
        {
            return;
        }
        Err(error) => panic!("IPv6 probe failed: {error}"),
    };
    drop(probe);
    let mut server = start_mock_llm_server(MockLlmServerOptions {
        host: Some("::1".to_owned()),
        sequence: vec![B::Success],
        ..Default::default()
    })
    .await
    .expect("IPv6 server starts");
    assert!(server.base_url.starts_with("http://[::1]:"));
    assert_eq!(post(&server).await.status(), 200);
    server.close().await;
}

#[tokio::test]
async fn option_validation_rejects_invalid_values() {
    let weights = |name: &str, value: f64| Some([(name.to_owned(), value)].into_iter().collect());
    let invalid = vec![
        (MockLlmServerOptions::default(), "sequence"),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                host: Some(String::new()),
                ..Default::default()
            },
            "host",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                port: Some(-1.0),
                ..Default::default()
            },
            "port",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                port: Some(65_536.0),
                ..Default::default()
            },
            "port",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                api_key: Some(String::new()),
                ..Default::default()
            },
            "apiKey",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                success_text: Some(String::new()),
                ..Default::default()
            },
            "successText",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                partial_text: Some(String::new()),
                ..Default::default()
            },
            "partialText",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                reasoning_text: Some(String::new()),
                ..Default::default()
            },
            "reasoningText",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                chunk_size: Some(0.0),
                ..Default::default()
            },
            "chunkSize",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                chunk_delay_ms: Some(-1.0),
                ..Default::default()
            },
            "chunkDelayMs",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                disconnect_delay_ms: Some(f64::INFINITY),
                ..Default::default()
            },
            "disconnectDelayMs",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                retry_after_ms: Some(0.0),
                ..Default::default()
            },
            "retryAfterMs",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                request_id: Some(String::new()),
                ..Default::default()
            },
            "requestId",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                tool_name: Some(String::new()),
                ..Default::default()
            },
            "toolName",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Success],
                tool_arguments: Some("{".to_owned()),
                ..Default::default()
            },
            "toolArguments",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Random],
                random_seed: Some(-1.0),
                ..Default::default()
            },
            "randomSeed",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Random],
                random_weights: weights("random", 1.0),
                ..Default::default()
            },
            "unknown concrete behavior",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Random],
                random_weights: weights("success", -1.0),
                ..Default::default()
            },
            "non-negative",
        ),
        (
            MockLlmServerOptions {
                sequence: vec![B::Random],
                random_weights: weights("success", 0.0),
                ..Default::default()
            },
            "positive weight",
        ),
    ];
    for (options, marker) in invalid {
        let error = match start_mock_llm_server(options).await {
            Ok(mut server) => {
                server.close().await;
                panic!("invalid options were accepted");
            }
            Err(error) => error,
        };
        assert!(
            error.to_string().contains(marker),
            "expected {marker:?}, got {error}"
        );
    }
}

#[test]
fn event_serialization_uses_typescript_field_names() {
    let event = MockLlmServerEvent::Result {
        attempt: 2,
        script_behavior: hoh_llm_mock_server::MockLlmScriptBehavior::Behavior(B::Random),
        behavior: hoh_llm_mock_server::MockLlmSelectedBehavior::Behavior(B::Success),
        outcome: MockLlmRequestOutcome::Completed,
        chunks_sent: 7,
    };
    assert_eq!(
        serde_json::to_string(&event).expect("serialize"),
        r#"{"type":"result","attempt":2,"scriptBehavior":"random","behavior":"success","outcome":"completed","chunksSent":7}"#
    );
}
