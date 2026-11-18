use crate::{EpochHeader, SessionEvent};
use hoh_util_values::{deep_equal_json, js_json_stringify};
use serde_json::{Map, Value};
use std::sync::Arc;

pub fn canonical_header(header: &EpochHeader) -> EpochHeader {
    let mut result = Map::new();
    if let Some(config) = header.get("config") {
        result.insert("config".into(), config.clone());
    }
    let defaults = &header["adapterDefaults"];
    if defaults["reasoningEffort"] == true || defaults["maxTokens"] == true {
        result.insert("adapterDefaults".into(), defaults.clone());
    }
    if header["tools"].as_array().is_some_and(|v| !v.is_empty()) {
        result.insert("tools".into(), header["tools"].clone());
    }
    Value::Object(result)
}
pub fn header_equals(a: &EpochHeader, b: &EpochHeader) -> bool {
    if let (Ok(ac), Ok(bc)) = (
        serde_json::from_value::<hoh_llm::LlmCallConfig>(a["config"].clone()),
        serde_json::from_value::<hoh_llm::LlmCallConfig>(b["config"].clone()),
    ) && !hoh_llm::call_config_equals(&ac, &bc)
    {
        return false;
    }
    // callConfigEquals compares only these source-owned fields, not extensions.
    for key in [
        "provider",
        "model",
        "reasoningEffort",
        "temperature",
        "maxTokens",
        "stop",
    ] {
        if !optional_equal(a["config"].get(key), b["config"].get(key)) {
            return false;
        }
    }
    for key in ["reasoningEffort", "maxTokens"] {
        if !optional_equal(a["adapterDefaults"].get(key), b["adapterDefaults"].get(key)) {
            return false;
        }
    }
    let empty = vec![];
    let at = a["tools"].as_array().unwrap_or(&empty);
    let bt = b["tools"].as_array().unwrap_or(&empty);
    at.len() == bt.len()
        && at
            .iter()
            .zip(bt)
            .all(|(a, b)| js_json_stringify(a) == js_json_stringify(b))
}
fn optional_equal(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => deep_equal_json(a, b),
        (None, None) => true,
        _ => false,
    }
}
pub fn fold_request_header(
    events: &[Arc<SessionEvent>],
    from: Option<Arc<EpochHeader>>,
) -> Option<Arc<EpochHeader>> {
    let mut state = from;
    for event in events {
        if event.event_type() == "request/header" {
            state = Some(Arc::new(canonical_header(&event.data()["header"])));
        }
    }
    state
}
