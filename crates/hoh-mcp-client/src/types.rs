use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCloseResult {
    pub close_confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum McpConnectionStatus {
    Closed {
        result: McpCloseResult,
    },
    WaitingCredentials {
        #[serde(rename = "lastError")]
        last_error: String,
    },
    Connecting {
        attempt: u64,
    },
    Connected {
        #[serde(rename = "toolCount")]
        tool_count: usize,
        #[serde(rename = "connectedAt")]
        connected_at: u64,
    },
    Reconnecting {
        attempt: u64,
        #[serde(rename = "maxAttempts")]
        max_attempts: u64,
        #[serde(rename = "nextRetryAtMs")]
        next_retry_at_ms: u64,
        #[serde(rename = "lastError")]
        last_error: String,
    },
    Failed {
        reason: McpFailureReason,
        #[serde(rename = "lastError", default, skip_serializing_if = "Option::is_none")]
        last_error: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpFailureReason {
    GaveUp,
    ReconnectDisabled,
    UnclosedGeneration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpProbeStage {
    Connect,
    List,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpProbeOutcome {
    Success {
        server_info: Option<McpServerInfo>,
        tool_names: Vec<String>,
        truncated: bool,
        duration_ms: u64,
    },
    Failure {
        stage: McpProbeStage,
        message: String,
        duration_ms: u64,
    },
}

impl Serialize for McpProbeOutcome {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        match self {
            Self::Success {
                server_info,
                tool_names,
                truncated,
                duration_ms,
            } => {
                map.serialize_entry("ok", &true)?;
                if let Some(server_info) = server_info {
                    map.serialize_entry("serverInfo", server_info)?;
                }
                map.serialize_entry("toolNames", tool_names)?;
                map.serialize_entry("truncated", truncated)?;
                map.serialize_entry("durationMs", duration_ms)?;
            }
            Self::Failure {
                stage,
                message,
                duration_ms,
            } => {
                map.serialize_entry("ok", &false)?;
                map.serialize_entry("stage", stage)?;
                map.serialize_entry("message", message)?;
                map.serialize_entry("durationMs", duration_ms)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for McpProbeOutcome {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| serde::de::Error::custom("probe outcome must be an object"))?;
        let ok = object
            .remove("ok")
            .and_then(|value| value.as_bool())
            .ok_or_else(|| serde::de::Error::custom("probe outcome requires boolean ok"))?;
        if ok {
            let server_info = object
                .remove("serverInfo")
                .map(serde_json::from_value)
                .transpose()
                .map_err(serde::de::Error::custom)?;
            let tool_names = serde_json::from_value(
                object
                    .remove("toolNames")
                    .ok_or_else(|| serde::de::Error::missing_field("toolNames"))?,
            )
            .map_err(serde::de::Error::custom)?;
            let truncated = object
                .remove("truncated")
                .and_then(|value| value.as_bool())
                .ok_or_else(|| serde::de::Error::missing_field("truncated"))?;
            let duration_ms = object
                .remove("durationMs")
                .and_then(|value| value.as_u64())
                .ok_or_else(|| serde::de::Error::missing_field("durationMs"))?;
            Ok(Self::Success {
                server_info,
                tool_names,
                truncated,
                duration_ms,
            })
        } else {
            let stage = serde_json::from_value(
                object
                    .remove("stage")
                    .ok_or_else(|| serde::de::Error::missing_field("stage"))?,
            )
            .map_err(serde::de::Error::custom)?;
            let message = object
                .remove("message")
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| serde::de::Error::missing_field("message"))?;
            let duration_ms = object
                .remove("durationMs")
                .and_then(|value| value.as_u64())
                .ok_or_else(|| serde::de::Error::missing_field("durationMs"))?;
            Ok(Self::Failure {
                stage,
                message,
                duration_ms,
            })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProbeResult {
    #[serde(flatten)]
    pub outcome: McpProbeOutcome,
    pub close_confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<String>,
}
