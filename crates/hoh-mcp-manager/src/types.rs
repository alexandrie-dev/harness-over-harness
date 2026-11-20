use hoh_kernel::Parallel;
use hoh_mcp_client::{McpCloseResult, McpConnectionStatus};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpManagedServerStatus {
    Disabled,
    LoadFailed { last_error: String },
    CleanupFailed { result: McpCloseResult },
    Connection(McpConnectionStatus),
}

impl Serialize for McpManagedServerStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::Connection(status) => status.serialize(serializer),
            Self::Disabled => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("state", "disabled")?;
                map.end()
            }
            Self::LoadFailed { last_error } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("state", "failed")?;
                map.serialize_entry("reason", "load")?;
                map.serialize_entry("lastError", last_error)?;
                map.end()
            }
            Self::CleanupFailed { result } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("state", "failed")?;
                map.serialize_entry("reason", "cleanup")?;
                map.serialize_entry("result", result)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for McpManagedServerStatus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let state = value
            .get("state")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::missing_field("state"))?;
        match (
            state,
            value.get("reason").and_then(serde_json::Value::as_str),
        ) {
            ("disabled", _) => Ok(Self::Disabled),
            ("failed", Some("load")) => Ok(Self::LoadFailed {
                last_error: value
                    .get("lastError")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| serde::de::Error::missing_field("lastError"))?
                    .to_owned(),
            }),
            ("failed", Some("cleanup")) => Ok(Self::CleanupFailed {
                result: serde_json::from_value(
                    value
                        .get("result")
                        .cloned()
                        .ok_or_else(|| serde::de::Error::missing_field("result"))?,
                )
                .map_err(serde::de::Error::custom)?,
            }),
            _ => serde_json::from_value(value)
                .map(Self::Connection)
                .map_err(serde::de::Error::custom),
        }
    }
}

impl McpManagedServerStatus {
    pub fn state(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::LoadFailed { .. } | Self::CleanupFailed { .. } => "failed",
            Self::Connection(status) => match status {
                McpConnectionStatus::Closed { .. } => "closed",
                McpConnectionStatus::WaitingCredentials { .. } => "waiting-credentials",
                McpConnectionStatus::Connecting { .. } => "connecting",
                McpConnectionStatus::Connected { .. } => "connected",
                McpConnectionStatus::Reconnecting { .. } => "reconnecting",
                McpConnectionStatus::Failed { .. } => "failed",
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpManagedServerView {
    pub name: String,
    pub configured: bool,
    pub status: McpManagedServerStatus,
}

pub struct McpManagerStatus {
    pub name: String,
    pub status: McpManagedServerStatus,
}

hoh_kernel::event!(McpManagerStatus = "mcp-manager/status", Parallel);
