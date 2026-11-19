//! Second review round: the API key in model ids and in chains of causes, short keys, and a
//! narrower retry rule for the output-limit field. Replies follow the documented formats.
mod support;
use hoh_error::HarnessError;
use hoh_llm::{BuiltinLlmFailureCode, LlmAdapter, LlmError};
use hoh_llm_openai::error::{REDACTED, redact, redact_text};
use hoh_llm_openai::{ProbeOptions, probe};
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

fn limited_opts() -> hoh_llm::GenerateOptions {
    let mut opts = options(vec![user("hi")]);
    opts.max_tokens = Some(64);
    opts
}

// ---- model ids ----

fn models_with_key() -> String {
    format!(
        r#"{{"object":"list","data":[{{"id":"qwen"}},{{"id":"{KEY}"}},{{"id":"id-{KEY}-x"}}]}}"#
    )
}

#[tokio::test]
async fn a_model_id_that_contains_the_key_is_dropped_from_the_list() {
    let server = serve(vec![(
        "GET /v1/models",
        vec![Reply::json(200, &models_with_key())],
    )])
    .await;
    let adapter = adapter(&server, |c| c.api_key = Some(KEY.into()));
    let models = adapter.list_models("local").await.unwrap();
    let ids: Vec<_> = models.iter().map(|m| m.id.as_str().to_owned()).collect();
    assert_eq!(ids, ["qwen"]);
    assert!(!format!("{models:?}").contains(KEY));
}

#[tokio::test]
async fn a_model_id_that_contains_the_key_is_dropped_from_the_probe() {
    let server = serve(vec![
        ("GET /v1/models", vec![Reply::json(200, &models_with_key())]),
        (CHAT, vec![Reply::json(200, COMPLETION)]),
    ])
    .await;
    let result = probe(&server.base_url, "qwen", Some(KEY), &quick(), None).await;
    assert!(result.ok, "{result:?}");
    assert_eq!(result.models, ["qwen"]);
    assert_eq!(result.model_listed, Some(true));
    let all = format!("{result:?} {}", serde_json::to_string(&result).unwrap());
    assert!(!all.contains(KEY), "{all}");
}

// ---- the chain of causes ----

#[derive(Debug)]
struct Inner(String);
impl std::fmt::Display for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "inner failure: {}", self.0)
    }
}
impl std::error::Error for Inner {}

#[derive(Debug)]
struct Outer {
    note: &'static str,
    source: Inner,
}
impl std::fmt::Display for Outer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.note)
    }
}
impl std::error::Error for Outer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn llm_error(message: &str) -> HarnessError {
    LlmError::new(message, BuiltinLlmFailureCode::Transport).unwrap()
}

#[test]
fn a_key_only_in_a_cause_is_removed_from_the_whole_chain() {
    let error = llm_error("request failed").with_cause(Outer {
        note: "outer",
        source: Inner(format!("rejected {KEY}")),
    });
    assert!(format!("{error:?}").contains(KEY), "the test needs a leak");
    let clean = redact(error, Some(KEY));
    assert_eq!(
        (clean.code(), clean.message()),
        ("TRANSPORT", "request failed")
    );
    let mut seen = vec![format!("{clean:?}"), clean.to_string()];
    let mut next = clean
        .cause()
        .map(|c| c as &(dyn std::error::Error + 'static));
    while let Some(cause) = next {
        seen.push(format!("{cause:?} {cause}"));
        next = cause.source();
    }
    let all = seen.join(" | ");
    assert!(!all.contains(KEY), "{all}");
    assert!(all.contains("inner failure: rejected [redacted]"), "{all}");
}

#[test]
fn a_clean_error_is_returned_unchanged() {
    let error = llm_error("plain").with_cause(Outer {
        note: "outer",
        source: Inner("fine".into()),
    });
    let same = redact(error.clone(), Some(KEY));
    assert_eq!(format!("{same:?}"), format!("{error:?}"));
    assert!(same.cause().is_some_and(|c| c.to_string() == "outer"));
}

// ---- short keys ----

#[test]
fn a_one_letter_key_is_replaced_only_as_a_whole_token() {
    let text = "Invalid API key: a. Cannot parse a banana; value a-b and a_c stay, (a) goes";
    let clean = redact_text(text, Some("a"));
    assert_eq!(
        clean,
        format!(
            "Invalid API key: {REDACTED}. Cannot parse {REDACTED} banana; value a-b and a_c stay, ({REDACTED}) goes"
        )
    );
    // Key at the very start and end of the text.
    assert_eq!(redact_text("a", Some("a")), REDACTED);
    assert_eq!(
        redact_text("abc12 and abc123 and abc12", Some("abc12")),
        format!("{REDACTED} and abc123 and {REDACTED}")
    );
}

#[test]
fn a_long_key_is_cut_out_wherever_it_occurs() {
    let key = "12345678";
    assert_eq!(
        redact_text("x12345678y 12345678", Some(key)),
        format!("x{REDACTED}y {REDACTED}")
    );
    assert_eq!(redact_text("nothing here", Some("")), "nothing here");
    assert_eq!(redact_text("nothing here", None), "nothing here");
}

#[tokio::test]
async fn a_short_key_does_not_wreck_the_diagnostic_of_an_http_error() {
    let server = serve(vec![(
        CHAT,
        vec![Reply::json(
            401,
            r#"{"error":{"message":"Invalid API key: a","type":"authentication_error","code":401}}"#,
        )],
    )])
    .await;
    let run = run(
        &adapter(&server, |c| c.api_key = Some("a".into())),
        options(vec![user("hi")]),
    )
    .await;
    assert_eq!(run.code(), "AUTH");
    assert_eq!(
        run.error.unwrap().message(),
        format!("Invalid API key: {REDACTED}")
    );
}

// ---- the retry rule for max_tokens ----

async fn requests_after(status: u16, body: &str) -> (usize, String) {
    let server = serve(vec![(CHAT, vec![Reply::json(status, body)])]).await;
    let run = run(&adapter(&server, |_| {}), limited_opts()).await;
    (server.captured().len(), run.code().to_owned())
}

#[tokio::test]
async fn only_a_400_about_an_unsupported_parameter_is_retried() {
    // 400 that mentions both field names but says nothing about support.
    let (n, code) = requests_after(
        400,
        r#"{"error":{"message":"max_tokens must be positive; max_completion_tokens is a different field","type":"invalid_request_error"}}"#,
    )
    .await;
    assert_eq!((n, code.as_str()), (1, "INVALID_REQUEST"));
    // The right words with another status.
    for status in [422, 500] {
        let (n, _) = requests_after(
            status,
            r#"{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model."}}"#,
        )
        .await;
        assert_eq!(n, 1, "status {status}");
    }
    // A 400 about another parameter that is unsupported while max_tokens is only mentioned.
    let (n, _) = requests_after(
        400,
        r#"{"error":{"message":"Unsupported parameter: 'logit_bias'. max_tokens is fine.","type":"invalid_request_error"}}"#,
    )
    .await;
    assert_eq!(n, 1);
    // The pydantic wording of a server that forbids unknown fields is a refusal of the field.
    let (n, _) = requests_after(
        400,
        r#"{"error":{"message":"max_tokens: Extra inputs are not permitted","type":"invalid_request_error"}}"#,
    )
    .await;
    assert_eq!(n, 2);
}
