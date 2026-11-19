//! Secret-bearing strings serialize on the native wire but never through Debug.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct SecretString(String);
impl SecretString {
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}
impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
