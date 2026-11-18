use hoh_llm::{adapter_failure::*, *};
use serde_json::json;
#[derive(Debug, thiserror::Error)]
#[error("provider failed")]
struct ForeignError;
#[derive(Debug)]
struct Hostile;
impl std::fmt::Display for Hostile {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("message getter failed")
    }
}
impl std::error::Error for Hostile {}
#[test]
fn normalizes_empty_primitive_throws_and_data_descriptors_without_values() {
    assert_eq!(
        normalize_llm_failure_value(&json!("")),
        LlmFailure::new("LLM adapter failed", "UNKNOWN")
    );
    assert_eq!(
        normalize_llm_failure_value(&json!(null)),
        LlmFailure::new("null", "UNKNOWN")
    );
    assert_eq!(
        normalize_llm_failure(&ForeignError),
        LlmFailure::new("provider failed", "UNKNOWN")
    );
}
#[test]
fn rejects_malformed_failure_snapshots() {
    for failure in [
        LlmFailure {
            request_id: Some("".into()),
            ..LlmFailure::new("provider failed", "FOREIGN")
        },
        LlmFailure {
            status: Some(99),
            ..LlmFailure::new("provider failed", "FOREIGN")
        },
        LlmFailure {
            offload_images: Some(0),
            ..LlmFailure::new("provider failed", "FOREIGN")
        },
    ] {
        let e = AdapterFailure {
            message: "provider failed".into(),
            code: "FOREIGN".into(),
            failure,
        };
        assert_eq!(
            normalize_llm_failure(&e),
            LlmFailure::new("provider failed", "UNKNOWN")
        );
    }
}
#[test]
fn falls_back_when_an_error_message_accessor_throws() {
    assert_eq!(
        normalize_llm_failure(&Hostile),
        LlmFailure::new("LLM adapter failed", "UNKNOWN")
    );
}
#[test]
fn trusts_carried_facts_only_when_codes_agree() {
    let f = LlmFailure {
        status: Some(429),
        ..LlmFailure::new("busy", "FOREIGN")
    };
    let mut e = AdapterFailure {
        message: "provider failed".into(),
        code: "FOREIGN".into(),
        failure: f.clone(),
    };
    assert_eq!(normalize_llm_failure(&e), f);
    e.code = "OTHER".into();
    assert_eq!(normalize_llm_failure(&e).code, "UNKNOWN");
}

#[test]
fn preserves_carried_facts_through_a_shared_error() {
    let failure = LlmFailure {
        status: Some(429),
        ..LlmFailure::new("busy", "FOREIGN")
    };
    let error = hoh_error::SharedError::new(AdapterFailure {
        message: "busy".into(),
        code: "FOREIGN".into(),
        failure: failure.clone(),
    });
    assert_eq!(normalize_llm_failure(&error), failure);
}
