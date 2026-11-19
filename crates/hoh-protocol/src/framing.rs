//! JSON-RPC values and lossless typed envelopes, independent of streams and stdio.
use crate::{Notification, Request};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum JsonRpcId {
    String(String),
    Number(#[schemars(with = "f64")] serde_json::Number),
}
impl JsonRpcId {
    pub fn key(&self) -> String {
        match self {
            Self::String(id) => format!("s:{id}"),
            Self::Number(id) => {
                let number = id.as_f64().expect("finite JSON number");
                format!("n:{}", if number == 0.0 { 0.0 } else { number })
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Version {
    #[serde(rename = "2.0")]
    V2,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RequestFrame {
    pub jsonrpc: Version,
    pub id: JsonRpcId,
    #[serde(flatten)]
    pub request: Request,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationFrame {
    pub jsonrpc: Version,
    #[serde(flatten)]
    pub notification: Notification,
}
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RpcError {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::present"
    )]
    #[schemars(required)]
    pub code: Option<i64>,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::present"
    )]
    #[schemars(required)]
    pub data: Option<Value>,
}
impl std::fmt::Debug for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcError")
            .field("code", &self.code)
            .field("message", &self.message)
            .field("data", &"[REDACTED]")
            .finish()
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ResponseFrame {
    Success {
        jsonrpc: Version,
        id: JsonRpcId,
        result: Value,
    },
    Error {
        jsonrpc: Version,
        id: JsonRpcId,
        error: RpcError,
    },
}
impl std::fmt::Debug for ResponseFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success { jsonrpc, id, .. } => f
                .debug_struct("Success")
                .field("jsonrpc", jsonrpc)
                .field("id", id)
                .field("result", &"[REDACTED]")
                .finish(),
            Self::Error { jsonrpc, id, error } => f
                .debug_struct("Error")
                .field("jsonrpc", jsonrpc)
                .field("id", id)
                .field("error", error)
                .finish(),
        }
    }
}
/// Typed admission with exact incoming object order, including extension data.
/// Formatting is compact JSON; fixture files use this canonical encoding plus LF.
#[derive(Clone, PartialEq)]
pub struct Wire<T> {
    typed: T,
    value: Value,
}
impl<T: std::fmt::Debug> std::fmt::Debug for Wire<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wire")
            .field("typed", &self.typed)
            .finish_non_exhaustive()
    }
}
impl<T> Wire<T> {
    pub fn typed(&self) -> &T {
        &self.typed
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
}
impl<T: Serialize> Serialize for Wire<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(s)
    }
}
impl<'de, T: DeserializeOwned> Deserialize<'de> for Wire<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        let typed = serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
        Ok(Self { typed, value })
    }
}
pub fn request(id: Value, method: &str, params: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}
pub fn notification(method: &str, params: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params })
}
pub fn success(id: Value, result: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}
pub fn error(id: Value, code: i32, message: impl Into<String>) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}
