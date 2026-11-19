use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeepSeekFileId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeepSeekFileScope(String);

impl DeepSeekFileId {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty()).then_some(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for DeepSeekFileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl DeepSeekFileScope {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
        .then_some(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for DeepSeekFileScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
