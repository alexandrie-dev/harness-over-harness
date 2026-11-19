mod support;
use hoh_llm::LlmFailure;
use hoh_llm_openai::{OpenAiAdapter, OpenAiConfig};
use hoh_timeout::AbortController;
use serde_json::json;
use support::*;

fn weather_opts() -> hoh_llm::GenerateOptions {
    let mut opts = options(vec![user("weather?")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    opts
}

fn stream_with_call(name: &str, arguments: &str) -> Reply {
    Reply::sse_text(&format!(
        "{}{}{}",
        delta(
            json!({"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": name, "arguments": arguments}}]})
        ),
        finish("tool_calls"),
        done()
    ))
}

// ---- strict tool calling ----

#[tokio::test]
async fn invalid_json_arguments_fail_with_a_clear_message() {
    let server = serve(vec![(
        CHAT,
        vec![stream_with_call("weather", "{\"city\": \"Par")],
    )])
    .await;
    let run = run(&adapter(&server, |_| {}), weather_opts()).await;
    assert_eq!(run.code(), "MALFORMED_RESPONSE");
    let message = run.error.unwrap().message().to_owned();
    assert!(
        message.contains("\"weather\"") && message.contains("call_1"),
        "{message}"
    );
    assert!(message.contains("not valid JSON"), "{message}");
    assert!(
        message.contains("Arguments received: {\"city\": \"Par"),
        "{message}"
    );
}

#[tokio::test]
async fn non_object_arguments_unknown_tool_and_missing_required_are_rejected() {
    for (name, arguments, expect) in [
        ("weather", "[1]", "not a JSON object"),
        ("weather", "\"text\"", "not a JSON object"),
        ("nope", "{}", "not a declared tool"),
        ("weather", "{}", "required parameter(s): city"),
        ("", "{}", "no tool name"),
    ] {
        let server = serve(vec![(CHAT, vec![stream_with_call(name, arguments)])]).await;
        let run = run(&adapter(&server, |_| {}), weather_opts()).await;
        assert_eq!(run.code(), "MALFORMED_RESPONSE", "{name} {arguments}");
        assert!(
            run.error.unwrap().message().contains(expect),
            "{name} {arguments}"
        );
    }
}

#[tokio::test]
async fn empty_arguments_mean_an_empty_object() {
    let server = serve(vec![(CHAT, vec![stream_with_call("ping", "")])]).await;
    let mut opts = options(vec![user("x")]);
    opts.tools = Some(vec![tool("ping", &[])]);
    let run = run(&adapter(&server, |_| {}), opts).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(
        run.blocks(),
        vec![("tool-call".to_owned(), "ping({})".to_owned())]
    );
}

#[tokio::test]
async fn lenient_mode_passes_raw_arguments_through() {
    let server = serve(vec![(
        CHAT,
        vec![stream_with_call("nope", "{\"city\": \"Par")],
    )])
    .await;
    let run = run(
        &adapter(&server, |c| c.strict_tool_arguments = false),
        weather_opts(),
    )
    .await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(
        run.blocks(),
        vec![("tool-call".to_owned(), "nope({\"city\": \"Par)".to_owned())]
    );
}

// ---- HTTP errors ----

async fn http_failure(status: u16, body: &str, headers: Vec<(&'static str, &str)>) -> LlmFailure {
    let mut reply = Reply::json(status, body);
    for (k, v) in headers {
        reply = reply.header(k, v);
    }
    let server = serve(vec![(CHAT, vec![reply])]).await;
    let run = run_simple(&server, |_| {}).await;
    run.error
        .expect("an error")
        .detail::<LlmFailure>()
        .expect("failure detail")
        .clone()
}

#[tokio::test]
async fn http_status_classification() {
    // vLLM shape.
    let f = http_failure(400, r#"{"object":"error","message":"bad field","type":"BadRequestError","param":null,"code":400}"#, vec![]).await;
    assert_eq!(
        (f.code.as_str(), f.status, f.message.as_str()),
        ("INVALID_REQUEST", Some(400), "bad field")
    );
    // llama.cpp / OpenAI shape.
    let f = http_failure(
        401,
        r#"{"error":{"code":401,"message":"Invalid API Key","type":"authentication_error"}}"#,
        vec![],
    )
    .await;
    assert_eq!(f.code, "AUTH");
    let f = http_failure(
        429,
        r#"{"error":{"message":"slow down"}}"#,
        vec![("retry-after", "3"), ("x-request-id", "r-1")],
    )
    .await;
    assert_eq!(f.code, "RATE_LIMIT");
    assert_eq!(f.provider_retry_after_ms, Some(3000.0));
    assert_eq!(f.request_id.as_ref().map(|r| r.as_str()), Some("r-1"));
    // Ollama native string error.
    let f = http_failure(
        404,
        r#"{"error":"model \"qwen\" not found, try pulling it first"}"#,
        vec![],
    )
    .await;
    assert_eq!(f.code, "UNKNOWN_MODEL");
    // Model still loading.
    let f = http_failure(
        503,
        r#"{"error":{"code":503,"message":"Loading model","type":"unavailable_error"}}"#,
        vec![],
    )
    .await;
    assert_eq!(f.code, "SERVER");
    // Not JSON at all (a proxy page).
    let f = http_failure(502, "<html>Bad gateway</html>", vec![]).await;
    assert_eq!(
        (f.code.as_str(), f.message.as_str()),
        ("SERVER", "<html>Bad gateway</html>")
    );
}

#[tokio::test]
async fn context_window_overflow_wording_of_each_server() {
    // vLLM
    let f = http_failure(400, r#"{"object":"error","message":"This model's maximum context length is 32768 tokens. However, you requested 40000 tokens in the messages.","type":"BadRequestError","code":400}"#, vec![]).await;
    assert_eq!(f.code, "CONTEXT_WINDOW_EXCEEDED");
    // llama.cpp
    let f = http_failure(400, r#"{"error":{"code":400,"message":"the request exceeds the available context size, try increasing it","type":"exceed_context_size_error","n_prompt_tokens":9000,"n_ctx":4096}}"#, vec![]).await;
    assert_eq!(f.code, "CONTEXT_WINDOW_EXCEEDED");
}

// ---- in-band errors and early close ----

#[tokio::test]
async fn error_object_inside_the_stream() {
    let text = format!(
        "{}data: {}\n\n",
        delta(json!({"content": "partial"})),
        json!({"error": {"code": 500, "message": "decode failed", "type": "server_error"}})
    );
    let server = serve(vec![(CHAT, vec![Reply::sse_text(&text)])]).await;
    let run = run_simple(&server, |_| {}).await;
    assert_eq!(run.code(), "SERVER");
    assert_eq!(run.error.unwrap().message(), "decode failed");
}

#[tokio::test]
async fn named_error_event() {
    let text =
        "event: error\ndata: {\"error\":{\"message\":\"too many requests\",\"code\":429}}\n\n";
    let server = serve(vec![(CHAT, vec![Reply::sse_text(text)])]).await;
    assert_eq!(run_simple(&server, |_| {}).await.code(), "RATE_LIMIT");
}

#[tokio::test]
async fn server_closing_early_is_stream_closed() {
    let text = delta(json!({"content": "partial"}));
    let server = serve(vec![(CHAT, vec![Reply::sse_text(&text)])]).await;
    let run = run_simple(&server, |_| {}).await;
    assert_eq!(run.code(), "STREAM_CLOSED");
}

#[tokio::test]
async fn connection_cut_inside_a_frame_is_stream_closed() {
    let whole = delta(json!({"content": "partial"}));
    let cut = whole[..whole.len() - 5].to_owned();
    let text = format!("{}{}", delta(json!({"content": "ok"})), cut);
    let server = serve(vec![(CHAT, vec![Reply::sse_text(&text)])]).await;
    assert_eq!(run_simple(&server, |_| {}).await.code(), "STREAM_CLOSED");
}

#[tokio::test]
async fn invalid_json_in_a_frame_is_malformed() {
    let server = serve(vec![(CHAT, vec![Reply::sse_text("data: {not json}\n\n")])]).await;
    assert_eq!(
        run_simple(&server, |_| {}).await.code(),
        "MALFORMED_RESPONSE"
    );
}

#[tokio::test]
async fn empty_response_is_reported() {
    let text = format!("{}{}", finish("stop"), done());
    let server = serve(vec![(CHAT, vec![Reply::sse_text(&text)])]).await;
    assert_eq!(run_simple(&server, |_| {}).await.code(), "EMPTY_RESPONSE");
}

#[tokio::test]
async fn content_filter_and_unknown_finish_reasons_fail() {
    for (reason, code) in [
        ("content_filter", "INVALID_RESPONSE"),
        ("abort", "STREAM_CLOSED"),
        ("weird", "MALFORMED_RESPONSE"),
    ] {
        let text = format!(
            "{}{}{}",
            delta(json!({"content": "x"})),
            finish(reason),
            done()
        );
        let server = serve(vec![(CHAT, vec![Reply::sse_text(&text)])]).await;
        assert_eq!(run_simple(&server, |_| {}).await.code(), code, "{reason}");
    }
}

#[tokio::test]
async fn silence_longer_than_the_idle_limit_times_out() {
    let text = delta(json!({"content": "x"}));
    let server = serve(vec![(CHAT, vec![Reply::sse_text(&text).stalled()])]).await;
    let run = run_simple(&server, |c| c.stream_idle_timeout_ms = 200).await;
    assert_eq!(run.code(), "TIMEOUT");
}

#[tokio::test]
async fn abort_signal_stops_a_stalled_stream() {
    let text = delta(json!({"content": "x"}));
    let server = serve(vec![(CHAT, vec![Reply::sse_text(&text).stalled()])]).await;
    let controller = AbortController::new();
    let mut opts = options(vec![user("hi")]);
    opts.signal = Some(controller.signal());
    let adapter = adapter(&server, |_| {});
    let handle = tokio::spawn({
        let controller = controller;
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            controller.abort();
        }
    });
    let run = run(&adapter, opts).await;
    handle.await.unwrap();
    assert_eq!(run.code(), "ABORTED");
}

#[tokio::test]
async fn connection_refused_is_a_transport_error() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    drop(listener);
    let adapter = OpenAiAdapter::new(OpenAiConfig::new(base)).unwrap();
    let run = run(&adapter, options(vec![user("hi")])).await;
    assert_eq!(run.code(), "TRANSPORT");
}

#[test]
fn bad_configuration_is_rejected_at_construction() {
    for base in ["", "not a url", "ftp://host/v1", "file:///x"] {
        let error = OpenAiAdapter::new(OpenAiConfig::new(base)).expect_err(base);
        assert_eq!(error.code(), "INVALID_CONFIG", "{base}");
    }
    let mut config = OpenAiConfig::new("http://127.0.0.1:1/v1");
    config.tool_choice = hoh_llm_openai::ToolChoice::Required;
    config.server = hoh_llm_openai::ServerKind::Ollama;
    assert_eq!(
        OpenAiAdapter::new(config).err().unwrap().code(),
        "INVALID_CONFIG"
    );
    let mut config = OpenAiConfig::new("http://127.0.0.1:1/v1");
    config.api_key = Some("bad key with spaces".into());
    assert_eq!(
        OpenAiAdapter::new(config).err().unwrap().code(),
        "INVALID_CREDENTIAL"
    );
}
