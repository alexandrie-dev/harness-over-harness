//! Version-gated catalog assembled from `/models` and local knowledge.

use crate::config::resolve_adapter_options;
use crate::listing::{ClientRequirement, ListedBadge, ListedFacts, ListingEntry};
use crate::types::{DeepSeekCatalogModel, Options};
use hoh_error::HarnessError;
use hoh_llm::{APP_IDENTITY, BuiltinLlmFailureCode as Code, LlmError, LlmModelBadge};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

pub type ModelOverride = DeepSeekCatalogModel;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogRefusal {
    UpgradeRequired { required_version: String },
    Unsupported,
}

pub struct ListedCatalogInput<'a> {
    pub listing: Option<&'a [ListingEntry]>,
    pub supported: &'a [DeepSeekCatalogModel],
    pub overrides: &'a IndexMap<String, ModelOverride>,
    pub route_max_tokens: u64,
    pub client_version: &'a str,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListedCatalog {
    pub offered: Vec<DeepSeekCatalogModel>,
    pub resolution: Vec<DeepSeekCatalogModel>,
    pub refusals: IndexMap<String, CatalogRefusal>,
    pub badges: IndexMap<String, LlmModelBadge>,
    pub problems: Vec<String>,
    pub override_errors: Vec<String>,
}

fn layer(model: &DeepSeekCatalogModel, overlay: &DeepSeekCatalogModel) -> DeepSeekCatalogModel {
    let mut merged = model.clone();
    if overlay.name.is_some() {
        merged.name.clone_from(&overlay.name);
    }
    if overlay.description.is_some() {
        merged.description.clone_from(&overlay.description);
    }
    if overlay.context_window.is_some() {
        merged.context_window = overlay.context_window;
    }
    if overlay.max_tokens.is_some() {
        merged.max_tokens = overlay.max_tokens;
    }
    if overlay.input_modalities.is_some() {
        merged
            .input_modalities
            .clone_from(&overlay.input_modalities);
    }
    if overlay.image_pixel_budget.is_some() {
        merged
            .image_pixel_budget
            .clone_from(&overlay.image_pixel_budget);
    }
    if overlay.image_max_bytes.is_some() {
        merged.image_max_bytes = overlay.image_max_bytes;
    }
    if overlay.system_prompt_update.is_some() {
        merged
            .system_prompt_update
            .clone_from(&overlay.system_prompt_update);
    }
    if overlay.tool_update.is_some() {
        merged.tool_update.clone_from(&overlay.tool_update);
    }
    if merged
        .input_modalities
        .as_ref()
        .is_none_or(|items| !items.iter().any(|item| item == "image"))
    {
        if overlay.image_pixel_budget.is_none() {
            merged.image_pixel_budget = None;
        }
        if overlay.image_max_bytes.is_none() {
            merged.image_max_bytes = None;
        }
    }
    merged
}

fn listed(
    base: &DeepSeekCatalogModel,
    entry: &ListingEntry,
    facts: &ListedFacts,
    route_max_tokens: u64,
) -> DeepSeekCatalogModel {
    let mut model = base.clone();
    if let Some(name) = &entry.name {
        model.name = Some(name.clone());
    }
    if let Some(value) = facts.context_window {
        model.context_window = Some(value);
    }
    if let Some(value) = facts.max_output_tokens {
        model.max_tokens = Some(route_max_tokens.min(value));
    }
    model.input_modalities = Some(facts.input_modalities.clone());
    match facts.system_prompt_update.as_deref() {
        Some("in-history") => model.system_prompt_update = Some("in-history".into()),
        Some("leading-only") => model.system_prompt_update = None,
        _ => {}
    }
    match facts.tool_update.as_deref() {
        Some("in-history") => model.tool_update = Some("in-history".into()),
        Some("addition-only") => model.tool_update = Some("addition-only".into()),
        Some("leading-only") => model.tool_update = None,
        _ => {}
    }
    if !facts.input_modalities.iter().any(|item| item == "image") {
        model.image_pixel_budget = None;
        model.image_max_bytes = None;
    } else if model.image_max_bytes.is_none() {
        model.image_max_bytes = Some(crate::request_pricing::DEFAULT_REQUEST_IMAGE_MAX_BYTES);
    }
    model
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Pre>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pre {
    Number(u64),
    Text(String),
}
impl Ord for Pre {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.cmp(b),
            (Self::Number(_), Self::Text(_)) => std::cmp::Ordering::Less,
            (Self::Text(_), Self::Number(_)) => std::cmp::Ordering::Greater,
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
        }
    }
}
impl PartialOrd for Pre {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
fn version(value: &str) -> Option<Version> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let (value, build) = value
        .split_once('+')
        .map_or((value, None), |(version, build)| (version, Some(build)));
    if build.is_some_and(|build| !valid_identifiers(build, false)) {
        return None;
    }
    let (core, pre) = value
        .split_once('-')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    let mut parts = core.split('.');
    let major = numeric_identifier(parts.next()?)?;
    let minor = numeric_identifier(parts.next()?)?;
    let patch = numeric_identifier(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    if pre.is_some_and(|pre| !valid_identifiers(pre, true)) {
        return None;
    }
    let pre = pre
        .map(|value| {
            value
                .split('.')
                .map(|item| {
                    numeric_identifier(item).map_or_else(|| Pre::Text(item.into()), Pre::Number)
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Version {
        major,
        minor,
        patch,
        pre,
    })
}

fn numeric_identifier(value: &str) -> Option<u64> {
    (!value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0')))
    .then(|| value.parse().ok())?
}

fn valid_identifiers(value: &str, reject_numeric_leading_zero: bool) -> bool {
    value.split('.').all(|identifier| {
        !identifier.is_empty()
            && identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && (!reject_numeric_leading_zero
                || !identifier.bytes().all(|byte| byte.is_ascii_digit())
                || identifier == "0"
                || !identifier.starts_with('0'))
    })
}
fn at_least(current: &Version, required: &Version) -> bool {
    let core = (current.major, current.minor, current.patch).cmp(&(
        required.major,
        required.minor,
        required.patch,
    ));
    if core != std::cmp::Ordering::Equal {
        return core == std::cmp::Ordering::Greater;
    }
    match (current.pre.is_empty(), required.pre.is_empty()) {
        (true, false) => true,
        (false, true) => false,
        _ => current.pre >= required.pre,
    }
}

fn gate(
    entry: &ListingEntry,
    known: bool,
    client_version: &str,
    problems: &mut Vec<String>,
) -> Option<CatalogRefusal> {
    match &entry.requirement {
        ClientRequirement::None => (!known).then_some(CatalogRefusal::Unsupported),
        ClientRequirement::MinVersion { version: required } => {
            let pair = version(client_version).zip(version(required));
            match pair {
                Some((current, required_version)) if at_least(&current, &required_version) => None,
                Some(_) => Some(CatalogRefusal::UpgradeRequired {
                    required_version: required.clone(),
                }),
                None => {
                    problems.push(format!(
                        "model \"{}\" declares an unreadable client requirement",
                        entry.id
                    ));
                    Some(CatalogRefusal::Unsupported)
                }
            }
        }
        ClientRequirement::Invalid => {
            problems.push(format!(
                "model \"{}\" declares an unreadable client requirement",
                entry.id
            ));
            Some(CatalogRefusal::Unsupported)
        }
    }
}

fn validated_override(
    model: DeepSeekCatalogModel,
    override_errors: &mut Vec<String>,
) -> DeepSeekCatalogModel {
    let options = Options {
        models: Some(vec![model.clone()]),
        ..Default::default()
    };
    match resolve_adapter_options(&options) {
        Ok(resolved) => resolved.models.into_iter().next().expect("one model"),
        Err(error) => {
            override_errors.push(error.to_string());
            model
        }
    }
}

pub fn resolve_listed_catalog(input: ListedCatalogInput<'_>) -> ListedCatalog {
    let supported: HashMap<_, _> = input
        .supported
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect();
    let mut result = ListedCatalog::default();
    let apply_override = |model: &DeepSeekCatalogModel, errors: &mut Vec<String>| {
        let Some(overlay) = input.overrides.get(&model.id) else {
            return model.clone();
        };
        let candidate = layer(model, overlay);
        let before = errors.len();
        let checked = validated_override(candidate, errors);
        if errors.len() != before {
            let detail = errors.pop().expect("new validation error");
            errors.push(format!(
                "modelOverrides.{} does not apply ({detail})",
                model.id
            ));
            model.clone()
        } else {
            checked
        }
    };
    let Some(listing) = input.listing else {
        result.resolution = input
            .supported
            .iter()
            .map(|model| apply_override(model, &mut result.override_errors))
            .collect();
        result.offered.clone_from(&result.resolution);
        return result;
    };
    for entry in listing {
        if let Some(refusal) = gate(
            entry,
            supported.contains_key(entry.id.as_str()),
            input.client_version,
            &mut result.problems,
        ) {
            result.refusals.insert(entry.id.clone(), refusal);
            continue;
        }
        let Some(facts) = entry.capabilities.facts() else {
            result.problems.push(format!(
                "model \"{}\" {}",
                entry.id,
                entry.capabilities.reason().unwrap_or_default()
            ));
            result
                .refusals
                .insert(entry.id.clone(), CatalogRefusal::Unsupported);
            continue;
        };
        let fallback = DeepSeekCatalogModel {
            id: entry.id.clone(),
            ..Default::default()
        };
        let model = listed(
            supported
                .get(entry.id.as_str())
                .copied()
                .unwrap_or(&fallback),
            entry,
            facts,
            input.route_max_tokens,
        );
        let model = apply_override(&model, &mut result.override_errors);
        if let Some(badge) = &entry.badge {
            match badge {
                ListedBadge::Badge { badge } => {
                    result.badges.insert(entry.id.clone(), badge.clone());
                }
                ListedBadge::Invalid { reason } => result
                    .problems
                    .push(format!("model \"{}\" shows no badge: {reason}", entry.id)),
            }
        }
        result.offered.push(model);
    }
    let offered: HashSet<_> = result
        .offered
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    let listed: HashSet<_> = listing.iter().map(|entry| entry.id.as_str()).collect();
    for id in input.overrides.keys() {
        if !listed.contains(id.as_str()) && !supported.contains_key(id.as_str()) {
            result.override_errors.push(format!("modelOverrides names \"{id}\", which neither the model listing nor the built-in catalog describes"));
        }
    }
    result.resolution = result.offered.clone();
    result.resolution.extend(
        input
            .supported
            .iter()
            .filter(|model| !offered.contains(model.id.as_str()))
            .map(|model| apply_override(model, &mut result.override_errors)),
    );
    result
}

pub fn refuse_listed_model(
    model: &str,
    refusal: Option<&CatalogRefusal>,
) -> Result<(), HarnessError> {
    match refusal {
        None => Ok(()),
        Some(CatalogRefusal::UpgradeRequired { required_version }) => Err(LlmError::new(
            format!("DeepSeek model \"{model}\" requires DeepSeek Harness {required_version} or later; current version is {}", APP_IDENTITY.version),
            Code::ClientUpgradeRequired,
        ).expect("refusal diagnostic is valid")),
        Some(CatalogRefusal::Unsupported) => Err(LlmError::new(
            format!("DeepSeek model \"{model}\" is not supported by DeepSeek Harness; current version is {}", APP_IDENTITY.version),
            Code::ModelUnsupported,
        ).expect("refusal diagnostic is valid")),
    }
}
