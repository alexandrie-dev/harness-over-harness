mod common;
use common::*;
use hoh_llm::*;
use serde_json::json;
#[test]
fn resolves_immutable_normal_defaults() {
    assert_eq!(
        value(resolve_retry_policy(None, "provider.retryPolicy").unwrap()),
        json!({"mode":"normal","maxRetries":5,"retryableCodes":["EMPTY_RESPONSE","RATE_LIMIT","SERVER","TIMEOUT","TRANSPORT"],"initialDelayMs":500,"maxDelayMs":10000,"jitterRatio":0.1})
    );
}
#[test]
fn resolves_and_detaches_a_configured_normal_policy() {
    let mut config: RetryPolicyConfig = decode(
        json!({"mode":"normal","maxRetries":4,"retryableCodes":["BUSY"],"backoff":{"initialDelayMs":25,"maxDelayMs":100,"jitterRatio":0}}),
    );
    let p = resolve_retry_policy(Some(&config), "p").unwrap();
    if let RetryPolicyConfig::Normal {
        retryable_codes: Some(c),
        ..
    } = &mut config
    {
        c.push("LATE".into());
    }
    assert_eq!(
        value(p),
        json!({"mode":"normal","maxRetries":4,"retryableCodes":["BUSY"],"initialDelayMs":25,"maxDelayMs":100,"jitterRatio":0})
    );
}
#[test]
fn resolves_always_mode_with_default_backoff() {
    assert_eq!(
        value(resolve_retry_policy_value(Some(&json!({"mode":"always"})), "p").unwrap()),
        json!({"mode":"always","initialDelayMs":500,"maxDelayMs":10000,"jitterRatio":0.1})
    );
}
#[test]
fn ignores_normal_only_fields_retained_after_switching_to_always_mode() {
    let p = resolve_retry_policy_value(
        Some(&json!({"mode":"always","maxRetries":5,"retryableCodes":["SERVER"]})),
        "p",
    )
    .unwrap();
    assert_eq!(value(p)["mode"], "always");
}
#[test]
fn rejects_invalid_policy() {
    for (c, message) in [
        (json!({"mode":"normal","maxRetries":-1}), "maxRetries"),
        (json!({"mode":"normal","maxRetries":1.5}), "maxRetries"),
        (
            json!({"mode":"normal","maxRetries":9007199254740992_u64}),
            "maxRetries",
        ),
        (
            json!({"mode":"always","backoff":{"initialDelayMs":0}}),
            "initialDelayMs",
        ),
        (
            json!({"mode":"normal","backoff":{"maxDelayMs":"Infinity"}}),
            "maxDelayMs",
        ),
        (
            json!({"mode":"normal","backoff":{"initialDelayMs":2147483648_u64}}),
            "initialDelayMs",
        ),
        (
            json!({"mode":"always","backoff":{"maxDelayMs":2147483648_u64}}),
            "maxDelayMs",
        ),
        (
            json!({"mode":"normal","backoff":{"initialDelayMs":20,"maxDelayMs":10}}),
            "less than or equal",
        ),
        (
            json!({"mode":"always","backoff":{"jitterRatio":1.1}}),
            "jitterRatio",
        ),
        (
            json!({"mode":"normal","retryableCodes":[]}),
            "must not be empty",
        ),
        (
            json!({"mode":"normal","retryableCodes":["SERVER","SERVER"]}),
            "duplicates",
        ),
        (
            json!({"mode":"normal","retryableCodes":[""]}),
            "non-empty strings",
        ),
        (
            json!({"mode":"normal","retryableCodes":[429]}),
            "non-empty strings",
        ),
        (
            json!({"mode":"normal","maxRetires":1}),
            "unknown key \"maxRetires\"",
        ),
        (
            json!({"mode":"always","backoff":{"initialDelay":1}}),
            "unknown key \"initialDelay\"",
        ),
        (
            json!({"mode":"sometimes"}),
            "mode must be \"normal\" or \"always\"",
        ),
    ] {
        assert!(
            resolve_retry_policy_value(Some(&c), "provider.retryPolicy")
                .unwrap_err()
                .to_string()
                .contains(message),
            "{c}"
        );
    }
}
#[test]
fn declares_the_resolver_bound_in_the_schema() {
    for c in [
        json!({"mode":"always","backoff":{"initialDelayMs":0}}),
        json!({"mode":"normal","retryableCodes":[]}),
        json!({"mode":"normal","retryableCodes":[""]}),
    ] {
        assert!(RetryPolicySchema::validate(&c).is_err());
    }
    let s = retry_policy_schema();
    assert_eq!(s["oneOf"][0]["properties"]["retryableCodes"]["minItems"], 1);
}
#[test]
fn accepts_the_smallest_values_inside_the_schema_bounds_and_still_defaults_the_codes() {
    let p=RetryPolicySchema::validate(&json!({"mode":"normal","retryableCodes":["X"],"backoff":{"initialDelayMs":f64::from_bits(1)}})).unwrap();
    assert_eq!(
        value(p)["backoff"]["initialDelayMs"],
        json!(f64::from_bits(1))
    );
    assert_eq!(
        value(RetryPolicySchema::validate(&json!({"mode":"normal"})).unwrap())["retryableCodes"],
        json!(DEFAULT_RETRYABLE_CODES)
    );
}
#[test]
fn publishes_the_retry_choices_through_the_serialized_schema_and_still_admits_other_codes() {
    assert_eq!(
        value(LLM_FAILURE_CODES)["STREAM_CLOSED"],
        json!({"retryChoice":true})
    );
    assert_eq!(value(LLM_FAILURE_CODES)["AUTH"], json!({}));
    let restored: serde_json::Value = decode(value(retry_policy_schema()));
    assert_eq!(
        restored["oneOf"][0]["properties"]["retryableCodes"]["items"]["anyOf"][0]["enum"],
        json!(RETRY_CODE_CHOICES)
    );
    assert_eq!(
        LLM_FAILURE_CODES
            .iter()
            .filter(|(_, v)| *v)
            .map(|(c, _)| *c)
            .collect::<Vec<_>>(),
        RETRY_CODE_CHOICES
    );
    assert!(
        RetryPolicySchema::validate(
            &json!({"mode":"normal","retryableCodes":["SERVER","Overloaded_529"]})
        )
        .is_ok()
    );
    assert!(!DEFAULT_RETRYABLE_CODES.contains(&"STREAM_CLOSED"));
}

#[test]
fn validates_typed_non_finite_backoff_and_publishes_its_bounds_through_schemars() {
    for (initial_delay_ms, max_delay_ms) in [(Some(f64::NAN), None), (None, Some(f64::INFINITY))] {
        let config = RetryPolicyConfig::Always {
            backoff: Some(BackoffConfig {
                initial_delay_ms,
                max_delay_ms,
                ..Default::default()
            }),
            extra: Default::default(),
        };
        assert!(resolve_retry_policy(Some(&config), "provider.retryPolicy").is_err());
    }
    let schema = schemars::schema_for!(RetryPolicyConfig);
    let schema = value(schema);
    assert_eq!(
        schema["oneOf"][0]["properties"]["maxRetries"]["maximum"],
        json!(9007199254740991_u64)
    );
    assert_eq!(
        schema["oneOf"][0]["properties"]["retryableCodes"]["items"]["anyOf"][0]["enum"],
        json!(RETRY_CODE_CHOICES)
    );
}
