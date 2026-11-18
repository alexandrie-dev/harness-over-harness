use hoh_error::HarnessError;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiKeyRejection {
    Empty,
    IllegalCharacters,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ApiKeyCheck {
    Ok { value: String },
    Rejected { reason: ApiKeyRejection },
}
impl Serialize for ApiKeyCheck {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut result = serializer.serialize_struct("ApiKeyCheck", 2)?;
        match self {
            Self::Ok { value } => {
                result.serialize_field("ok", &true)?;
                result.serialize_field("value", value)?;
            }
            Self::Rejected { reason } => {
                result.serialize_field("ok", &false)?;
                result.serialize_field("reason", reason)?;
            }
        }
        result.end()
    }
}
impl<'de> Deserialize<'de> for ApiKeyCheck {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value.get("ok").and_then(serde_json::Value::as_bool) {
            Some(true) => value
                .get("value")
                .and_then(serde_json::Value::as_str)
                .map(|value| Self::Ok {
                    value: value.into(),
                })
                .ok_or_else(|| serde::de::Error::custom("API key check value must be a string")),
            Some(false) => serde_json::from_value(value.get("reason").cloned().unwrap_or_default())
                .map(|reason| Self::Rejected { reason })
                .map_err(serde::de::Error::custom),
            None => Err(serde::de::Error::custom(
                "API key check ok must be a boolean",
            )),
        }
    }
}
pub fn normalize_api_key(raw: &str) -> ApiKeyCheck {
    let value = hoh_util_values::js_trim(raw);
    if value.is_empty() {
        return ApiKeyCheck::Rejected {
            reason: ApiKeyRejection::Empty,
        };
    }
    if !value.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return ApiKeyCheck::Rejected {
            reason: ApiKeyRejection::IllegalCharacters,
        };
    }
    ApiKeyCheck::Ok {
        value: value.into(),
    }
}
pub fn assert_usable_api_key(
    raw: &str,
    pkg: &str,
    reference: &str,
) -> Result<String, HarnessError> {
    match normalize_api_key(raw) {
        ApiKeyCheck::Ok { value } => Ok(value),
        ApiKeyCheck::Rejected { reason } => Err(crate::error::llm_error(
            match reason {
                ApiKeyRejection::Empty => format!(
                    "{pkg}: the API key resolved from {reference} is blank; set {reference} to the raw key (the web Models page writes it) or export it in the launching environment"
                ),
                ApiKeyRejection::IllegalCharacters => format!(
                    "{pkg}: the API key resolved from {reference} contains characters no HTTP header can carry; set {reference} to the raw key alone (the web Models page writes it)"
                ),
            },
            crate::error::BuiltinLlmFailureCode::InvalidCredential,
        )),
    }
}
