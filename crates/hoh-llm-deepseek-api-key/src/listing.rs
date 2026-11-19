use crate::config::{DeepSeekCatalogModel, ImagePixelBudget};
use hoh_error::{BoxError, HarnessError};
use hoh_llm::{
    APP_IDENTITY, BuiltinLlmFailureCode, LlmError, LlmModelBadge, LlmModelBadgeTone,
    attribution_headers,
};
use hoh_package_manifest::LocalizedText;
use hoh_timeout::AbortSignal;
use semver::Version;
use serde_json::{Map, Value};
use std::{collections::HashMap, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Requirement {
    None,
    MinVersion(String),
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListedFacts {
    pub input_modalities: Vec<String>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub system_prompt_update: Option<String>,
    pub tool_update: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ListedCapabilities {
    Usable(ListedFacts),
    Unusable(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ListingEntry {
    pub id: String,
    pub name: Option<String>,
    pub requirement: Requirement,
    pub capabilities: ListedCapabilities,
    pub badge: Option<Result<LlmModelBadge, String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModelListing {
    pub entries: Vec<ListingEntry>,
    pub skipped: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CatalogRefusal {
    UpgradeRequired(String),
    Unsupported,
}

#[derive(Clone, Debug)]
pub(crate) struct ListedCatalog {
    pub offered: Vec<DeepSeekCatalogModel>,
    pub resolution: Vec<DeepSeekCatalogModel>,
    pub refusals: HashMap<String, CatalogRefusal>,
    pub badges: HashMap<String, LlmModelBadge>,
    pub problems: Vec<String>,
}

fn fields(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn requirement(entry: &Map<String, Value>) -> Requirement {
    let Some(declared) = entry.get("client_requirements") else {
        return Requirement::None;
    };
    let Some(requirements) = fields(declared) else {
        return Requirement::Invalid;
    };
    let Some(own) = requirements.get(APP_IDENTITY.product) else {
        return Requirement::None;
    };
    let Some(own) = fields(own) else {
        return Requirement::Invalid;
    };
    match own.get("min_version") {
        None => Requirement::None,
        Some(Value::String(version)) if !version.is_empty() => {
            Requirement::MinVersion(version.clone())
        }
        _ => Requirement::Invalid,
    }
}

fn modalities(value: Option<&Value>, default: &[&str]) -> Option<Vec<String>> {
    let Some(value) = value else {
        return Some(default.iter().map(|value| (*value).into()).collect());
    };
    let array = value.as_array()?;
    let mut result = Vec::new();
    for value in array {
        let Some(value) = value.as_str() else {
            continue;
        };
        if matches!(value, "text" | "image") && !result.iter().any(|seen| seen == value) {
            result.push(value.into());
        }
    }
    Some(result)
}

fn positive_capacity(value: Option<&Value>) -> Result<Option<u64>, ()> {
    match value {
        None => Ok(None),
        Some(value) => {
            let value = value.as_f64().ok_or(())?;
            if value.is_finite()
                && value.fract() == 0.0
                && (1.0..=9_007_199_254_740_991.0).contains(&value)
            {
                Ok(Some(value as u64))
            } else {
                Err(())
            }
        }
    }
}

fn capabilities(entry: &Map<String, Value>) -> ListedCapabilities {
    let Some(messages) = entry
        .get("api_capabilities")
        .and_then(fields)
        .and_then(|value| value.get("anthropic_messages"))
        .and_then(fields)
    else {
        return ListedCapabilities::Unusable(
            "declares no api_capabilities.anthropic_messages".into(),
        );
    };
    let outputs = modalities(entry.get("output_modalities"), &["text"]);
    if outputs
        .as_ref()
        .is_none_or(|values| !values.iter().any(|value| value == "text"))
    {
        return ListedCapabilities::Unusable("declares no text output".into());
    }
    let Some(inputs) =
        modalities(entry.get("input_modalities"), &["text"]).filter(|values| !values.is_empty())
    else {
        return ListedCapabilities::Unusable(
            "declares no input modality DeepSeek Harness supports".into(),
        );
    };
    let Ok(context_window) = positive_capacity(entry.get("context_window")) else {
        return ListedCapabilities::Unusable(
            "declares an invalid context_window or max_output_tokens".into(),
        );
    };
    let Ok(max_output_tokens) = positive_capacity(entry.get("max_output_tokens")) else {
        return ListedCapabilities::Unusable(
            "declares an invalid context_window or max_output_tokens".into(),
        );
    };
    let prompt = messages
        .get("system_prompt_update")
        .filter(|value| !value.is_null());
    let tools = messages.get("tool_update").filter(|value| !value.is_null());
    ListedCapabilities::Usable(ListedFacts {
        input_modalities: inputs,
        context_window,
        max_output_tokens,
        system_prompt_update: prompt.map(|value| {
            if value.as_str() == Some("in-history") {
                "in-history"
            } else {
                "leading-only"
            }
            .into()
        }),
        tool_update: tools.map(|value| {
            if matches!(value.as_str(), Some("in-history" | "addition-only")) {
                value.as_str().expect("matched string")
            } else {
                "leading-only"
            }
            .into()
        }),
    })
}

fn language_id(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    (2..=8).contains(&first.len())
        && first.bytes().all(|byte| byte.is_ascii_lowercase())
        && parts.all(|part| {
            (1..=8).contains(&part.len())
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn default_ignorable(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{115f}'..='\u{1160}'
            | '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}' | '\u{3164}' | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}' | '\u{ffa0}' | '\u{fff0}'..='\u{fff8}'
            | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

fn badge_text(value: &str) -> Option<&str> {
    let text = hoh_util_values::js_trim(value);
    let readable = value.chars().all(|character| {
        !character.is_control()
            && !matches!(character, '\u{2028}' | '\u{2029}')
            && !matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    });
    let visible = text
        .chars()
        .any(|character| !character.is_whitespace() && !default_ignorable(character));
    (!text.is_empty() && text.chars().count() <= 12 && readable && visible).then_some(text)
}

fn badge(entry: &Map<String, Value>) -> Option<Result<LlmModelBadge, String>> {
    let metadata = match entry.get("client_metadata") {
        None | Some(Value::Null) => return None,
        Some(Value::Object(metadata)) => metadata,
        Some(_) => return Some(Err("client_metadata is not an object".into())),
    };
    let declared = metadata.get(APP_IDENTITY.product)?;
    if declared.is_null() {
        return None;
    }
    let Some(declared) = declared.as_object() else {
        return Some(Err(format!(
            "client_metadata[\"{}\"] is not an object",
            APP_IDENTITY.product
        )));
    };
    let declared = declared.get("badge")?;
    if declared.is_null() {
        return None;
    }
    let Some(declared) = declared.as_object() else {
        return Some(Err(format!(
            "client_metadata[\"{}\"].badge is not an object",
            APP_IDENTITY.product
        )));
    };
    let text = match declared.get("text") {
        Some(Value::String(value)) => {
            badge_text(value).map(|value| LocalizedText::Text(value.into()))
        }
        Some(Value::Object(values)) => {
            let mut translated = indexmap::IndexMap::new();
            let mut valid = true;
            for (language, value) in values {
                let Some(value) = value.as_str() else {
                    valid = false;
                    break;
                };
                let Some(value) = badge_text(value) else {
                    valid = false;
                    break;
                };
                if !language_id(language) {
                    valid = false;
                    break;
                }
                translated.insert(language.clone(), value.into());
            }
            (valid && translated.contains_key("en"))
                .then_some(LocalizedText::Translations(translated))
        }
        _ => None,
    };
    let Some(text) = text else {
        return Some(Err(format!(
            "client_metadata[\"{}\"].badge.text is not a string or an object of lowercase language ids including \"en\", each text 1 to 12 characters on one line",
            APP_IDENTITY.product
        )));
    };
    let tone = match declared.get("tone").and_then(Value::as_str) {
        Some("info") => LlmModelBadgeTone::Info,
        Some("success") => LlmModelBadgeTone::Success,
        Some("warning") => LlmModelBadgeTone::Warning,
        _ => LlmModelBadgeTone::Neutral,
    };
    Some(Ok(LlmModelBadge { text, tone }))
}

pub(crate) fn parse_model_listing(body: &Value) -> Result<ModelListing, HarnessError> {
    let rows = body.get("data").and_then(Value::as_array).ok_or_else(|| {
        LlmError::new(
            "DeepSeek model listing has no \"data\" array",
            BuiltinLlmFailureCode::InvalidResponse,
        )
        .expect("valid LLM error")
    })?;
    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for row in rows {
        let Some(entry) = fields(row) else { continue };
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
            requirement: requirement(entry),
            capabilities: capabilities(entry),
            badge: badge(entry),
        });
    }
    if entries.is_empty() {
        return Err(LlmError::new(
            "DeepSeek model listing contains no model entry",
            BuiltinLlmFailureCode::InvalidResponse,
        )
        .expect("valid LLM error"));
    }
    Ok(ModelListing {
        skipped: rows.len() - entries.len(),
        entries,
    })
}

fn redirect_error(url: &str, status: reqwest::StatusCode) -> Option<BoxError> {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308).then(|| {
        Box::new(
            LlmError::new(
                format!("could not reach {url}"),
                BuiltinLlmFailureCode::Transport,
            )
            .expect("valid LLM error"),
        ) as BoxError
    })
}

pub(crate) async fn request_model_listing(
    url: &str,
    api_key: &str,
    timeout_ms: f64,
    signal: Option<&AbortSignal>,
) -> Result<ModelListing, BoxError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let request = model_listing_request(&client, url, api_key)?;
    let exchange = async {
        let response = client.execute(request).await.map_err(|error| {
            LlmError::with_options(
                format!("could not reach {url}"),
                BuiltinLlmFailureCode::Transport,
                hoh_llm::LlmErrorOptions {
                    cause: Some(hoh_error::SharedError::new(Box::new(error))),
                    ..Default::default()
                },
            )
            .expect("valid LLM error")
        })?;
        let status = response.status();
        if let Some(error) = redirect_error(url, status) {
            return Err(error);
        }
        let text = response.text().await.map_err(|error| {
            Box::new(
                LlmError::with_options(
                    format!("could not reach {url}"),
                    BuiltinLlmFailureCode::Transport,
                    hoh_llm::LlmErrorOptions {
                        cause: Some(hoh_error::SharedError::new(Box::new(error))),
                        ..Default::default()
                    },
                )
                .expect("valid LLM error"),
            ) as BoxError
        })?;
        if !status.is_success() {
            return Err::<_, BoxError>(Box::new(
                LlmError::with_options(
                    format!("{url} answered HTTP {}", status.as_u16()),
                    BuiltinLlmFailureCode::Http(status.as_u16()),
                    hoh_llm::LlmErrorOptions {
                        status: Some(status.as_u16()),
                        ..Default::default()
                    },
                )
                .expect("valid LLM error"),
            ));
        }
        let body: Value = serde_json::from_str(&text).map_err(|error| {
            Box::new(
                LlmError::with_options(
                    format!("{url} did not answer with JSON"),
                    BuiltinLlmFailureCode::InvalidResponse,
                    hoh_llm::LlmErrorOptions {
                        cause: Some(hoh_error::SharedError::new(Box::new(error))),
                        ..Default::default()
                    },
                )
                .expect("valid LLM error"),
            ) as BoxError
        })?;
        parse_model_listing(&body).map_err(|error| Box::new(error) as BoxError)
    };
    let abort = async {
        match signal {
            Some(signal) => Some(signal.cancelled().await),
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        biased;
        reason = abort => Err(Box::new(LlmError::with_options(
            format!("DeepSeek model listing request to {url} was aborted"),
            BuiltinLlmFailureCode::Aborted,
            hoh_llm::LlmErrorOptions { cause: reason, ..Default::default() },
        ).expect("valid LLM error"))),
        result = tokio::time::timeout(Duration::from_millis(timeout_ms.max(1.0) as u64), exchange) => match result {
            Ok(value) => value,
            Err(error) => Err(Box::new(LlmError::with_options(
                format!("DeepSeek model listing request to {url} timed out after {timeout_ms} ms"),
                BuiltinLlmFailureCode::Timeout,
                hoh_llm::LlmErrorOptions { cause: Some(hoh_error::SharedError::new(Box::new(error))), ..Default::default() },
            ).expect("valid LLM error"))),
        }
    }
}

fn model_listing_request(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
) -> Result<reqwest::Request, BoxError> {
    let mut request = client
        .get(url)
        .header("accept", "application/json")
        .header("x-api-key", api_key);
    if let Ok(headers) = attribution_headers(None) {
        for (name, value) in headers {
            request = request.header(name, value);
        }
    }
    request.build().map_err(|error| Box::new(error) as BoxError)
}

fn layered(
    base: &DeepSeekCatalogModel,
    entry: &ListingEntry,
    facts: &ListedFacts,
    route_max_tokens: u64,
) -> DeepSeekCatalogModel {
    let mut model = base.clone();
    if let Some(name) = &entry.name {
        model.name = Some(name.clone());
    }
    model.context_window = facts.context_window.or(model.context_window);
    model.max_tokens = facts
        .max_output_tokens
        .map(|cap| route_max_tokens.min(cap))
        .or(model.max_tokens);
    model.input_modalities = Some(facts.input_modalities.clone());
    model.system_prompt_update = match facts.system_prompt_update.as_deref() {
        Some("in-history") => Some("in-history".into()),
        Some("leading-only") => None,
        _ => model.system_prompt_update,
    };
    model.tool_update = match facts.tool_update.as_deref() {
        Some("in-history") => Some("in-history".into()),
        Some("addition-only") => Some("addition-only".into()),
        Some("leading-only") => None,
        _ => model.tool_update,
    };
    if !facts.input_modalities.iter().any(|value| value == "image") {
        model.image_pixel_budget = None::<ImagePixelBudget>;
        model.image_max_bytes = None;
    }
    model
}

pub(crate) fn resolve_listed_catalog(
    listing: Option<&ModelListing>,
    supported: &[DeepSeekCatalogModel],
    route_max_tokens: u64,
) -> ListedCatalog {
    let supported_by_id: HashMap<_, _> = supported
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect();
    let Some(listing) = listing else {
        return ListedCatalog {
            offered: supported.to_vec(),
            resolution: supported.to_vec(),
            refusals: HashMap::new(),
            badges: HashMap::new(),
            problems: vec![],
        };
    };
    let current = Version::parse(APP_IDENTITY.version).expect("package version is semantic");
    let mut offered = Vec::new();
    let mut refusals = HashMap::new();
    let mut badges = HashMap::new();
    let mut problems = Vec::new();
    for entry in &listing.entries {
        let refusal = match &entry.requirement {
            Requirement::None => (!supported_by_id.contains_key(entry.id.as_str()))
                .then_some(CatalogRefusal::Unsupported),
            Requirement::MinVersion(required) => match Version::parse(required) {
                Ok(required) if current >= required => None,
                Ok(_) => Some(CatalogRefusal::UpgradeRequired(required.clone())),
                Err(_) => {
                    problems.push(format!(
                        "model \"{}\" declares an unreadable client requirement",
                        entry.id
                    ));
                    Some(CatalogRefusal::Unsupported)
                }
            },
            Requirement::Invalid => {
                problems.push(format!(
                    "model \"{}\" declares an unreadable client requirement",
                    entry.id
                ));
                Some(CatalogRefusal::Unsupported)
            }
        };
        if let Some(refusal) = refusal {
            refusals.insert(entry.id.clone(), refusal);
            continue;
        }
        let ListedCapabilities::Usable(facts) = &entry.capabilities else {
            let ListedCapabilities::Unusable(reason) = &entry.capabilities else {
                unreachable!()
            };
            problems.push(format!("model \"{}\" {reason}", entry.id));
            refusals.insert(entry.id.clone(), CatalogRefusal::Unsupported);
            continue;
        };
        let base = supported_by_id
            .get(entry.id.as_str())
            .copied()
            .cloned()
            .unwrap_or_else(|| DeepSeekCatalogModel {
                id: entry.id.clone(),
                name: None,
                description: None,
                context_window: None,
                max_tokens: None,
                input_modalities: None,
                image_pixel_budget: None,
                image_max_bytes: None,
                system_prompt_update: None,
                tool_update: None,
            });
        offered.push(layered(&base, entry, facts, route_max_tokens));
        match &entry.badge {
            Some(Ok(badge)) => {
                badges.insert(entry.id.clone(), badge.clone());
            }
            Some(Err(reason)) => {
                problems.push(format!("model \"{}\" shows no badge: {reason}", entry.id))
            }
            None => {}
        }
    }
    let mut resolution = offered.clone();
    for model in supported {
        if !resolution.iter().any(|candidate| candidate.id == model.id) {
            resolution.push(model.clone());
        }
    }
    ListedCatalog {
        offered,
        resolution,
        refusals,
        badges,
        problems,
    }
}

pub(crate) fn refusal_error(model: &str, refusal: &CatalogRefusal) -> HarnessError {
    match refusal {
        CatalogRefusal::UpgradeRequired(required) => LlmError::new(
            format!("DeepSeek model \"{model}\" requires DeepSeek Harness {required} or later; current version is {}", APP_IDENTITY.version),
            BuiltinLlmFailureCode::ClientUpgradeRequired,
        ),
        CatalogRefusal::Unsupported => LlmError::new(
            format!("DeepSeek model \"{model}\" is not supported by DeepSeek Harness; current version is {}", APP_IDENTITY.version),
            BuiltinLlmFailureCode::ModelUnsupported,
        ),
    }.expect("valid LLM error")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_models;
    use hoh_error::HarnessError;
    use serde_json::json;

    fn messages() -> Value {
        json!({"output_modalities":["text"],"api_capabilities":{"anthropic_messages":{}}})
    }

    #[test]
    fn parses_listing_facts_and_skips_bad_and_duplicate_rows() {
        let mut flash = messages();
        let object = flash.as_object_mut().unwrap();
        object.insert("id".into(), json!("deepseek-flash"));
        object.insert("name".into(), json!("Listed Flash"));
        object.insert("context_window".into(), json!(1_048_576));
        object.insert("max_output_tokens".into(), json!(393_216));
        object.insert("input_modalities".into(), json!(["image", "text"]));
        let listing =
            parse_model_listing(&json!({"data":[null, flash, {"id":"deepseek-flash"}]})).unwrap();
        assert_eq!(listing.skipped, 2);
        assert_eq!(listing.entries[0].name.as_deref(), Some("Listed Flash"));
        let ListedCapabilities::Usable(facts) = &listing.entries[0].capabilities else {
            panic!("usable")
        };
        assert_eq!(facts.context_window, Some(1_048_576));
        assert_eq!(facts.max_output_tokens, Some(393_216));
        assert_eq!(facts.input_modalities, ["image", "text"]);
    }

    #[test]
    fn rejects_a_body_without_any_model_entry() {
        let error = parse_model_listing(&json!({"data":[null, {"id":""}]})).unwrap_err();
        assert_eq!(error.code(), "INVALID_RESPONSE");
        assert_eq!(
            error.message(),
            "DeepSeek model listing contains no model entry"
        );
    }

    #[test]
    fn resolves_offered_badges_and_all_three_refusal_paths() {
        let badge = json!({"text":"New","tone":"info"});
        let body = json!({"data":[
            {"id":"deepseek-flash","client_metadata":{"deepseek-harness":{"badge":badge}},"output_modalities":["text"],"api_capabilities":{"anthropic_messages":{"system_prompt_update":"in-history"}}},
            {"id":"deepseek-v4-pro"},
            {"id":"deepseek-v5","output_modalities":["text"],"api_capabilities":{"anthropic_messages":{}}},
            {"id":"deepseek-v5-next","client_requirements":{"deepseek-harness":{"min_version":"99.0.0-alpha.1"}},"output_modalities":["text"],"api_capabilities":{"anthropic_messages":{}}}
        ]});
        let listing = parse_model_listing(&body).unwrap();
        let catalog = resolve_listed_catalog(Some(&listing), &default_models(), 256_000);
        assert_eq!(
            catalog
                .offered
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["deepseek-flash"]
        );
        assert!(catalog.badges.contains_key("deepseek-flash"));
        assert_eq!(
            catalog.refusals["deepseek-v4-pro"],
            CatalogRefusal::Unsupported
        );
        assert_eq!(catalog.refusals["deepseek-v5"], CatalogRefusal::Unsupported);
        assert!(matches!(
            catalog.refusals["deepseek-v5-next"],
            CatalogRefusal::UpgradeRequired(_)
        ));
        assert_eq!(
            refusal_error("deepseek-v4-pro", &catalog.refusals["deepseek-v4-pro"]).code(),
            "MODEL_UNSUPPORTED"
        );
        assert_eq!(
            refusal_error("deepseek-v5-next", &catalog.refusals["deepseek-v5-next"]).code(),
            "CLIENT_UPGRADE_REQUIRED"
        );
    }

    #[test]
    fn a_compatible_version_requirement_offers_a_new_model() {
        let listing = parse_model_listing(&json!({"data":[{
            "id":"deepseek-v5",
            "client_requirements":{"deepseek-harness":{"min_version":"0.0.1"}},
            "output_modalities":["text"],
            "api_capabilities":{"anthropic_messages":{}}
        }]}))
        .unwrap();
        let catalog = resolve_listed_catalog(Some(&listing), &default_models(), 256_000);
        assert_eq!(catalog.offered[0].id, "deepseek-v5");
        assert_eq!(catalog.resolution.len(), 3);
    }

    #[test]
    fn listing_request_sends_x_api_key_to_the_exact_url() {
        let client = reqwest::Client::new();
        let request = model_listing_request(
            &client,
            "https://api.deepseek.com/models",
            "fixture-api-key",
        )
        .unwrap();
        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(request.url().as_str(), "https://api.deepseek.com/models");
        assert_eq!(request.headers()["x-api-key"], "fixture-api-key");
        assert_eq!(request.headers()["accept"], "application/json");
    }

    #[test]
    fn malformed_http_result_is_an_llm_error() {
        let error = parse_model_listing(&json!({})).unwrap_err();
        assert_eq!(HarnessError::of(&error).unwrap().code(), "INVALID_RESPONSE");
    }

    #[test]
    fn badge_rejects_unreadable_characters_before_trimming() {
        let listing = parse_model_listing(&json!({"data":[{
            "id":"deepseek-flash",
            "client_metadata":{"deepseek-harness":{"badge":{"text":"New\n"}}},
            "output_modalities":["text"],
            "api_capabilities":{"anthropic_messages":{}}
        }]}))
        .unwrap();
        assert!(matches!(listing.entries[0].badge, Some(Err(_))));
    }

    #[test]
    fn non_object_client_metadata_becomes_a_catalog_problem() {
        for metadata in [json!("x"), json!([])] {
            let listing = parse_model_listing(&json!({"data":[{
                "id":"deepseek-flash",
                "client_metadata":metadata,
                "output_modalities":["text"],
                "api_capabilities":{"anthropic_messages":{}}
            }]}))
            .unwrap();
            let catalog = resolve_listed_catalog(Some(&listing), &default_models(), 256_000);
            assert_eq!(
                catalog.problems,
                ["model \"deepseek-flash\" shows no badge: client_metadata is not an object"]
            );
        }
        for metadata in [Value::Null, json!({})] {
            let listing = parse_model_listing(&json!({"data":[{
                "id":"deepseek-flash",
                "client_metadata":metadata,
                "output_modalities":["text"],
                "api_capabilities":{"anthropic_messages":{}}
            }]}))
            .unwrap();
            assert!(listing.entries[0].badge.is_none());
        }
    }

    #[test]
    fn absent_and_null_badges_mean_no_badge_without_a_catalog_problem() {
        for own in [json!({}), json!({"badge":null})] {
            let listing = parse_model_listing(&json!({"data":[{
                "id":"deepseek-flash",
                "client_metadata":{"deepseek-harness":own},
                "output_modalities":["text"],
                "api_capabilities":{"anthropic_messages":{}}
            }]}))
            .unwrap();
            assert!(listing.entries[0].badge.is_none());
            let catalog = resolve_listed_catalog(Some(&listing), &default_models(), 256_000);
            assert!(catalog.problems.is_empty());
            assert!(catalog.badges.is_empty());
        }
    }

    #[test]
    fn non_null_update_modes_fall_back_to_leading_only() {
        let listing = parse_model_listing(&json!({"data":[{
            "id":"deepseek-flash",
            "output_modalities":["text"],
            "api_capabilities":{"anthropic_messages":{
                "system_prompt_update":5,
                "tool_update":5
            }}
        }]}))
        .unwrap();
        let ListedCapabilities::Usable(facts) = &listing.entries[0].capabilities else {
            panic!("usable")
        };
        assert_eq!(facts.system_prompt_update.as_deref(), Some("leading-only"));
        assert_eq!(facts.tool_update.as_deref(), Some("leading-only"));
    }

    #[test]
    fn capacities_follow_javascript_safe_integer_rules() {
        for (value, expected) in [
            (json!(1.0), Ok(Some(1))),
            (json!(1.5), Err(())),
            (json!(0), Err(())),
            (json!(-1), Err(())),
            (json!(9_007_199_254_740_992_u64), Err(())),
        ] {
            assert_eq!(positive_capacity(Some(&value)), expected, "{value}");
        }
    }

    #[test]
    fn fetch_redirect_error_statuses_are_transport_failures() {
        let error = redirect_error(
            "https://api.deepseek.com/models",
            reqwest::StatusCode::FOUND,
        )
        .unwrap();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().code(),
            "TRANSPORT"
        );
        assert_eq!(
            error.to_string(),
            "could not reach https://api.deepseek.com/models"
        );
        assert!(
            redirect_error(
                "https://api.deepseek.com/models",
                reqwest::StatusCode::NOT_MODIFIED
            )
            .is_none()
        );
    }
}
