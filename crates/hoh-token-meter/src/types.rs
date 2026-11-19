use hoh_session::{SessionLogOffset, SessionSeq};
use indexmap::IndexMap;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The fixed estimator has no settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TokenMeterConfig(IndexMap<String, Value>);

impl TokenMeterConfig {
    pub(crate) fn unknown_key(&self) -> Option<&str> {
        self.0
            .keys()
            .min_by_key(|key| javascript_key_order(key))
            .map(String::as_str)
    }
}

fn javascript_key_order(key: &str) -> (u8, u32) {
    match key.parse::<u32>() {
        Ok(index) if index != u32::MAX && index.to_string() == key => (0, index),
        _ => (1, 0),
    }
}

impl JsonSchema for TokenMeterConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TokenMeterConfig".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::try_from(json!({ "type": "object" })).unwrap_or_else(|_| Schema::from(true))
    }
}

/// The baseline from which a signed surface delta produces current pressure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TokenMeasurementBaseline {
    None {
        #[serde(serialize_with = "crate::measurement_projection::serialize_js_number")]
        tokens: f64,
    },
    Estimated {
        #[serde(serialize_with = "crate::measurement_projection::serialize_js_number")]
        tokens: f64,
    },
    Usage {
        #[serde(serialize_with = "crate::measurement_projection::serialize_js_number")]
        tokens: f64,
        usage: Value,
    },
}

impl TokenMeasurementBaseline {
    pub fn tokens(&self) -> f64 {
        match self {
            Self::None { tokens } | Self::Estimated { tokens } | Self::Usage { tokens, .. } => {
                *tokens
            }
        }
    }
}

/// Detached request-pressure and surface snapshot at one consumed log revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenMeasurement {
    pub log_revision: SessionLogOffset,
    pub baseline: TokenMeasurementBaseline,
    #[serde(serialize_with = "crate::measurement_projection::serialize_js_number")]
    pub surface_delta_tokens: f64,
    #[serde(serialize_with = "crate::measurement_projection::serialize_js_number")]
    pub total_tokens: f64,
    pub surface_tokens: u64,
    pub nodes: Vec<TokenSurfaceNode>,
}

/// One token-priced node in the current ordered session surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSurfaceNode {
    pub seq: SessionSeq,
    pub tokens: u64,
    pub heuristic_tokens: u64,
}

/// Durable cumulative provider usage for a complete session log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageProjection {
    pub uncached_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

/// Approximate context occupancy for a status display.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPressureProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// Fixed-heuristic composition of the next request's context.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBreakdownProjection {
    pub system_tokens: u64,
    pub tools_tokens: u64,
    pub message_tokens: u64,
}
