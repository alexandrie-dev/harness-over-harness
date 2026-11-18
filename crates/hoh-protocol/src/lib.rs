//! Transport-independent SDK/HoH messages. See PORT_NOTES.md for extension choices.
pub mod approval;
pub mod framing;
mod hoh;
mod registry;
mod sdk;
pub mod transport;
pub use hoh::*;
pub use registry::*;
pub use sdk::*;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Ordered arbitrary plugin-owned JSON; this is not an untyped method payload.
pub type JsonObject = serde_json::Map<String, Value>;

macro_rules! object {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct $name { $(pub $field: $ty),* }
    };
}
pub(crate) use object;
macro_rules! choices {
    ($name:ident { $($variant:ident => $value:literal),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        pub enum $name { $(#[serde(rename = $value)] $variant),* }
    };
}
pub(crate) use choices;

/// No optional field is serialized as null.
macro_rules! optional {
    ($name:ident { $($(#[$attr:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct $name { $($(#[$attr])* pub $field: $ty),* }
    };
}
pub(crate) use optional;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// Missing optional members use serde(default); explicit null is not a TS optional value.
pub(crate) fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
