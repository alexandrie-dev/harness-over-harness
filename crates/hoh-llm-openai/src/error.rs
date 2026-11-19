//! Failure construction and classification of HTTP and in-band errors.
use hoh_error::HarnessError;
use hoh_llm::{
    BuiltinLlmFailureCode, LlmError, LlmErrorOptions, LlmFailure, ProviderRequestId,
    is_context_window_exceeded_error, is_quota_exceeded_error,
};
use serde_json::Value;

pub(crate) fn failure(code: BuiltinLlmFailureCode, message: String) -> HarnessError {
    failure_with(code, message, LlmErrorOptions::default())
}

pub(crate) fn failure_with(
    code: BuiltinLlmFailureCode,
    message: String,
    options: LlmErrorOptions,
) -> HarnessError {
    let fallback = code.as_str().into_owned();
    // An empty message is the only way construction fails; keep the code in that case.
    LlmError::with_options(message, code, options)
        .unwrap_or_else(|_| HarnessError::named(LlmError::NAME, "LLM request failed", fallback))
}

/// Pieces of a server error body; each server shapes it a little differently.
#[derive(Debug, Default, PartialEq)]
pub struct ErrorFields {
    pub message: Option<String>,
    pub kind: String,
    pub code: Option<Value>,
}

/// FastAPI / Pydantic: `{"detail": "text"}` or `{"detail": [{"type":"extra_forbidden",
/// "loc":["body","max_tokens"], "msg":"Extra inputs are not permitted"}]}`, flattened into
/// `body.max_tokens: Extra inputs are not permitted`.
fn detail_fields(detail: &Value) -> Option<ErrorFields> {
    let message = match detail {
        Value::String(text) => text.clone(),
        Value::Array(entries) => {
            let lines: Vec<String> = entries
                .iter()
                .filter_map(|entry| {
                    let msg = entry.get("msg").and_then(Value::as_str)?;
                    let loc: Vec<String> = entry
                        .get("loc")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|part| match part {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        })
                        .collect();
                    Some(if loc.is_empty() {
                        msg.to_owned()
                    } else {
                        format!("{}: {msg}", loc.join("."))
                    })
                })
                .collect();
            if lines.is_empty() {
                return None;
            }
            lines.join("; ")
        }
        _ => return None,
    };
    Some(ErrorFields {
        message: Some(message),
        ..Default::default()
    })
}

/// vLLM: `{"object":"error","message":..,"type":..,"code":400}`;
/// llama.cpp and Ollama: `{"error":{"message":..,"type":..,"code":..}}`;
/// Ollama native style: `{"error":"text"}`.
pub fn error_fields(body: &Value) -> Option<ErrorFields> {
    let object = match body.get("error") {
        Some(Value::String(text)) => {
            return Some(ErrorFields {
                message: Some(text.clone()),
                ..Default::default()
            });
        }
        Some(inner @ Value::Object(_)) => inner,
        Some(_) | None if body.get("object").and_then(Value::as_str) == Some("error") => body,
        None => {
            return detail_fields(body.get("detail")?);
        }
        _ => return None,
    };
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(ErrorFields {
        message: text("message"),
        kind: text("type").unwrap_or_default(),
        code: object.get("code").filter(|c| !c.is_null()).cloned(),
    })
}

fn numeric_status(code: &Option<Value>) -> Option<u16> {
    let value = code.as_ref()?;
    let n = match value {
        Value::Number(n) => n.as_u64()?,
        Value::String(s) => s.parse().ok()?,
        _ => return None,
    };
    u16::try_from(n).ok().filter(|n| (400..600).contains(n))
}

/// Seconds, or an HTTP date that this adapter does not parse (local servers send seconds).
pub fn retry_after_ms(value: &str) -> Option<f64> {
    let seconds: f64 = value.trim().parse().ok()?;
    (seconds.is_finite() && seconds > 0.0).then_some(seconds * 1000.0)
}

/// Classify a server error. `status` is the HTTP status when the error preceded streaming;
/// in-band errors may carry a status-like `code` instead (llama.cpp).
pub fn classify(
    fields: &ErrorFields,
    status: Option<u16>,
    retry_after: Option<f64>,
    request_id: Option<String>,
) -> HarnessError {
    let message = fields.message.clone().unwrap_or_else(|| match status {
        Some(status) => format!("OpenAI-compatible request failed ({status})"),
        None => "OpenAI-compatible stream error".to_owned(),
    });
    let code_text = match &fields.code {
        Some(Value::String(s)) => s.as_str(),
        _ => "",
    };
    let detail = format!("{} {code_text} {message}", fields.kind);
    let effective = status.or_else(|| numeric_status(&fields.code));
    let lowered = detail.to_ascii_lowercase();
    let code = if matches!(effective, Some(401 | 403)) {
        BuiltinLlmFailureCode::Auth
    } else if is_quota_exceeded_error(&detail) || effective == Some(402) {
        BuiltinLlmFailureCode::Quota
    } else if effective == Some(429) {
        BuiltinLlmFailureCode::RateLimit
    } else if is_context_window_exceeded_error(&detail)
        // llama.cpp: type "exceed_context_size_error", "…exceeds the available context size…".
        || lowered.contains("exceed_context_size")
        || lowered.contains("available context size")
    {
        BuiltinLlmFailureCode::ContextWindowExceeded
    } else if effective == Some(404) && lowered.contains("model") {
        BuiltinLlmFailureCode::UnknownModel
    } else if matches!(effective, Some(400 | 413 | 422)) || fields.kind.contains("invalid_request")
    {
        BuiltinLlmFailureCode::InvalidRequest
    } else if effective.is_none_or(|s| s >= 500) {
        BuiltinLlmFailureCode::Server
    } else {
        BuiltinLlmFailureCode::Http(effective.unwrap_or(500))
    };
    failure_with(
        code,
        message,
        LlmErrorOptions {
            status,
            provider_retry_after_ms: retry_after.filter(|n| n.is_finite() && *n > 0.0),
            request_id: request_id
                .filter(|id| !id.is_empty())
                .map(ProviderRequestId::from),
            ..Default::default()
        },
    )
}

/// Error for a non-2xx response whose body may or may not be JSON.
pub fn http_error(
    status: u16,
    body: &str,
    retry_after: Option<f64>,
    request_id: Option<String>,
) -> HarnessError {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let mut fields = parsed.as_ref().and_then(error_fields).unwrap_or_default();
    if fields.message.is_none() {
        let text = body.trim();
        if !text.is_empty() {
            fields.message = Some(truncate(text, 500));
        }
    }
    classify(&fields, Some(status), retry_after, request_id)
}

pub fn truncate(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        None => text.to_owned(),
        Some((end, _)) => format!("{}…", &text[..end]),
    }
}

/// Marker that replaces the API key in diagnostics.
pub const REDACTED: &str = "[redacted]";

/// A key shorter than this is replaced only where it stands as a whole token. Replacing every
/// occurrence of a one-letter key would turn each such letter of a message into the marker and
/// destroy the diagnostic, while a real API key is far longer.
pub const MIN_SUBSTRING_KEY_CHARS: usize = 8;

fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn usable(secret: Option<&str>) -> Option<&str> {
    secret.filter(|key| !key.is_empty())
}

/// `text` without the API key in use. A server may echo the key it rejected ("Invalid API key: …").
/// Keys of [`MIN_SUBSTRING_KEY_CHARS`] characters or more are cut out wherever they occur,
/// ignoring ASCII case; shorter ones only where they are a whole token with the same case (not
/// touching a letter, digit, `_` or `-`).
pub fn redact_text(text: &str, secret: Option<&str>) -> String {
    let Some(key) = usable(secret) else {
        return text.to_owned();
    };
    if key.chars().count() >= MIN_SUBSTRING_KEY_CHARS {
        // ASCII case-insensitive: a provider may echo the key in another case. Lowercasing ASCII
        // keeps every byte offset of `text`, so the matches index the original string.
        let (lowered, wanted) = (text.to_ascii_lowercase(), key.to_ascii_lowercase());
        let mut out = String::with_capacity(text.len());
        let mut copied = 0;
        for (start, found) in lowered.match_indices(&wanted) {
            out.push_str(&text[copied..start]);
            out.push_str(REDACTED);
            copied = start + found.len();
        }
        out.push_str(&text[copied..]);
        return out;
    }
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (start, found) in text.match_indices(key) {
        let end = start + found.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        if before.is_some_and(is_token_char) || after.is_some_and(is_token_char) {
            continue;
        }
        out.push_str(&text[copied..start]);
        out.push_str(REDACTED);
        copied = end;
    }
    out.push_str(&text[copied..]);
    out
}

/// Whether [`redact_text`] would change `text`.
pub fn mentions_secret(text: &str, secret: Option<&str>) -> bool {
    usable(secret).is_some() && redact_text(text, secret) != text
}

/// Display and Debug of every error in the chain starting at `cause`.
fn chain_texts(cause: &(dyn std::error::Error + 'static)) -> Vec<String> {
    let mut texts = Vec::new();
    let mut next = Some(cause);
    while let Some(error) = next {
        texts.push(error.to_string());
        texts.push(format!("{error:?}"));
        next = error.source();
    }
    texts
}

/// The error with the API key removed from its message, its failure facts and its whole chain of
/// causes. An error that does not contain the key is returned unchanged. Otherwise it is rebuilt;
/// its causes are replaced by one error that carries their texts, scrubbed (a cause cannot be
/// cloned, and its own Debug output could carry the key).
pub fn redact(error: HarnessError, secret: Option<&str>) -> HarnessError {
    let Some(_) = usable(secret) else {
        return error;
    };
    let facts = error.detail::<LlmFailure>();
    let cause_texts: Vec<String> = error
        .cause()
        .map(|cause| chain_texts(cause))
        .unwrap_or_default();
    // A key can be split between two causes: look at the chain written out as one text, too.
    let displays: Vec<String> = cause_texts.iter().step_by(2).cloned().collect();
    let debugs: Vec<String> = cause_texts.iter().skip(1).step_by(2).cloned().collect();
    let split =
        mentions_secret(&displays.concat(), secret) || mentions_secret(&debugs.concat(), secret);
    let leaks = mentions_secret(error.message(), secret)
        || facts.is_some_and(|f| {
            mentions_secret(&f.message, secret)
                || f.request_id
                    .as_ref()
                    .is_some_and(|id| mentions_secret(id.as_str(), secret))
        })
        || split
        || cause_texts.iter().any(|text| mentions_secret(text, secret));
    if !leaks {
        return error;
    }
    let rebuilt = match facts {
        Some(facts) => {
            let mut facts = facts.clone();
            facts.message = redact_text(&facts.message, secret);
            facts.request_id = facts
                .request_id
                .filter(|id| !mentions_secret(id.as_str(), secret));
            LlmError::from_failure(&facts, None).ok()
        }
        None => None,
    }
    .unwrap_or_else(|| {
        HarnessError::named(
            LlmError::NAME,
            redact_text(error.message(), secret),
            error.code(),
        )
    });
    match error.cause() {
        Some(cause) => {
            // Display of each cause in the chain, outermost first, each scrubbed on its own.
            let mut shown = Vec::new();
            let mut next: Option<&(dyn std::error::Error + 'static)> = Some(cause);
            while let Some(link) = next {
                shown.push(redact_text(&link.to_string(), secret));
                next = link.source();
            }
            // Pieces of a key split between causes survive the scrub above: scrub the joined text.
            let text = if mentions_secret(&shown.concat(), secret) {
                redact_text(&shown.concat(), secret)
            } else {
                shown.join(": ")
            };
            rebuilt.with_cause(HarnessError::named(LlmError::NAME, text, error.code()))
        }
        None => rebuilt,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `text` (before separators) ends with `field` as a whole word.
fn ends_with_word(text: &str, field: &str) -> bool {
    text.strip_suffix(field)
        .is_some_and(|rest| !rest.chars().next_back().is_some_and(is_word_char))
}

/// The names listed right after a phrase ("Unrecognized request arguments supplied: a, max_tokens.")
/// include `field`. Reading stops at the first thing that is not a name and a comma or `and`.
fn lists_field(after: &str, field: &str) -> bool {
    let separator = |c: char| matches!(c, '\'' | '"' | '`' | ':') || c.is_whitespace();
    let mut rest = after;
    loop {
        rest = rest.trim_start_matches(separator);
        let end = rest.find(|c: char| !is_word_char(c)).unwrap_or(rest.len());
        let (name, tail) = rest.split_at(end);
        if name.is_empty() {
            return false;
        }
        if name == field {
            return true;
        }
        let tail = tail.trim_start_matches(['\'', '"', '`']);
        match tail
            .strip_prefix(',')
            .or_else(|| tail.trim_start().strip_prefix("and "))
        {
            Some(more) => rest = more,
            None => return false,
        }
    }
}

/// A 400 whose text says that the output-limit parameter `field` itself is not supported.
/// Recognized, compared ignoring case, with `field` as a whole word:
/// - `Unsupported parameter: 'max_tokens' …` (OpenAI), `Unrecognized request argument supplied:
///   max_tokens` (OpenAI), `Unknown field: max_tokens`, `Unrecognized parameter …`,
///   `Unknown parameter/argument …` — the name (or a list of names) follows the phrase;
/// - `'max_tokens' is not supported …`, `max_tokens is unsupported`, `max_tokens: Extra inputs
///   are not permitted` (Pydantic; FastAPI `detail` entries are flattened to
///   `body.max_tokens: Extra inputs are not permitted`) — the name precedes the phrase.
///
/// Any other status, and any 400 that merely mentions the field (a value that is too large, another
/// parameter being the unsupported one, `max_tokens_extra`), is not a reason to retry.
pub fn rejects_limit_field(error: &HarnessError, field: &str) -> bool {
    let is_400 = error
        .detail::<LlmFailure>()
        .is_some_and(|facts| facts.status == Some(400));
    if !is_400 {
        return false;
    }
    let message = error.message().to_ascii_lowercase();
    let field = field.to_ascii_lowercase();
    let separator = |c: char| matches!(c, '\'' | '"' | '`' | ':') || c.is_whitespace();
    let named_after = [
        "unsupported parameters",
        "unsupported parameter",
        "unrecognized request arguments supplied",
        "unrecognized request argument supplied",
        "unrecognized request argument",
        "unrecognized parameter",
        "unrecognized argument",
        "unrecognized field",
        "unknown parameter",
        "unknown argument",
        "unknown field",
    ]
    .iter()
    .any(|phrase| {
        message
            .match_indices(phrase)
            .any(|(at, _)| lists_field(&message[at + phrase.len()..], &field))
    });
    let named_before = [
        "is not supported",
        "is unsupported",
        "extra inputs are not permitted",
    ]
    .iter()
    .any(|phrase| {
        message
            .match_indices(phrase)
            .any(|(at, _)| ends_with_word(message[..at].trim_end_matches(separator), &field))
    });
    named_after || named_before
}
