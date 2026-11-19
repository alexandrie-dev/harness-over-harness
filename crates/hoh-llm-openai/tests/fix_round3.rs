//! Third review round: the key is matched ignoring ASCII case and across split causes, a resolved
//! model name is cleaned, the model's own answer is left alone, and the retry rule for the
//! output-limit field is exact about names and wording. Replies follow the documented formats.
mod support;
use hoh_error::HarnessError;
use hoh_llm::{BuiltinLlmFailureCode, LlmAdapter, LlmError, LlmErrorOptions};
use hoh_llm_openai::error::{REDACTED, error_fields, redact, redact_text, rejects_limit_field};
use hoh_llm_openai::{ProbeOptions, probe};
use serde_json::json;
use support::*;

const KEY: &str = "sk-review-secret-1234";
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

fn limited_opts() -> hoh_llm::GenerateOptions {
    let mut opts = options(vec![user("hi")]);
    opts.max_tokens = Some(64);
    opts
}

// ---- case-insensitive key ----

#[test]
fn a_long_key_is_matched_ignoring_ascii_case() {
    let upper = KEY.to_ascii_uppercase();
    let mixed = "Sk-Review-Secret-1234";
    assert_eq!(
        redact_text(&format!("a {upper} b {mixed} c {KEY} d"), Some(KEY)),
        format!("a {REDACTED} b {REDACTED} c {REDACTED} d")
    );
    // Offsets stay right around non-ASCII text, and the key itself may hold non-ASCII letters.
    assert_eq!(
        redact_text(&format!("Ключ «{upper}» неверный"), Some(KEY)),
        format!("Ключ «{REDACTED}» неверный")
    );
    let unicode_key = "ключ-секрет-ЯЯЯЯ";
    assert_eq!(
        redact_text("ошибка: ключ-секрет-ЯЯЯЯ!", Some(unicode_key)),
        format!("ошибка: {REDACTED}!")
    );
    // A short key stays case-sensitive and whole-token: "A" is not the key "a".
    assert_eq!(redact_text("A a", Some("a")), format!("A {REDACTED}"));
}

#[tokio::test]
async fn an_echo_in_another_case_is_removed_from_an_http_error() {
    let body = format!(
        r#"{{"error":{{"message":"Invalid API key: {}","type":"authentication_error","code":401}}}}"#,
        KEY.to_ascii_uppercase()
    );
    let server = serve(vec![(CHAT, vec![Reply::json(401, &body)])]).await;
    let run = run(
        &adapter(&server, |c| c.api_key = Some(KEY.into())),
        options(vec![user("hi")]),
    )
    .await;
    assert_eq!(
        run.error.unwrap().message(),
        format!("Invalid API key: {REDACTED}")
    );
}

fn models_in_other_case() -> String {
    format!(
        r#"{{"object":"list","data":[{{"id":"qwen"}},{{"id":"{}"}},{{"id":"m-{}"}}]}}"#,
        KEY.to_ascii_uppercase(),
        KEY.to_ascii_lowercase()
    )
}

#[tokio::test]
async fn model_ids_with_the_key_in_another_case_are_dropped() {
    let server = serve(vec![
        (
            "GET /v1/models",
            vec![Reply::json(200, &models_in_other_case())],
        ),
        (CHAT, vec![Reply::json(200, COMPLETION)]),
    ])
    .await;
    let adapter = adapter(&server, |c| c.api_key = Some(KEY.into()));
    let ids: Vec<_> = adapter
        .list_models("local")
        .await
        .unwrap()
        .iter()
        .map(|m| m.id.as_str().to_owned())
        .collect();
    assert_eq!(ids, ["qwen"]);
    let result = probe(&server.base_url, "qwen", Some(KEY), &quick(), None).await;
    assert_eq!(result.models, ["qwen"]);
}

// ---- a key split between two causes ----

#[derive(Debug)]
struct Link {
    text: String,
    source: Option<Box<Link>>,
}
impl std::fmt::Display for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}
impl std::error::Error for Link {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|s| s as &(dyn std::error::Error + 'static))
    }
}

fn chain(texts: &[&str]) -> Link {
    let mut link: Option<Link> = None;
    for text in texts.iter().rev() {
        link = Some(Link {
            text: (*text).to_owned(),
            source: link.map(Box::new),
        });
    }
    link.unwrap()
}

fn everything(error: &HarnessError) -> String {
    let mut parts = vec![format!("{error:?}"), error.to_string()];
    let mut next = error
        .cause()
        .map(|c| c as &(dyn std::error::Error + 'static));
    while let Some(cause) = next {
        parts.push(format!("{cause:?} {cause}"));
        next = cause.source();
    }
    parts.join(" | ")
}

#[test]
fn a_key_split_between_two_causes_does_not_come_out_in_pieces() {
    let (head, tail) = KEY.split_at(10);
    assert_eq!(format!("{head}{tail}"), KEY);
    let error = LlmError::new("request failed", BuiltinLlmFailureCode::Transport)
        .unwrap()
        .with_cause(chain(&[
            &format!("rejected {head}"),
            &format!("{tail} by the server"),
        ]));
    let clean = redact(error, Some(KEY));
    let all = everything(&clean);
    assert!(all.contains(REDACTED), "{all}");
    assert!(!all.contains(KEY), "{all}");
    assert!(!all.contains(head) && !all.contains(tail), "{all}");
    assert_eq!(clean.message(), "request failed");
    // Three links, key in the middle one and the last one.
    let error = LlmError::new("x", BuiltinLlmFailureCode::Transport)
        .unwrap()
        .with_cause(chain(&["a", &format!("b {head}"), &format!("{tail} c")]));
    let all = everything(&redact(error, Some(KEY)));
    assert!(!all.contains(head) && !all.contains(tail), "{all}");
}

// ---- resolve_model ----

#[tokio::test]
async fn a_resolved_model_name_with_the_key_is_cleaned() {
    let server = serve(vec![]).await;
    let adapter = adapter(&server, |c| c.api_key = Some(KEY.into()));
    for name in [
        KEY.to_owned(),
        KEY.to_ascii_uppercase(),
        format!("x-{KEY}-y"),
    ] {
        let info = adapter.resolve_model("local", &name, None).await.unwrap();
        let text = format!("{info:?}");
        assert!(
            !text
                .to_ascii_lowercase()
                .contains(&KEY.to_ascii_lowercase()),
            "{text}"
        );
        assert!(text.contains(REDACTED), "{text}");
    }
    let info = adapter.resolve_model("local", "qwen", None).await.unwrap();
    assert_eq!(info.model.id.as_str(), "qwen");
    assert_eq!(info.model.name, "qwen");
}

// ---- the model's own answer is not filtered ----

#[tokio::test]
async fn the_answer_of_the_model_is_passed_on_unchanged() {
    // Deliberate boundary (README): a model that repeats the key in its text, reasoning or tool
    // arguments is not edited; the user hands the key to the provider.
    let server = serve(vec![(
        CHAT,
        vec![Reply::sse_text(&format!(
            "{}{}{}{}{}",
            delta(json!({"reasoning_content": format!("think {KEY}")})),
            delta(json!({"content": format!("answer {KEY}")})),
            delta(json!({"tool_calls": [{"index": 0, "id": "c1",
                "function": {"name": "echo", "arguments": format!("{{\"city\":\"{KEY}\"}}")}}]})),
            finish("tool_calls"),
            done()
        ))],
    )])
    .await;
    let mut opts = options(vec![user("x")]);
    opts.tools = Some(vec![tool("echo", &[])]);
    let run = run(&adapter(&server, |c| c.api_key = Some(KEY.into())), opts).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    let blocks = run.blocks();
    assert_eq!(blocks[0], ("reasoning".to_owned(), format!("think {KEY}")));
    assert_eq!(blocks[1], ("text".to_owned(), format!("answer {KEY}")));
    assert_eq!(
        blocks[2],
        (
            "tool-call".to_owned(),
            format!("echo({{\"city\":\"{KEY}\"}})")
        )
    );
}

// ---- the retry rule: exact names and wordings ----

fn refusal(status: Option<u16>, message: &str) -> HarnessError {
    LlmError::with_options(
        message,
        BuiltinLlmFailureCode::InvalidRequest,
        LlmErrorOptions {
            status,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn recognized_wordings_are_a_refusal_of_the_field() {
    for message in [
        "Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.",
        "Unrecognized request argument supplied: max_tokens",
        "Unrecognized request arguments supplied: temperature_x, max_tokens",
        "Unrecognized request arguments supplied: 'a' and 'max_tokens'.",
        "Unsupported parameters: top_k and max_tokens.",
        "Unknown field: max_tokens",
        "unknown parameter: \"max_tokens\"",
        "Unknown argument `max_tokens`",
        "Unrecognized parameter: max_tokens",
        "UNSUPPORTED PARAMETER: MAX_TOKENS",
        "'max_tokens' is not supported with this model",
        "max_tokens is unsupported",
        "The field max_tokens is unsupported by this server.",
        "max_tokens: Extra inputs are not permitted",
        "body.max_tokens: Extra inputs are not permitted",
        "body.max_tokens: Extra inputs are not permitted; body.foo: Extra inputs are not permitted",
    ] {
        assert!(
            rejects_limit_field(&refusal(Some(400), message), "max_tokens"),
            "{message}"
        );
    }
    for message in [
        "Unsupported parameter: 'max_completion_tokens' is not supported with this model.",
        "Unknown field: max_completion_tokens",
        "body.max_completion_tokens: Extra inputs are not permitted",
    ] {
        assert!(
            rejects_limit_field(&refusal(Some(400), message), "max_completion_tokens"),
            "{message}"
        );
    }
}

#[test]
fn names_are_matched_as_whole_words() {
    for message in [
        "Unrecognized request argument supplied: max_tokens_extra",
        "Unrecognized request argument supplied: not_max_tokens",
        "Unknown field: xmax_tokens",
        "Unknown field: max_tokens2",
        "not_max_tokens is not supported",
        "max_tokens_extra is not supported",
        "xmax_tokens is unsupported",
        "max_tokens_extra: Extra inputs are not permitted",
        "body.my_max_tokens: Extra inputs are not permitted",
    ] {
        assert!(
            !rejects_limit_field(&refusal(Some(400), message), "max_tokens"),
            "{message}"
        );
    }
}

#[test]
fn other_complaints_are_not_a_refusal_of_the_field() {
    for message in [
        "max_tokens must be positive",
        "max_tokens is too large: 100000. This model supports at most 4096 completion tokens.",
        "max_tokens and max_completion_tokens cannot both be set",
        "Unsupported parameter: 'logit_bias'. max_tokens is fine.",
        "'logit_bias' is not supported. max_tokens is fine.",
        "Unsupported parameter: 'max_completion_tokens' is not supported with this model.",
        "Invalid request",
    ] {
        assert!(
            !rejects_limit_field(&refusal(Some(400), message), "max_tokens"),
            "{message:?}"
        );
    }
    // The right words, a wrong status or no status at all.
    for status in [
        Some(401),
        Some(404),
        Some(422),
        Some(429),
        Some(500),
        Some(503),
        None,
    ] {
        assert!(
            !rejects_limit_field(
                &refusal(
                    status,
                    "Unsupported parameter: 'max_tokens' is not supported."
                ),
                "max_tokens"
            ),
            "{status:?}"
        );
    }
}

// ---- FastAPI / Pydantic bodies ----

#[test]
fn error_fields_reads_a_fastapi_detail() {
    let fields = |body: serde_json::Value| error_fields(&body).and_then(|f| f.message);
    assert_eq!(
        fields(
            json!({"detail": [{"type": "extra_forbidden", "loc": ["body", "max_tokens"],
            "msg": "Extra inputs are not permitted", "input": 64}]})
        ),
        Some("body.max_tokens: Extra inputs are not permitted".into())
    );
    assert_eq!(
        fields(json!({"detail": [
            {"loc": ["body", "messages", 0, "content"], "msg": "Field required"},
            {"msg": "No location"},
            {"loc": ["body"], "note": "no msg, skipped"}]})),
        Some("body.messages.0.content: Field required; No location".into())
    );
    assert_eq!(
        fields(json!({"detail": "Model not found"})),
        Some("Model not found".into())
    );
    assert_eq!(fields(json!({"detail": 5})), None);
    assert_eq!(fields(json!({"detail": []})), None);
    assert_eq!(fields(json!({"detail": [{"loc": ["x"]}]})), None);
    assert_eq!(fields(json!({})), None);
    // The other shapes still work.
    assert_eq!(fields(json!({"error": {"message": "m"}})), Some("m".into()));
}

const PYDANTIC_400: &str = r#"{"detail":[{"type":"extra_forbidden","loc":["body","max_tokens"],"msg":"Extra inputs are not permitted","input":64}]}"#;

#[tokio::test]
async fn a_fastapi_detail_refusal_is_retried_with_the_other_field() {
    let server = serve(vec![(CHAT, vec![Reply::json(400, PYDANTIC_400), hello()])]).await;
    let run = run(&adapter(&server, |_| {}), limited_opts()).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    let bodies: Vec<_> = server.captured().iter().map(|r| r.json()).collect();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["max_tokens"], 64);
    assert_eq!(bodies[1]["max_completion_tokens"], 64);
    assert!(bodies[1].get("max_tokens").is_none());
}

#[tokio::test]
async fn the_same_body_with_another_status_is_not_retried() {
    for status in [422, 500] {
        let server = serve(vec![(
            CHAT,
            vec![Reply::json(status, PYDANTIC_400), hello()],
        )])
        .await;
        let run = run(&adapter(&server, |_| {}), limited_opts()).await;
        assert!(run.error.is_some(), "{status}");
        assert_eq!(server.captured().len(), 1, "{status}");
    }
}

#[tokio::test]
async fn the_probe_retries_after_the_new_wordings_too() {
    for body in [
        PYDANTIC_400,
        r#"{"error":{"message":"Unrecognized request argument supplied: max_tokens","type":"invalid_request_error"}}"#,
        r#"{"error":{"message":"Unknown field: max_tokens"}}"#,
    ] {
        let server = serve(vec![
            (
                "GET /v1/models",
                vec![Reply::json(200, r#"{"data":[{"id":"qwen"}]}"#)],
            ),
            (
                CHAT,
                vec![Reply::json(400, body), Reply::json(200, COMPLETION)],
            ),
        ])
        .await;
        let result = probe(&server.base_url, "qwen", None, &quick(), None).await;
        assert!(result.ok && result.tool_calls, "{body}: {result:?}");
        let bodies: Vec<_> = server.captured()[1..].iter().map(|r| r.json()).collect();
        assert_eq!(bodies[1]["max_completion_tokens"], 1, "{body}");
    }
}

#[tokio::test]
async fn a_name_that_only_looks_like_the_field_is_not_retried() {
    let server = serve(vec![(
        CHAT,
        vec![Reply::json(
            400,
            r#"{"error":{"message":"Unrecognized request argument supplied: max_tokens_extra","type":"invalid_request_error"}}"#,
        )],
    )])
    .await;
    let run = run(&adapter(&server, |_| {}), limited_opts()).await;
    assert_eq!(run.code(), "INVALID_REQUEST");
    assert_eq!(server.captured().len(), 1);
}
