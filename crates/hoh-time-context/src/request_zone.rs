//! Browser-zone facts from committed or proposed user-rpc messages.

use hoh_error::BoxError;
use hoh_llm::UserMessage;
use hoh_util_time::{TimeZoneCause, TimeZoneRejection, TimeZoneResult, canonicalize_time_zone};
use hoh_util_values::js_json_stringify;
use serde_json::{Value, json};

use crate::TimeContextError;

/// Browser-zone facts for the messages selected while preparing a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserTimeZoneContext {
    Resolved { time_zone: String },
    Mixed { time_zones: Vec<String> },
    Missing,
}

/// Read a raw zone reported by a user-rpc source.
pub fn reported_browser_time_zone(message: &UserMessage) -> Option<&str> {
    (message.source.kind == "user"
        && message
            .source
            .data
            .get("rpcId")
            .is_some_and(Value::is_string))
    .then(|| {
        message
            .source
            .data
            .get("clientTimeZone")
            .and_then(Value::as_str)
    })
    .flatten()
}

fn quoted(value: &str) -> String {
    js_json_stringify(&Value::String(value.to_owned()))
}

fn browser_time_zone(value: &str) -> Result<String, BoxError> {
    match canonicalize_time_zone(value) {
        TimeZoneResult::Accepted { time_zone } if time_zone == value => Ok(time_zone),
        TimeZoneResult::Accepted { .. } => Err(Box::new(TimeContextError::TypeError {
            message: format!("browser time zone must be canonical: {}", quoted(value)),
            cause: None,
        })),
        TimeZoneResult::Rejected {
            reason: TimeZoneRejection::Unsupported,
            cause,
        } => Err(Box::new(TimeContextError::TypeError {
            message: format!("browser time zone is unsupported: {}", quoted(value)),
            cause: cause.map(|TimeZoneCause::RangeError { message }| {
                Box::new(TimeContextError::RangeError(message)) as BoxError
            }),
        })),
        TimeZoneResult::Rejected {
            reason:
                TimeZoneRejection::Blank
                | TimeZoneRejection::Padded
                | TimeZoneRejection::NotAreaLocation,
            ..
        } => Err(Box::new(TimeContextError::TypeError {
            message: format!(
                "browser time zone must be canonical UTC or IANA Area/Location: {}",
                quoted(value)
            ),
            cause: None,
        })),
        TimeZoneResult::Rejected {
            reason: TimeZoneRejection::NotCanonicalShape,
            ..
        } => Err(Box::new(TimeContextError::TypeError {
            message: format!("browser time zone must be canonical: {}", quoted(value)),
            cause: None,
        })),
    }
}

/// Derive unique, mixed, or missing browser-zone facts.
pub fn derive_browser_time_zone_context(
    messages: &[UserMessage],
    entered_time_zones: &[String],
) -> Result<BrowserTimeZoneContext, BoxError> {
    let mut time_zones = Vec::new();
    for value in entered_time_zones
        .iter()
        .map(String::as_str)
        .chain(messages.iter().filter_map(reported_browser_time_zone))
    {
        let time_zone = browser_time_zone(value)?;
        if !time_zones.contains(&time_zone) {
            time_zones.push(time_zone);
        }
    }
    time_zones.sort();
    Ok(match time_zones.len() {
        0 => BrowserTimeZoneContext::Missing,
        1 => BrowserTimeZoneContext::Resolved {
            time_zone: time_zones.remove(0),
        },
        _ => BrowserTimeZoneContext::Mixed { time_zones },
    })
}

/// Render the model policy for one browser-zone context.
pub fn render_browser_time_zone_context(context: &BrowserTimeZoneContext) -> String {
    match context {
        BrowserTimeZoneContext::Resolved { time_zone } => format!(
            "Browser time zone for this request: {time_zone}. Interpret otherwise-unqualified dates and times in this zone."
        ),
        BrowserTimeZoneContext::Mixed { time_zones } => format!(
            "Browser time zone for this request: mixed {}. Ask the user to clarify otherwise-unqualified dates and times.",
            js_json_stringify(&json!(time_zones))
        ),
        BrowserTimeZoneContext::Missing => "Browser time zone for this request: unavailable. Ask the user to clarify otherwise-unqualified dates and times.".into(),
    }
}
