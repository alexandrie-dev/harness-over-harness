mod support;
use hoh_llm_openai::{ProbeOptions, probe};
use support::*;

const MODELS: &str = r#"{"object":"list","data":[{"id":"qwen"}]}"#;
const COMPLETION: &str = r#"{"id":"c","choices":[{"index":0,"message":{"role":"assistant","content":"p"},"finish_reason":"length"}]}"#;

fn quick() -> ProbeOptions {
    ProbeOptions {
        connect_timeout_ms: 500,
        models_timeout_ms: 500,
        completion_timeout_ms: 2_000,
    }
}

#[tokio::test]
async fn healthy_server_with_tools() {
    let server = serve(vec![
        ("GET /v1/models", vec![Reply::json(200, MODELS)]),
        (CHAT, vec![Reply::json(200, COMPLETION)]),
    ])
    .await;
    let result = probe(&server.base_url, "qwen", Some("k"), &quick(), None).await;
    assert!(result.ok, "{result:?}");
    assert!(result.tool_calls);
    assert_eq!(result.models, ["qwen"]);
    assert_eq!(result.model_listed, Some(true));
    let requests = server.captured();
    assert_eq!(requests[0].headers["authorization"], "Bearer k");
    let body = requests[1].json();
    assert_eq!(
        (body["max_tokens"].as_u64(), body["stream"].as_bool()),
        (Some(1), Some(false))
    );
    assert!(body["tools"].is_array());
    let json = serde_json::to_value(&result).unwrap();
    assert!(json.get("latencyMs").is_some() && json["toolCalls"] == true);
}

#[tokio::test]
async fn server_that_rejects_tools_is_ok_without_tool_calls() {
    let server = serve(vec![
        ("GET /v1/models", vec![Reply::json(200, MODELS)]),
        (CHAT, vec![
            Reply::json(400, r#"{"object":"error","message":"\"auto\" tool choice requires --enable-auto-tool-choice","type":"BadRequestError","code":400}"#),
            Reply::json(200, COMPLETION),
        ]),
    ])
    .await;
    let result = probe(&server.base_url, "qwen", None, &quick(), None).await;
    assert!(result.ok && !result.tool_calls, "{result:?}");
    assert_eq!(server.captured().len(), 3);
    assert!(server.captured()[2].json().get("tools").is_none());
}

#[tokio::test]
async fn model_not_in_the_list_is_reported_not_fatal() {
    let server = serve(vec![
        ("GET /v1/models", vec![Reply::json(200, MODELS)]),
        (CHAT, vec![Reply::json(200, COMPLETION)]),
    ])
    .await;
    let result = probe(&server.base_url, "other", None, &quick(), None).await;
    assert!(result.ok);
    assert_eq!(result.model_listed, Some(false));
}

#[tokio::test]
async fn authentication_failure_is_reported_from_the_completion() {
    let body =
        r#"{"error":{"message":"Invalid API Key","type":"authentication_error","code":401}}"#;
    let server = serve(vec![
        ("GET /v1/models", vec![Reply::json(401, body)]),
        (CHAT, vec![Reply::json(401, body)]),
    ])
    .await;
    let result = probe(&server.base_url, "qwen", None, &quick(), None).await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "AUTH");
    assert_eq!(result.model_listed, None);
}

#[tokio::test]
async fn unreachable_server_stops_after_the_list() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    drop(listener);
    let result = probe(&base, "qwen", None, &quick(), None).await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "TRANSPORT");
}

#[tokio::test]
async fn silent_server_times_out() {
    let server = serve(vec![
        ("GET /v1/models", vec![Reply::json(200, MODELS)]),
        (CHAT, vec![Reply::sse_text("").stalled()]),
    ])
    .await;
    let options = ProbeOptions {
        completion_timeout_ms: 200,
        ..quick()
    };
    let result = probe(&server.base_url, "qwen", None, &options, None).await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "TIMEOUT");
}

#[tokio::test]
async fn bad_base_url_is_reported() {
    let result = probe("nope", "qwen", None, &quick(), None).await;
    assert_eq!(result.error.unwrap().code, "INVALID_CONFIG");
}
