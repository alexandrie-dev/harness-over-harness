//! MCP client bridge for one external server.
//!
//! Wire names remain separate from model-facing names. Each connection
//! generation publishes a complete tool set atomically and owns its cleanup.

pub mod connection;
pub mod credentials;
pub mod deadline;
pub mod diagnostic;
pub mod generation;
mod output_schema;
pub mod probe;
mod runtime;
pub mod server_context;
pub mod status;
pub mod templates;
pub mod tools;
pub mod transport;
pub mod types;

pub use connection::{
    DEFAULT_MAX_INSTRUCTION_BYTES, RECONNECT_DEFAULTS, ReconnectConfig, ResolvedReconnectPolicy,
    resolve_reconnect_policy,
};
pub use diagnostic::{McpLocalError, McpLocalErrorCode, McpToolError, McpToolErrorCode};
pub use probe::{McpProbeOptions, probe_server};
pub use runtime::{McpClient, connection_status_of};
pub use status::{McpConnectionStatusHandle, McpStatusCell};
pub use templates::assert_template_syntax;
pub use tools::{
    McpResult, McpToolDefinitionOptions, create_mcp_tool_definition, public_tool_name,
};
pub use types::*;

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const NAME: &str = "mcp-client";
pub const DEFAULT_TOOL_CALL_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_CLOSE_TIMEOUT_MS: u64 = 5_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "transport", rename_all = "kebab-case")]
pub enum Config {
    Stdio {
        #[serde(rename = "serverName")]
        server_name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: IndexMap<String, String>,
        #[serde(default)]
        cwd: String,
        #[serde(rename = "toolCallTimeoutMs", default = "default_tool_timeout")]
        tool_call_timeout_ms: u64,
        #[serde(rename = "startupTimeoutMs", default)]
        startup_timeout_ms: Option<u64>,
        #[serde(rename = "closeTimeoutMs", default = "default_close_timeout")]
        close_timeout_ms: u64,
        #[serde(rename = "expandTemplates", default)]
        expand_templates: bool,
        #[serde(rename = "failOnStartupError", default)]
        fail_on_startup_error: bool,
        #[serde(rename = "maxInstructionBytes", default)]
        max_instruction_bytes: Option<usize>,
        #[serde(default)]
        reconnect: Option<ReconnectConfig>,
    },
    StreamableHttp {
        #[serde(rename = "serverName")]
        server_name: String,
        url: String,
        #[serde(default)]
        headers: IndexMap<String, String>,
        #[serde(rename = "toolCallTimeoutMs", default = "default_tool_timeout")]
        tool_call_timeout_ms: u64,
        #[serde(rename = "startupTimeoutMs", default)]
        startup_timeout_ms: Option<u64>,
        #[serde(rename = "closeTimeoutMs", default = "default_close_timeout")]
        close_timeout_ms: u64,
        #[serde(rename = "expandTemplates", default)]
        expand_templates: bool,
        #[serde(rename = "failOnStartupError", default)]
        fail_on_startup_error: bool,
        #[serde(rename = "maxInstructionBytes", default)]
        max_instruction_bytes: Option<usize>,
        #[serde(default)]
        reconnect: Option<ReconnectConfig>,
    },
}

const fn default_tool_timeout() -> u64 {
    DEFAULT_TOOL_CALL_TIMEOUT_MS
}
const fn default_close_timeout() -> u64 {
    DEFAULT_CLOSE_TIMEOUT_MS
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let max = hoh_timeout::MAX_TIMER_DELAY_MS;
        if self
            .startup_timeout_ms()
            .is_some_and(|value| value == 0 || value > max)
        {
            return Err(format!(
                "mcp-client({}): startupTimeoutMs must be a positive finite number no greater than {max}",
                self.server_name()
            ));
        }
        if self.close_timeout_ms() == 0 || self.close_timeout_ms() > max {
            return Err(format!(
                "mcp-client({}): closeTimeoutMs must be a positive finite number no greater than {max}",
                self.server_name()
            ));
        }
        if self.max_instruction_bytes() == 0 {
            return Err(format!(
                "mcp-client({}): maxInstructionBytes must be a positive integer",
                self.server_name()
            ));
        }
        Ok(())
    }
    pub fn server_name(&self) -> &str {
        match self {
            Self::Stdio { server_name, .. } | Self::StreamableHttp { server_name, .. } => {
                server_name
            }
        }
    }
    pub fn tool_call_timeout_ms(&self) -> u64 {
        match self {
            Self::Stdio {
                tool_call_timeout_ms,
                ..
            }
            | Self::StreamableHttp {
                tool_call_timeout_ms,
                ..
            } => *tool_call_timeout_ms,
        }
    }
    pub fn startup_timeout_ms(&self) -> Option<u64> {
        match self {
            Self::Stdio {
                startup_timeout_ms, ..
            }
            | Self::StreamableHttp {
                startup_timeout_ms, ..
            } => *startup_timeout_ms,
        }
    }
    pub fn close_timeout_ms(&self) -> u64 {
        match self {
            Self::Stdio {
                close_timeout_ms, ..
            }
            | Self::StreamableHttp {
                close_timeout_ms, ..
            } => *close_timeout_ms,
        }
    }
    pub fn expand_templates(&self) -> bool {
        match self {
            Self::Stdio {
                expand_templates, ..
            }
            | Self::StreamableHttp {
                expand_templates, ..
            } => *expand_templates,
        }
    }
    pub fn fail_on_startup_error(&self) -> bool {
        match self {
            Self::Stdio {
                fail_on_startup_error,
                ..
            }
            | Self::StreamableHttp {
                fail_on_startup_error,
                ..
            } => *fail_on_startup_error,
        }
    }
    pub fn max_instruction_bytes(&self) -> usize {
        match self {
            Self::Stdio {
                max_instruction_bytes,
                ..
            }
            | Self::StreamableHttp {
                max_instruction_bytes,
                ..
            } => max_instruction_bytes.unwrap_or(DEFAULT_MAX_INSTRUCTION_BYTES),
        }
    }
    pub fn reconnect(&self) -> Option<&ReconnectConfig> {
        match self {
            Self::Stdio { reconnect, .. } | Self::StreamableHttp { reconnect, .. } => {
                reconnect.as_ref()
            }
        }
    }
}

pub fn valid_server_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
