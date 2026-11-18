use crate::error::{RETRY_CODE_CHOICES, ValueError};
use hoh_timeout::MAX_TIMER_DELAY_MS;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
pub const DEFAULT_RETRYABLE_CODES: &[&str] = &[
    "EMPTY_RESPONSE",
    "RATE_LIMIT",
    "SERVER",
    "TIMEOUT",
    "TRANSPORT",
];
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackoffConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_delay_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter_ratio: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum RetryPolicyConfig {
    Normal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_retries: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retryable_codes: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backoff: Option<BackoffConfig>,
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    Always {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backoff: Option<BackoffConfig>,
        // Layered normal-only values are ignored even when malformed.
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}
impl schemars::JsonSchema for RetryPolicyConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RetryPolicyConfig".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // This fixed schema describes the same tagged configuration; custom code choices
        // and inactive normal fields in always mode cannot be derived from the enum alone.
        schemars::Schema::try_from(retry_policy_schema()).expect("retry schema is an object")
    }
}
pub type NormalRetryPolicyConfig = RetryPolicyConfig;
pub type AlwaysRetryPolicyConfig = RetryPolicyConfig;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRetryBackoff {
    pub initial_delay_ms: f64,
    pub max_delay_ms: f64,
    pub jitter_ratio: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum ResolvedRetryPolicy {
    Normal {
        max_retries: u64,
        retryable_codes: Vec<String>,
        #[serde(flatten)]
        backoff: ResolvedRetryBackoff,
    },
    Always {
        #[serde(flatten)]
        backoff: ResolvedRetryBackoff,
    },
}
pub type ResolvedNormalRetryPolicy = ResolvedRetryPolicy;
pub type ResolvedAlwaysRetryPolicy = ResolvedRetryPolicy;
fn fail(text: impl Into<String>) -> ValueError {
    ValueError::Message(text.into())
}
fn keys(value: &Map<String, Value>, allowed: &[&str], path: &str) -> Result<(), ValueError> {
    for key in value.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(fail(format!("{path}: unknown key \"{key}\"")));
        }
    }
    Ok(())
}
pub fn resolve_retry_policy(
    config: Option<&RetryPolicyConfig>,
    path: &str,
) -> Result<ResolvedRetryPolicy, ValueError> {
    let backoff = |config: Option<&BackoffConfig>| -> Result<ResolvedRetryBackoff, ValueError> {
        let path = format!("{path}.backoff");
        if let Some(config) = config {
            keys(&config.extra, &[], &path)?;
        }
        let initial_delay_ms = config.and_then(|b| b.initial_delay_ms).unwrap_or(500.0);
        let max_delay_ms = config.and_then(|b| b.max_delay_ms).unwrap_or(10_000.0);
        let jitter_ratio = config.and_then(|b| b.jitter_ratio).unwrap_or(0.1);
        for (key, n) in [
            ("initialDelayMs", initial_delay_ms),
            ("maxDelayMs", max_delay_ms),
        ] {
            if !n.is_finite() || n <= 0.0 || n > MAX_TIMER_DELAY_MS as f64 {
                return Err(fail(format!(
                    "{path}.{key} must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
                )));
            }
        }
        if initial_delay_ms > max_delay_ms {
            return Err(fail(format!(
                "{path}.initialDelayMs must be less than or equal to maxDelayMs"
            )));
        }
        if !jitter_ratio.is_finite() || !(0.0..=1.0).contains(&jitter_ratio) {
            return Err(fail(format!("{path}.jitterRatio must be between 0 and 1")));
        }
        Ok(ResolvedRetryBackoff {
            initial_delay_ms,
            max_delay_ms,
            jitter_ratio,
        })
    };
    let Some(config) = config else {
        return Ok(ResolvedRetryPolicy::Normal {
            max_retries: 5,
            retryable_codes: DEFAULT_RETRYABLE_CODES
                .iter()
                .map(|c| (*c).into())
                .collect(),
            backoff: backoff(None)?,
        });
    };
    match config {
        RetryPolicyConfig::Normal {
            max_retries,
            retryable_codes,
            backoff: configured,
            extra,
        } => {
            keys(extra, &[], path)?;
            let max_retries = max_retries.unwrap_or(5.0);
            if !max_retries.is_finite()
                || max_retries.fract() != 0.0
                || !(0.0..=9_007_199_254_740_991.0).contains(&max_retries)
            {
                return Err(fail(format!(
                    "{path}.maxRetries must be a non-negative safe integer"
                )));
            }
            let retryable_codes = retryable_codes.clone().unwrap_or_else(|| {
                DEFAULT_RETRYABLE_CODES
                    .iter()
                    .map(|c| (*c).into())
                    .collect()
            });
            if retryable_codes.is_empty() {
                return Err(fail(format!("{path}.retryableCodes must not be empty")));
            }
            if retryable_codes.iter().any(String::is_empty) {
                return Err(fail(format!(
                    "{path}.retryableCodes must contain only non-empty strings"
                )));
            }
            let unique: indexmap::IndexSet<_> = retryable_codes.iter().collect();
            if unique.len() != retryable_codes.len() {
                return Err(fail(format!(
                    "{path}.retryableCodes must not contain duplicates"
                )));
            }
            // The finite, integral, non-negative safe-integer bound above makes this exact.
            Ok(ResolvedRetryPolicy::Normal {
                max_retries: max_retries as u64,
                retryable_codes,
                backoff: backoff(configured.as_ref())?,
            })
        }
        RetryPolicyConfig::Always {
            backoff: configured,
            extra,
        } => {
            keys(extra, &["maxRetries", "retryableCodes"], path)?;
            Ok(ResolvedRetryPolicy::Always {
                backoff: backoff(configured.as_ref())?,
            })
        }
    }
}
pub fn resolve_retry_policy_value(
    config: Option<&Value>,
    path: &str,
) -> Result<ResolvedRetryPolicy, ValueError> {
    let defaults = json!({"mode":"normal"});
    let config = config.unwrap_or(&defaults);
    let mode = config.get("mode").and_then(Value::as_str).unwrap_or("");
    if mode != "normal" && mode != "always" {
        return Err(fail(format!(
            "{path}.mode must be \"normal\" or \"always\""
        )));
    }
    let object = config
        .as_object()
        .ok_or_else(|| fail(format!("{path}.mode must be \"normal\" or \"always\"")))?;
    keys(
        object,
        &["mode", "maxRetries", "retryableCodes", "backoff"],
        path,
    )?;
    let max_retries = config
        .get("maxRetries")
        .filter(|v| !v.is_null())
        .unwrap_or(&json!(5))
        .clone();
    let retryable = config
        .get("retryableCodes")
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| json!(DEFAULT_RETRYABLE_CODES));
    let (max_retries, retryable_codes) = if mode == "normal" {
        let retries = max_retries
            .as_f64()
            .filter(|n| {
                n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n <= 9_007_199_254_740_991.0
            })
            .ok_or_else(|| {
                fail(format!(
                    "{path}.maxRetries must be a non-negative safe integer"
                ))
            })?;
        let codes = retryable.as_array().ok_or_else(|| {
            fail(format!(
                "{path}.retryableCodes must contain only non-empty strings"
            ))
        })?;
        if codes.is_empty() {
            return Err(fail(format!("{path}.retryableCodes must not be empty")));
        }
        let mut seen = indexmap::IndexSet::new();
        for code in codes {
            let code = code.as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
                fail(format!(
                    "{path}.retryableCodes must contain only non-empty strings"
                ))
            })?;
            if !seen.insert(code.to_owned()) {
                return Err(fail(format!(
                    "{path}.retryableCodes must not contain duplicates"
                )));
            }
        }
        (retries as u64, seen.into_iter().collect())
    } else {
        (0, vec![])
    };
    let backoff = config.get("backoff").and_then(Value::as_object);
    if let Some(b) = backoff {
        keys(
            b,
            &["initialDelayMs", "maxDelayMs", "jitterRatio"],
            &format!("{path}.backoff"),
        )?;
    }
    let number = |key: &str, default: f64| {
        backoff
            .and_then(|b| b.get(key))
            .filter(|v| !v.is_null())
            .map_or(Some(default), Value::as_f64)
            .unwrap_or(f64::NAN)
    };
    let initial_delay_ms = number("initialDelayMs", 500.0);
    let max_delay_ms = number("maxDelayMs", 10_000.0);
    let jitter_ratio = number("jitterRatio", 0.1);
    for (key, n) in [
        ("initialDelayMs", initial_delay_ms),
        ("maxDelayMs", max_delay_ms),
    ] {
        if !n.is_finite() || n <= 0.0 || n > MAX_TIMER_DELAY_MS as f64 {
            return Err(fail(format!(
                "{path}.backoff.{key} must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
            )));
        }
    }
    if initial_delay_ms > max_delay_ms {
        return Err(fail(format!(
            "{path}.backoff.initialDelayMs must be less than or equal to maxDelayMs"
        )));
    }
    if !jitter_ratio.is_finite() || !(0.0..=1.0).contains(&jitter_ratio) {
        return Err(fail(format!(
            "{path}.backoff.jitterRatio must be between 0 and 1"
        )));
    }
    let backoff = ResolvedRetryBackoff {
        initial_delay_ms,
        max_delay_ms,
        jitter_ratio,
    };
    Ok(if mode == "normal" {
        ResolvedRetryPolicy::Normal {
            max_retries,
            retryable_codes,
            backoff,
        }
    } else {
        ResolvedRetryPolicy::Always { backoff }
    })
}
/// Explicit JSON Schema replaces the serialized schemastery definition.
pub fn retry_policy_schema() -> Value {
    let backoff = json!({"type":"object","properties":{
        "initialDelayMs":{"type":"number","exclusiveMinimum":0,"maximum":MAX_TIMER_DELAY_MS,"default":500},
        "maxDelayMs":{"type":"number","exclusiveMinimum":0,"maximum":MAX_TIMER_DELAY_MS,"default":10000},
        "jitterRatio":{"type":"number","minimum":0,"maximum":1,"default":0.1}
    }});
    json!({"oneOf":[
        {"type":"object","required":["mode"],"properties":{"mode":{"const":"normal"},"maxRetries":{"type":"integer","minimum":0,"maximum":9007199254740991_u64,"default":5},"retryableCodes":{"type":"array","minItems":1,"default":DEFAULT_RETRYABLE_CODES,"items":{"anyOf":[{"enum":RETRY_CODE_CHOICES},{"type":"string","minLength":1}]}},"backoff":backoff}},
        {"type":"object","required":["mode"],"properties":{"mode":{"const":"always"},"backoff":backoff}}
    ]})
}
pub struct RetryPolicySchema;
impl RetryPolicySchema {
    pub fn schema() -> Value {
        retry_policy_schema()
    }
    pub fn validate(value: &Value) -> Result<ResolvedRetryPolicy, ValueError> {
        resolve_retry_policy_value(Some(value), "retryPolicy")
    }
}
