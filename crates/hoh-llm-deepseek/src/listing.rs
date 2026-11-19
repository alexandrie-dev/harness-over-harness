//! Authenticated `GET /models` listing and entry decoding.

use hoh_error::HarnessError;
use hoh_llm::{
    APP_IDENTITY, BuiltinLlmFailureCode as Code, LlmError, LlmErrorOptions, LlmModelBadge,
    LlmModelBadgeTone, attribution_headers,
};
use hoh_package_manifest::LocalizedText;
use hoh_timeout::{AbortSignal, timeout_of};
use hoh_util_values::js_trim;
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

pub const MAX_BADGE_TEXT_LENGTH: usize = 12;
pub const LISTING_TIMEOUT: &str = "MODEL_LISTING_TIMEOUT";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ClientRequirement {
    None,
    MinVersion { version: String },
    Invalid,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedFacts {
    pub input_modalities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_update: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_update: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ListedBadge {
    Badge { badge: LlmModelBadge },
    Invalid { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ListedCapabilities {
    Usable { usable: True, facts: ListedFacts },
    Unusable { usable: False, reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum True {
    #[serde(rename = "true")]
    Value,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum False {
    #[serde(rename = "false")]
    Value,
}

impl ListedCapabilities {
    pub fn facts(&self) -> Option<&ListedFacts> {
        match self {
            Self::Usable { facts, .. } => Some(facts),
            Self::Unusable { .. } => None,
        }
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Unusable { reason, .. } => Some(reason),
            Self::Usable { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListingEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub requirement: ClientRequirement,
    pub capabilities: ListedCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<ListedBadge>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelListing {
    pub entries: Vec<ListingEntry>,
    pub skipped: usize,
}

fn llm_error(message: impl Into<String>, code: Code) -> HarnessError {
    LlmError::new(message, code).expect("listing diagnostics are non-empty")
}

pub fn model_listing_url(base_url: &str) -> Result<String, url::ParseError> {
    let base = Url::parse(base_url)?;
    let origin = base.origin().ascii_serialization();
    Ok(format!("{origin}/models"))
}

fn object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn requirement_of(entry: &Map<String, Value>) -> ClientRequirement {
    let Some(value) = entry.get("client_requirements") else {
        return ClientRequirement::None;
    };
    let Some(requirements) = object(value) else {
        return ClientRequirement::Invalid;
    };
    let Some(own) = requirements.get(APP_IDENTITY.product) else {
        return ClientRequirement::None;
    };
    let Some(own) = object(own) else {
        return ClientRequirement::Invalid;
    };
    match own.get("min_version") {
        None => ClientRequirement::None,
        Some(Value::String(version)) if !version.is_empty() => ClientRequirement::MinVersion {
            version: version.clone(),
        },
        Some(_) => ClientRequirement::Invalid,
    }
}

fn known_modalities(value: &Value) -> Option<Vec<String>> {
    let listed = value.as_array()?;
    let mut out = Vec::new();
    for value in listed {
        let Some(item) = value.as_str() else { continue };
        if matches!(item, "text" | "image") && !out.iter().any(|known| known == item) {
            out.push(item.into());
        }
    }
    Some(out)
}

fn positive_integer(value: Option<&Value>) -> Result<Option<u64>, ()> {
    let Some(value) = value else { return Ok(None) };
    value
        .as_u64()
        .filter(|value| *value > 0)
        .map(Some)
        .ok_or(())
}

fn capabilities_of(entry: &Map<String, Value>) -> ListedCapabilities {
    let messages = entry
        .get("api_capabilities")
        .and_then(object)
        .and_then(|value| value.get("anthropic_messages"))
        .and_then(object);
    let Some(messages) = messages else {
        return ListedCapabilities::Unusable {
            usable: False::Value,
            reason: "declares no api_capabilities.anthropic_messages".into(),
        };
    };
    let outputs = entry
        .get("output_modalities")
        .map(known_modalities)
        .unwrap_or_else(|| Some(vec!["text".into()]));
    if !outputs.is_some_and(|items| items.iter().any(|item| item == "text")) {
        return ListedCapabilities::Unusable {
            usable: False::Value,
            reason: "declares no text output".into(),
        };
    }
    let inputs = entry
        .get("input_modalities")
        .map(known_modalities)
        .unwrap_or_else(|| Some(vec!["text".into()]));
    let Some(inputs) = inputs.filter(|items| !items.is_empty()) else {
        return ListedCapabilities::Unusable {
            usable: False::Value,
            reason: "declares no input modality DeepSeek Harness supports".into(),
        };
    };
    let (Ok(context_window), Ok(max_output_tokens)) = (
        positive_integer(entry.get("context_window")),
        positive_integer(entry.get("max_output_tokens")),
    ) else {
        return ListedCapabilities::Unusable {
            usable: False::Value,
            reason: "declares an invalid context_window or max_output_tokens".into(),
        };
    };
    let system_prompt_update = messages
        .get("system_prompt_update")
        .filter(|value| !value.is_null())
        .map(|value| {
            if value.as_str() == Some("in-history") {
                "in-history"
            } else {
                "leading-only"
            }
            .into()
        });
    let tool_update = messages
        .get("tool_update")
        .filter(|value| !value.is_null())
        .map(|value| match value.as_str() {
            Some("in-history") => "in-history",
            Some("addition-only") => "addition-only",
            _ => "leading-only",
        })
        .map(str::to_owned);
    ListedCapabilities::Usable {
        usable: True::Value,
        facts: ListedFacts {
            input_modalities: inputs,
            context_window,
            max_output_tokens,
            system_prompt_update,
            tool_update,
        },
    }
}

fn is_ignored_character(ch: char) -> bool {
    matches!(ch, '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{115f}'..='\u{1160}' | '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}')
}

fn grapheme_count(text: &str) -> usize {
    let mut count = 0;
    let mut joined = false;
    for ch in text.chars() {
        if ch == '\u{200d}' {
            joined = true;
            continue;
        }
        if ('\u{1f3fb}'..='\u{1f3ff}').contains(&ch)
            || ('\u{0300}'..='\u{036f}').contains(&ch)
            || ('\u{fe00}'..='\u{fe0f}').contains(&ch)
        {
            continue;
        }
        if joined {
            joined = false;
        } else {
            count += 1;
        }
    }
    count
}

fn badge_text(value: &Value) -> Option<String> {
    let original = value.as_str()?;
    let text = js_trim(original);
    let unreadable = original.chars().any(|ch| {
        ch.is_control()
            || matches!(ch, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    });
    let visible = text
        .chars()
        .any(|ch| !ch.is_whitespace() && !is_ignored_character(ch));
    (!unreadable && visible && grapheme_count(text) <= MAX_BADGE_TEXT_LENGTH).then(|| text.into())
}

fn localized_badge_text(value: &Value) -> Option<LocalizedText> {
    if value.is_string() {
        return badge_text(value).map(LocalizedText::Text);
    }
    let variants = object(value)?;
    let mut texts = IndexMap::new();
    for (language, value) in variants {
        let valid_language = {
            let mut pieces = language.split('-');
            pieces.next().is_some_and(|part| {
                (2..=8).contains(&part.len()) && part.bytes().all(|ch| ch.is_ascii_lowercase())
            }) && pieces.all(|part| {
                (1..=8).contains(&part.len())
                    && part
                        .bytes()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
            })
        };
        if !valid_language || language == "__proto__" {
            return None;
        }
        texts.insert(language.clone(), badge_text(value)?);
    }
    texts
        .contains_key("en")
        .then_some(LocalizedText::Translations(texts))
}

fn optional_object(value: Option<&Value>) -> Result<Option<&Map<String, Value>>, ()> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => object(value).map(Some).ok_or(()),
    }
}

fn badge_of(entry: &Map<String, Value>) -> Option<ListedBadge> {
    let own_name = format!("client_metadata[\"{}\"]", APP_IDENTITY.product);
    let metadata = match optional_object(entry.get("client_metadata")) {
        Ok(value) => value,
        Err(()) => {
            return Some(ListedBadge::Invalid {
                reason: "client_metadata is not an object".into(),
            });
        }
    };
    let own = match optional_object(metadata.and_then(|value| value.get(APP_IDENTITY.product))) {
        Ok(value) => value,
        Err(()) => {
            return Some(ListedBadge::Invalid {
                reason: format!("{own_name} is not an object"),
            });
        }
    };
    let badge = match optional_object(own.and_then(|value| value.get("badge"))) {
        Ok(value) => value,
        Err(()) => {
            return Some(ListedBadge::Invalid {
                reason: format!("{own_name}.badge is not an object"),
            });
        }
    }?;
    let text = badge.get("text").and_then(localized_badge_text);
    let Some(text) = text else {
        return Some(ListedBadge::Invalid {
            reason: format!(
                "{own_name}.badge.text is not a string or an object of lowercase language ids including \"en\", each text 1 to {MAX_BADGE_TEXT_LENGTH} characters on one line"
            ),
        });
    };
    let tone = match badge.get("tone").and_then(Value::as_str) {
        Some("info") => LlmModelBadgeTone::Info,
        Some("success") => LlmModelBadgeTone::Success,
        Some("warning") => LlmModelBadgeTone::Warning,
        _ => LlmModelBadgeTone::Neutral,
    };
    Some(ListedBadge::Badge {
        badge: LlmModelBadge { text, tone },
    })
}

pub fn parse_model_listing(body: &Value) -> Result<ModelListing, HarnessError> {
    let rows = body
        .as_object()
        .and_then(|body| body.get("data"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            llm_error(
                "DeepSeek model listing has no \"data\" array",
                Code::InvalidResponse,
            )
        })?;
    let mut entries = Vec::new();
    let mut seen = IndexSet::new();
    for row in rows {
        let Some(entry) = object(row) else { continue };
        let Some(id) = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        entries.push(ListingEntry {
            id: id.into(),
            name: entry
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_owned),
            requirement: requirement_of(entry),
            capabilities: capabilities_of(entry),
            badge: badge_of(entry),
        });
    }
    if entries.is_empty() {
        return Err(llm_error(
            "DeepSeek model listing contains no model entry",
            Code::InvalidResponse,
        ));
    }
    Ok(ModelListing {
        skipped: rows.len() - entries.len(),
        entries,
    })
}

pub async fn request_model_listing(
    url: &str,
    headers: &indexmap::IndexMap<String, String>,
    signal: &AbortSignal,
) -> Result<ModelListing, HarnessError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| llm_error(format!("could not reach {url}: {error}"), Code::Transport))?;
    let mut request = client.get(url).header("accept", "application/json");
    for (name, value) in attribution_headers(None)
        .unwrap_or_default()
        .iter()
        .chain(headers.iter())
    {
        request = request.header(name, value);
    }
    let operation = async {
        let response = request.send().await.map_err(|error| {
            llm_error(format!("could not reach {url}: {error}"), Code::Transport)
        })?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|error| {
            llm_error(format!("could not reach {url}: {error}"), Code::Transport)
        })?;
        if !(200..300).contains(&status) {
            return Err(LlmError::with_options(
                format!("{url} answered HTTP {status}"),
                Code::Http(status),
                LlmErrorOptions {
                    status: Some(status),
                    ..Default::default()
                },
            )
            .expect("HTTP diagnostic is valid"));
        }
        let body: Value = serde_json::from_str(&text).map_err(|error| {
            LlmError::with_options(
                format!("{url} did not answer with JSON"),
                Code::InvalidResponse,
                LlmErrorOptions {
                    cause: Some(hoh_error::SharedError::new(error)),
                    ..Default::default()
                },
            )
            .expect("JSON diagnostic is valid")
        })?;
        parse_model_listing(&body)
    };
    tokio::select! {
        biased;
        reason = signal.cancelled() => {
            if let Some(timeout) = timeout_of(Some(&reason), Some(LISTING_TIMEOUT)) {
                Err(llm_error(format!("DeepSeek model listing request to {url} timed out after {} ms", timeout.timeout_ms), Code::Timeout))
            } else {
                Err(llm_error(format!("DeepSeek model listing request to {url} was aborted"), Code::Aborted))
            }
        }
        result = operation => result,
    }
}
