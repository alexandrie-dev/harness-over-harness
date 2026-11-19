//! Review round: API key hygiene, tool-call names and arguments, the output-limit field,
//! and what the probe accepts as an answer. Server replies are written from the documented
//! formats (OpenAI error shape, OpenAI-style streaming chunks), not recorded from live servers.
mod support;
use hoh_llm::{LlmAdapter, LlmFailure};
use hoh_llm_openai::{OpenAiAdapter, OpenAiConfig, ProbeOptions, ServerKind, probe};
use serde_json::json;
use support::*;

const KEY: &str = "sk-review-secret-1234";
const MODELS: &str = r#"{"object":"list","data":[{"id":"qwen"}]}"#;
const COMPLETION: &str = r#"{"id":"c","choices":[{"index":0,"message":{"role":"assistant","content":"p"},"finish_reason":"length"}]}"#;

fn quick() -> ProbeOptions {
    ProbeOptions {
        connect_timeout_ms: 500,
        models_timeout_ms: 500,
        completion_timeout_ms: 2_000,
    }
}

fn hello() -> Reply {
    Reply::sse_text(&format!(
        "{}{}{}",
        delta(json!({"content": "hi"})),
        finish("stop"),
        done()
    ))
}

fn unsupported_max_tokens() -> Reply {
    // OpenAI, models that only accept `max_completion_tokens`.
    Reply::json(
        400,
        r#"{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.","type":"invalid_request_error","param":"max_tokens","code":"unsupported_parameter"}}"#,
    )
}

fn weather_opts() -> hoh_llm::GenerateOptions {
    let mut opts = options(vec![user("weather?")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    opts
}

fn limited_opts() -> hoh_llm::GenerateOptions {
    let mut opts = options(vec![user("hi")]);
    opts.max_tokens = Some(64);
    opts
}

// ---- 1. the key does not reach Debug output ----

#[test]
fn config_and_adapter_debug_hide_the_key() {
    let mut config = OpenAiConfig::new("http://127.0.0.1:1/v1");
    config.api_key = Some(KEY.into());
    let text = format!("{config:?}");
    assert!(!text.contains(KEY) && text.contains("[redacted]"), "{text}");
    assert!(text.contains("http://127.0.0.1:1/v1"), "{text}");
    let adapter = OpenAiAdapter::new(config).unwrap();
    let text = format!("{adapter:?} {:?}", adapter.config());
    assert!(!text.contains(KEY), "{text}");
    // No key: nothing to hide, and the field says so.
    assert!(format!("{:?}", OpenAiConfig::new("http://h/v1")).contains("api_key: None"));
}

// ---- 2. the key is scrubbed from every diagnostic that leaves the adapter ----

fn failure_text(error: &hoh_error::HarnessError) -> String {
    format!(
        "{} | {error:?} | {:?}",
        error.message(),
        error.detail::<LlmFailure>()
    )
}

#[tokio::test]
async fn key_is_removed_from_an_http_error() {
    let body = format!(
        r#"{{"error":{{"message":"Invalid API key: {KEY}","type":"authentication_error","code":401}}}}"#
    );
    let server = serve(vec![(CHAT, vec![Reply::json(401, &body)])]).await;
    let run = run(
        &adapter(&server, |c| c.api_key = Some(KEY.into())),
        options(vec![user("hi")]),
    )
    .await;
    assert_eq!(run.code(), "AUTH");
    let text = failure_text(run.error.as_ref().unwrap());
    assert!(!text.contains(KEY), "{text}");
    assert!(text.contains("Invalid API key: [redacted]"), "{text}");
    // Proof that the server really received the key (the test is not vacuous).
    assert_eq!(
        server.captured()[0].headers["authorization"],
        format!("Bearer {KEY}")
    );
}

#[tokio::test]
async fn key_is_removed_from_an_error_inside_the_stream() {
    let frame = format!(
        r#"data: {{"error":{{"message":"Incorrect API key provided: {KEY}.","type":"invalid_request_error"}}}}

"#
    );
    let server = serve(vec![(
        CHAT,
        vec![Reply::sse_text(&format!(
            "{}{frame}",
            delta(json!({"content": "he"}))
        ))],
    )])
    .await;
    let first = run(
        &adapter(&server, |c| c.api_key = Some(KEY.into())),
        options(vec![user("hi")]),
    )
    .await;
    let text = failure_text(first.error.as_ref().expect("an error"));
    assert!(!text.contains(KEY) && text.contains("[redacted]"), "{text}");
    // A text body that is not JSON is scrubbed too.
    let server = serve(vec![(
        CHAT,
        vec![Reply::sse_text(&format!("data: not json {KEY}\n\n"))],
    )])
    .await;
    let second = run(
        &adapter(&server, |c| c.api_key = Some(KEY.into())),
        options(vec![user("hi")]),
    )
    .await;
    let text = failure_text(second.error.as_ref().expect("an error"));
    assert!(!text.contains(KEY), "{text}");
}

#[tokio::test]
async fn key_is_removed_from_the_model_list_error() {
    let body = format!(r#"{{"error":{{"message":"bad key {KEY}"}}}}"#);
    let server = serve(vec![("GET /v1/models", vec![Reply::json(401, &body)])]).await;
    let adapter = adapter(&server, |c| c.api_key = Some(KEY.into()));
    let error = adapter.list_models("local").await.unwrap_err();
    assert!(!error.to_string().contains(KEY), "{error}");
    assert!(!format!("{error:?}").contains(KEY), "{error:?}");
}

#[tokio::test]
async fn key_is_removed_from_the_probe_result() {
    let rejected = format!(
        r#"{{"error":{{"message":"Invalid API key: {KEY}","type":"authentication_error","code":401}}}}"#
    );
    let server = serve(vec![
        ("GET /v1/models", vec![Reply::json(401, &rejected)]),
        (CHAT, vec![Reply::json(401, &rejected)]),
    ])
    .await;
    let result = probe(&server.base_url, "qwen", Some(KEY), &quick(), None).await;
    assert!(!result.ok);
    let all = format!("{result:?} {}", serde_json::to_string(&result).unwrap());
    assert!(!all.contains(KEY) && all.contains("[redacted]"), "{all}");
    // A key echoed by a 200 body that is an error object, and by a proxy page.
    let echoed = format!(r#"{{"error":{{"message":"upstream said {KEY}"}}}}"#);
    for body in [echoed, format!("<html>denied for {KEY}</html>")] {
        let server = serve(vec![
            ("GET /v1/models", vec![Reply::json(200, MODELS)]),
            (CHAT, vec![Reply::json(200, &body)]),
        ])
        .await;
        let result = probe(&server.base_url, "qwen", Some(KEY), &quick(), None).await;
        assert!(!result.ok);
        let all = format!("{result:?} {}", serde_json::to_string(&result).unwrap());
        assert!(!all.contains(KEY), "{all}");
    }
}

// ---- 3. the tool name is the concatenation of its fragments ----

fn call_frames(deltas: Vec<serde_json::Value>) -> Reply {
    let mut text = String::new();
    for call in deltas {
        text.push_str(&delta(json!({"tool_calls": [call]})));
    }
    text.push_str(&finish("tool_calls"));
    text.push_str(done());
    Reply::sse_text(&text)
}

#[tokio::test]
async fn tool_name_is_assembled_from_fragments() {
    let server = serve(vec![(
        CHAT,
        vec![call_frames(vec![
            json!({"index": 0, "id": "call_1", "function": {"name": "wea", "arguments": ""}}),
            json!({"index": 0, "function": {"name": "ther", "arguments": "{\"city\":"}}),
            json!({"index": 0, "function": {"arguments": "\"Paris\"}"}}),
        ])],
    )])
    .await;
    let run = run(&adapter(&server, |_| {}), weather_opts()).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(
        run.blocks(),
        vec![(
            "tool-call".to_owned(),
            "weather({\"city\":\"Paris\"})".to_owned()
        )]
    );
}

#[tokio::test]
async fn a_name_repeated_in_full_on_every_delta_is_not_doubled() {
    let server = serve(vec![(
        CHAT,
        vec![call_frames(vec![
            json!({"index": 0, "id": "call_1", "function": {"name": "weather", "arguments": "{\"city\":"}}),
            json!({"index": 0, "function": {"name": "weather", "arguments": "\"Paris\"}"}}),
        ])],
    )])
    .await;
    let run = run(&adapter(&server, |_| {}), weather_opts()).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(
        run.blocks(),
        vec![(
            "tool-call".to_owned(),
            "weather({\"city\":\"Paris\"})".to_owned()
        )]
    );
}

// ---- 4. arguments that are not a string of JSON ----

#[tokio::test]
async fn arguments_of_a_wrong_json_type_are_malformed() {
    for (arguments, kind) in [
        (json!([1]), "array"),
        (json!(7), "number"),
        (json!(true), "boolean"),
    ] {
        let server = serve(vec![(
            CHAT,
            vec![call_frames(vec![
                json!({"index": 0, "id": "call_1", "function": {"name": "ping", "arguments": arguments}}),
            ])],
        )])
        .await;
        let mut opts = options(vec![user("x")]);
        opts.tools = Some(vec![tool("ping", &[])]);
        let run = run(&adapter(&server, |_| {}), opts).await;
        assert_eq!(run.code(), "MALFORMED_RESPONSE", "{arguments}");
        let message = run.error.unwrap().message().to_owned();
        assert!(
            message.contains(&format!("JSON {kind}")) && message.contains("ping"),
            "{message}"
        );
    }
}

#[tokio::test]
async fn an_arguments_object_is_still_accepted() {
    let server = serve(vec![(
        CHAT,
        vec![call_frames(vec![
            json!({"index": 0, "id": "call_1", "function": {"name": "weather", "arguments": {"city": "Paris"}}}),
        ])],
    )])
    .await;
    let run = run(&adapter(&server, |_| {}), weather_opts()).await;
    assert!(run.error.is_none(), "{:?}", run.error);
}

// ---- 5. max_tokens / max_completion_tokens ----

#[tokio::test]
async fn openai_servers_get_max_completion_tokens() {
    let server = serve(vec![(CHAT, vec![hello()])]).await;
    let adapter = adapter(&server, |c| c.server = ServerKind::OpenAi);
    let run = run(&adapter, limited_opts()).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    let body = server.last_body();
    assert_eq!(body["max_completion_tokens"], 64);
    assert!(body.get("max_tokens").is_none());
}

#[tokio::test]
async fn a_refused_max_tokens_is_retried_and_remembered() {
    let server = serve(vec![(
        CHAT,
        vec![unsupported_max_tokens(), hello(), hello()],
    )])
    .await;
    let adapter = adapter(&server, |_| {});
    let first = run(&adapter, limited_opts()).await;
    assert!(first.error.is_none(), "{:?}", first.error);
    assert_eq!(first.blocks(), vec![("text".to_owned(), "hi".to_owned())]);
    let bodies: Vec<_> = server.captured().iter().map(|r| r.json()).collect();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["max_tokens"], 64);
    assert!(bodies[0].get("max_completion_tokens").is_none());
    assert_eq!(bodies[1]["max_completion_tokens"], 64);
    assert!(bodies[1].get("max_tokens").is_none());
    // The next request does not repeat the refusal.
    let second = run(&adapter, limited_opts()).await;
    assert!(second.error.is_none(), "{:?}", second.error);
    let bodies: Vec<_> = server.captured().iter().map(|r| r.json()).collect();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[2]["max_completion_tokens"], 64);
    assert!(bodies[2].get("max_tokens").is_none());
}

#[tokio::test]
async fn a_value_that_is_too_large_is_not_retried_with_another_field() {
    let server = serve(vec![(
        CHAT,
        vec![Reply::json(
            400,
            r#"{"error":{"message":"max_tokens is too large: 100000. This model supports at most 4096 completion tokens.","type":"invalid_request_error"}}"#,
        )],
    )])
    .await;
    let run = run(&adapter(&server, |_| {}), limited_opts()).await;
    assert_eq!(run.code(), "INVALID_REQUEST");
    assert_eq!(server.captured().len(), 1);
}

#[tokio::test]
async fn the_probe_retries_with_max_completion_tokens() {
    let server = serve(vec![
        ("GET /v1/models", vec![Reply::json(200, MODELS)]),
        (
            CHAT,
            vec![unsupported_max_tokens(), Reply::json(200, COMPLETION)],
        ),
    ])
    .await;
    let result = probe(&server.base_url, "qwen", None, &quick(), None).await;
    assert!(result.ok && result.tool_calls, "{result:?}");
    let bodies: Vec<_> = server.captured()[1..].iter().map(|r| r.json()).collect();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["max_tokens"], 1);
    assert_eq!(bodies[1]["max_completion_tokens"], 1);
    assert!(bodies[1].get("max_tokens").is_none() && bodies[1]["tools"].is_array());
}

// ---- 6. the probe checks the answer, not only the status ----

#[tokio::test]
async fn a_200_that_is_not_a_completion_fails_the_probe() {
    let bodies = [
        (
            "<html><body>Welcome to nginx</body></html>",
            "MALFORMED_RESPONSE",
        ),
        ("{}", "MALFORMED_RESPONSE"),
        (r#"{"choices":[]}"#, "MALFORMED_RESPONSE"),
        (
            r#"{"error":{"message":"model is loading","type":"server_error","code":503}}"#,
            "SERVER",
        ),
    ];
    for (body, code) in bodies {
        let server = serve(vec![
            ("GET /v1/models", vec![Reply::json(200, MODELS)]),
            (CHAT, vec![Reply::json(200, body)]),
        ])
        .await;
        let result = probe(&server.base_url, "qwen", None, &quick(), None).await;
        assert!(!result.ok && !result.tool_calls, "{body}: {result:?}");
        let error = result.error.expect("a diagnosis");
        assert_eq!(error.code, code, "{body}");
        assert!(!error.message.is_empty(), "{body}");
        assert_eq!(result.latency_ms, 0, "{body}");
    }
}

#[tokio::test]
async fn an_error_object_with_status_200_is_diagnosed_with_its_text() {
    let server = serve(vec![
        ("GET /v1/models", vec![Reply::json(200, MODELS)]),
        (
            CHAT,
            vec![Reply::json(
                200,
                r#"{"error":{"message":"model qwen not found","type":"not_found","code":404}}"#,
            )],
        ),
    ])
    .await;
    let result = probe(&server.base_url, "qwen", None, &quick(), None).await;
    assert!(!result.ok);
    assert!(
        result
            .error
            .unwrap()
            .message
            .contains("model qwen not found")
    );
}
