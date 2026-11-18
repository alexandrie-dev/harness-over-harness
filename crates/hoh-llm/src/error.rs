use crate::types::{LlmFailure, ProviderRequestId};
pub use hoh_error::HarnessError;
use serde::{Deserialize, Serialize};
use std::error::Error;

macro_rules! codes {
    ($($variant:ident => $code:literal $(, $choice:literal)?);* $(;)?) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum BuiltinLlmFailureCode { $($variant,)* Http(u16) }
        impl BuiltinLlmFailureCode {
            pub fn as_str(&self) -> std::borrow::Cow<'static, str> { match self { $(Self::$variant => $code.into(),)* Self::Http(status) => format!("HTTP_{status}").into() } }
        }
        pub const LLM_FAILURE_CODES: FailureCodes = FailureCodes(&[$(($code, codes!(@choice $($choice)?))),*]);
    };
    (@choice $value:literal) => {$value};
    (@choice) => {false};
}
/// Ordered code directory. Serialization retains the source object-of-objects shape.
#[derive(Clone, Copy, Debug)]
pub struct FailureCodes(&'static [(&'static str, bool)]);
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureCodeInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_choice: Option<bool>,
}
impl FailureCodes {
    pub fn iter(&self) -> std::slice::Iter<'static, (&'static str, bool)> {
        self.0.iter()
    }
    pub fn get(&self, code: &str) -> Option<FailureCodeInfo> {
        self.0
            .iter()
            .find(|(name, _)| *name == code)
            .map(|(_, choice)| FailureCodeInfo {
                retry_choice: choice.then_some(true),
            })
    }
}
impl Serialize for FailureCodes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut codes = serializer.serialize_map(Some(self.0.len()))?;
        for (code, choice) in self.0 {
            codes.serialize_entry(
                code,
                &FailureCodeInfo {
                    retry_choice: choice.then_some(true),
                },
            )?;
        }
        codes.end()
    }
}
codes! {
    Aborted => "ABORTED"; AccountQuota => "ACCOUNT_QUOTA"; AccountSignInRequired => "ACCOUNT_SIGN_IN_REQUIRED";
    AccountTokenInvalid => "ACCOUNT_TOKEN_INVALID"; Auth => "AUTH"; ClientUpgradeRequired => "CLIENT_UPGRADE_REQUIRED";
    ContextWindowExceeded => "CONTEXT_WINDOW_EXCEEDED"; DiscoveryFailed => "DISCOVERY_FAILED";
    DiscoveryUnsupported => "DISCOVERY_UNSUPPORTED"; DuplicateAdapter => "DUPLICATE_ADAPTER";
    DuplicateDirectory => "DUPLICATE_DIRECTORY"; DuplicateDiscovery => "DUPLICATE_DISCOVERY";
    EmptyResponse => "EMPTY_RESPONSE", true; FilesApi => "FILES_API"; ImageOffloadRequired => "IMAGE_OFFLOAD_REQUIRED";
    InvalidAdapter => "INVALID_ADAPTER"; InvalidCatalog => "INVALID_CATALOG"; InvalidConfig => "INVALID_CONFIG";
    InvalidCredential => "INVALID_CREDENTIAL"; InvalidDirectory => "INVALID_DIRECTORY"; InvalidDiscovery => "INVALID_DISCOVERY";
    InvalidModelContext => "INVALID_MODEL_CONTEXT"; InvalidModelInfo => "INVALID_MODEL_INFO";
    InvalidModelMaxTokens => "INVALID_MODEL_MAX_TOKENS"; InvalidModelReasoning => "INVALID_MODEL_REASONING";
    InvalidPreparedCall => "INVALID_PREPARED_CALL"; InvalidReplayState => "INVALID_REPLAY_STATE";
    InvalidRequest => "INVALID_REQUEST"; InvalidResponse => "INVALID_RESPONSE"; MalformedResponse => "MALFORMED_RESPONSE", true;
    MissingCredential => "MISSING_CREDENTIAL"; ModelSwitchCompactionFailed => "MODEL_SWITCH_COMPACTION_FAILED";
    ModelUnsupported => "MODEL_UNSUPPORTED"; NoAdapter => "NO_ADAPTER"; NoCredentialStore => "NO_CREDENTIAL_STORE";
    NoDiscovery => "NO_DISCOVERY"; PiAiError => "PI_AI_ERROR"; Quota => "QUOTA", true; RateLimit => "RATE_LIMIT", true;
    RegistrationDisposed => "REGISTRATION_DISPOSED"; RequestExtension => "REQUEST_EXTENSION"; RequestHeaders => "REQUEST_HEADERS";
    Server => "SERVER", true; StreamClosed => "STREAM_CLOSED", true; Timeout => "TIMEOUT", true; Transport => "TRANSPORT", true;
    Unknown => "UNKNOWN"; UnknownModel => "UNKNOWN_MODEL"; UnstorableProviderId => "UNSTORABLE_PROVIDER_ID";
    UnsupportedContent => "UNSUPPORTED_CONTENT"; UnsupportedOption => "UNSUPPORTED_OPTION";
    UnsupportedReasoningEffort => "UNSUPPORTED_REASONING_EFFORT";
}
pub const RETRY_CODE_CHOICES: &[&str] = &[
    "EMPTY_RESPONSE",
    "MALFORMED_RESPONSE",
    "QUOTA",
    "RATE_LIMIT",
    "SERVER",
    "STREAM_CLOSED",
    "TIMEOUT",
    "TRANSPORT",
];
pub const CONTEXT_WINDOW_EXCEEDED_CODE: &str = "CONTEXT_WINDOW_EXCEEDED";
pub const QUOTA_EXCEEDED_CODE: &str = "QUOTA";
pub const ACCOUNT_QUOTA_EXCEEDED_CODE: &str = "ACCOUNT_QUOTA";
pub const EMPTY_RESPONSE_CODE: &str = "EMPTY_RESPONSE";
pub const INVALID_CREDENTIAL_CODE: &str = "INVALID_CREDENTIAL";
pub const IMAGE_OFFLOAD_REQUIRED_CODE: &str = "IMAGE_OFFLOAD_REQUIRED";

#[derive(Clone, Debug, Default)]
pub struct LlmErrorOptions {
    pub cause: Option<hoh_error::SharedError>,
    pub status: Option<u16>,
    pub provider_retry_after_ms: Option<f64>,
    pub request_id: Option<ProviderRequestId>,
    pub offload_images: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValueError {
    #[error("{0}")]
    Message(String),
}
/// Constructor namespace for the source HarnessError subclass (PORTING §7).
pub struct LlmError;
impl LlmError {
    pub const NAME: &str = "LlmError";
    #[allow(clippy::new_ret_no_self)] // Constructor namespace required by PORTING §7.
    pub fn new(
        message: impl Into<String>,
        code: BuiltinLlmFailureCode,
    ) -> Result<HarnessError, ValueError> {
        Self::with_options(message, code, LlmErrorOptions::default())
    }
    pub fn with_options(
        message: impl Into<String>,
        code: BuiltinLlmFailureCode,
        options: LlmErrorOptions,
    ) -> Result<HarnessError, ValueError> {
        let failure = LlmFailure {
            message: message.into(),
            code: code.as_str().into_owned(),
            status: options.status,
            provider_retry_after_ms: options.provider_retry_after_ms,
            request_id: options.request_id,
            offload_images: options.offload_images,
        };
        Self::validate(&failure)?;
        Ok(Self::make(failure, options.cause))
    }
    pub fn from_failure(
        failure: &LlmFailure,
        cause: Option<hoh_error::SharedError>,
    ) -> Result<HarnessError, ValueError> {
        Self::validate(failure)?;
        Ok(Self::make(failure.clone(), cause))
    }
    fn validate(f: &LlmFailure) -> Result<(), ValueError> {
        let text = if f.message.is_empty() {
            Some("LlmError message must be a non-empty string")
        } else if f.code.is_empty() {
            Some("LlmError code must be a non-empty string")
        } else if f.status.is_some_and(|s| !(100..=599).contains(&s)) {
            Some("LlmError status must be an integer from 100 through 599")
        } else if f
            .provider_retry_after_ms
            .is_some_and(|n| !n.is_finite() || n <= 0.0)
        {
            Some("LlmError providerRetryAfterMs must be a positive finite number")
        } else if f.request_id.as_ref().is_some_and(|s| s.as_str().is_empty()) {
            Some("LlmError requestId must be a non-empty string")
        } else {
            None
        };
        match text {
            Some(text) => Err(ValueError::Message(text.into())),
            None => Ok(()),
        }
    }
    fn make(failure: LlmFailure, cause: Option<hoh_error::SharedError>) -> HarnessError {
        let error = HarnessError::named(Self::NAME, failure.message.clone(), failure.code.clone())
            .with_detail(failure);
        match cause {
            Some(cause) => error.with_cause(cause),
            None => error,
        }
    }
}
pub(crate) fn llm_error(message: impl Into<String>, code: BuiltinLlmFailureCode) -> HarnessError {
    // Internal callers use non-empty source diagnostics and a typed built-in code.
    LlmError::new(message, code).expect("internal LLM diagnostic is non-empty")
}
pub fn is_harness_error(value: &(dyn Error + 'static)) -> bool {
    HarnessError::of(value).is_some()
}

fn matches(detail: &str, patterns: &[&str]) -> bool {
    // JavaScript's non-u regex counts UTF-16 units and folds ASCII without
    // allowing Unicode case-fold additions such as Kelvin sign and long s.
    let detail: String = detail
        .encode_utf16()
        .map(|unit| {
            char::from_u32(u32::from(unit))
                .unwrap_or('\u{fffd}')
                .to_ascii_lowercase()
        })
        .collect();
    patterns.iter().any(|pattern| {
        // The expressions are fixed source patterns, with the JS whitespace/word classes made explicit.
        let pattern = pattern
            .replace(
                r"[\s_-]",
                &format!("[{}_-]", hoh_util_values::JS_WHITE_SPACE_SET),
            )
            .replace(r"\s", &format!("[{}]", hoh_util_values::JS_WHITE_SPACE_SET))
            .replace(r"\b", r"(?-u:\b)")
            .replace(".{0,40}", r"[^\n\r\u{2028}\u{2029}]{0,40}");
        regex::Regex::new(&pattern)
            .expect("source regex is valid")
            .is_match(&detail)
    })
}
pub fn is_context_window_exceeded_error(detail: &str) -> bool {
    matches(
        detail,
        &[
            r"(?:^|[^a-z0-9])context[\s_-](?:length|window)[\s_-](?:exceed(?:ed|s)?|overflow(?:ed)?|limit[\s_-]exceeded)(?:$|[^a-z0-9])",
            r"\b(?:maximum|max)(?:\s+(?:allowed|supported))?\s+context\s+(?:length|window)\b",
            r"\b(?:request|prompt|input|messages?)\s+(?:is\s+|are\s+)?too\s+(?:large|long)\s+for\s+(?:(?:this|the)\s+)?(?:model(?:'s)?\s+)?context(?:\s+window)?\b",
            r"\b(?:input|prompt|request)\s+(?:is\s+)?too\s+(?:long|large)\s+for\s+(?:this|the)\s+model\b",
            r"\b(?:input|prompt|request|messages?)\b.{0,40}\b(?:exceed(?:s|ed)?|overflows?|is\s+larger\s+than)\b.{0,40}\b(?:the\s+)?(?:model(?:'s)?\s+)?context(?:\s+(?:length|window))?\b",
        ],
    )
}
pub fn is_quota_exceeded_error(detail: &str) -> bool {
    matches(
        detail,
        &[
            r"\binsufficient[\s_-]+(?:quota|balance|credits?)\b",
            r"\b(?:quota|usage[\s_-]+limit)[\s_-]+(?:exceeded|exhausted|reached)\b",
            r"\bexceed(?:ed|s)?[\s_-]+(?:(?:your|the)[\s_-]+)?(?:current[\s_-]+)?quota\b",
            r"\b(?:balance|credits?)[\s_-]+(?:exhausted|depleted)\b",
            r"\bout[\s_-]+of[\s_-]+(?:credits?|budget)\b",
        ],
    )
}
/// An AggregateError has no standard-library equivalent; this representation retains its members.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct AggregateError {
    pub message: String,
    pub errors: Vec<hoh_error::SharedError>,
}
pub fn error_chain(value: &(dyn Error + 'static)) -> String {
    enum Work<'a> {
        Enter(&'a (dyn Error + 'static)),
        Finish {
            ptr: *const (),
            message: String,
            members: usize,
            cause: bool,
        },
    }
    let mut pending = vec![Work::Enter(value)];
    let mut path = std::collections::HashSet::new();
    let mut results: Vec<String> = vec![];
    while let Some(work) = pending.pop() {
        match work {
            Work::Enter(error) => {
                let ptr = std::ptr::from_ref(error).cast::<()>();
                if path.contains(&ptr) {
                    results.push("<circular cause>".into());
                    continue;
                }
                let facts = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut message = error.to_string();
                    if message.is_empty() {
                        message = HarnessError::of(error)
                            .map(|h| h.name())
                            .unwrap_or(if error.is::<AggregateError>() {
                                "AggregateError"
                            } else {
                                "Error"
                            })
                            .into();
                    }
                    let members = error
                        .downcast_ref::<AggregateError>()
                        .map(|a| {
                            a.errors
                                .iter()
                                .map(|e| e.get_ref() as &(dyn Error + 'static))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    (message, members, error.source())
                }));
                let Ok((message, members, cause)) = facts else {
                    results.push("<unrenderable value>".into());
                    continue;
                };
                path.insert(ptr);
                pending.push(Work::Finish {
                    ptr,
                    message,
                    members: members.len(),
                    cause: cause.is_some(),
                });
                if let Some(cause) = cause {
                    pending.push(Work::Enter(cause));
                }
                pending.extend(members.into_iter().rev().map(Work::Enter));
            }
            Work::Finish {
                ptr,
                mut message,
                members,
                cause,
            } => {
                // Every scheduled child contributes exactly one result before its parent's Finish.
                let children = results.split_off(results.len() - members - usize::from(cause));
                let cause_text = cause.then(|| &children[members]);
                let suppress = cause_text.is_none_or(|s| s.is_empty() || s == &message);
                if members > 0 {
                    message += " [";
                    message += &children[..members].join("; ");
                    message += "]";
                }
                if !suppress {
                    message += ": ";
                    message += &children[members];
                }
                path.remove(&ptr);
                results.push(message);
            }
        }
    }
    results.pop().unwrap_or_default()
}
pub fn error_chain_value(value: &serde_json::Value) -> String {
    value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| hoh_util_values::js_string(value))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDiscoveryRejectedDetails {
    pub settings_ns: String,
    #[serde(rename = "baseURL", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}
