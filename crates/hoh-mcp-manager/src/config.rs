use hoh_error::BoxError;
use hoh_kernel::Volatile;
use hoh_mcp_client::{ReconnectConfig, assert_template_syntax, resolve_reconnect_policy};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const DEFAULT_STARTUP_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransport {
    Stdio,
    StreamableHttp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpManagedServerEntry {
    pub transport: McpTransport,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: IndexMap<String, String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub tool_call_timeout_ms: Option<u64>,
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_ms: u64,
    #[serde(default)]
    pub close_timeout_ms: Option<u64>,
    #[serde(default)]
    pub reconnect: Option<ReconnectConfig>,
}

const fn default_true() -> bool {
    true
}
const fn default_startup_timeout() -> u64 {
    DEFAULT_STARTUP_TIMEOUT_MS
}

impl McpManagedServerEntry {
    pub fn stdio(command: impl Into<String>) -> Self {
        Self {
            transport: McpTransport::Stdio,
            command: command.into(),
            args: Vec::new(),
            env: IndexMap::new(),
            cwd: String::new(),
            url: String::new(),
            headers: IndexMap::new(),
            enabled: true,
            tool_call_timeout_ms: None,
            startup_timeout_ms: DEFAULT_STARTUP_TIMEOUT_MS,
            close_timeout_ms: None,
            reconnect: None,
        }
    }

    pub fn streamable_http(url: impl Into<String>) -> Self {
        Self {
            transport: McpTransport::StreamableHttp,
            url: url.into(),
            ..Self::stdio("")
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub servers: Volatile<IndexMap<String, McpManagedServerEntry>>,
}

pub(crate) fn validate_servers(
    servers: &IndexMap<String, McpManagedServerEntry>,
) -> Result<(), BoxError> {
    for (name, entry) in servers {
        validate_entry_schema(name, entry)?;
    }
    for (name, entry) in servers {
        validate_entry_semantics(name, entry)?;
    }
    Ok(())
}

pub(crate) fn validate_entry(name: &str, entry: &McpManagedServerEntry) -> Result<(), BoxError> {
    validate_entry_schema(name, entry)?;
    validate_entry_semantics(name, entry)
}

fn validate_entry_schema(name: &str, entry: &McpManagedServerEntry) -> Result<(), BoxError> {
    let base = format!("servers.{name}");
    validate_timeout(
        entry.startup_timeout_ms,
        &format!("{base}.startupTimeoutMs"),
    )?;
    if let Some(value) = entry.close_timeout_ms {
        validate_timeout(value, &format!("{base}.closeTimeoutMs"))?;
    }
    validate_reconnect_schema(entry.reconnect.as_ref(), &format!("{base}.reconnect"))
}

fn validate_reconnect_schema(
    reconnect: Option<&ReconnectConfig>,
    path: &str,
) -> Result<(), BoxError> {
    const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
    let Some(reconnect) = reconnect else {
        return Ok(());
    };
    if let Some(value) = reconnect.initial_delay_ms {
        validate_timeout(value, &format!("{path}.initialDelayMs"))?;
    }
    if let Some(value) = reconnect.max_delay_ms {
        validate_timeout(value, &format!("{path}.maxDelayMs"))?;
    }
    if let Some(value) = reconnect.max_attempts
        && (value == 0 || value > MAX_SAFE_INTEGER)
    {
        return Err(format!("{path}.maxAttempts must be a positive safe integer").into());
    }
    Ok(())
}

fn validate_entry_semantics(name: &str, entry: &McpManagedServerEntry) -> Result<(), BoxError> {
    validate_entry_name(name)?;
    let base = format!("servers.{name}");
    match entry.transport {
        McpTransport::Stdio => {
            if entry.command.is_empty() {
                return Err(format!("{base}: a stdio server requires \"command\"").into());
            }
            reject_nonempty(&base, "url", &entry.url, "stdio", "streamable-http")?;
            if !entry.headers.is_empty() {
                return Err(format!(
                    "{base}: \"headers\" is not a stdio field — remove it or set transport: streamable-http"
                )
                .into());
            }
            validate_template(&entry.command, &format!("{base}.command"))?;
            for (index, value) in entry.args.iter().enumerate() {
                validate_template(value, &format!("{base}.args[{index}]"))?;
            }
            for (key, value) in &entry.env {
                validate_template(value, &format!("{base}.env.{key}"))?;
            }
            validate_template(&entry.cwd, &format!("{base}.cwd"))?;
        }
        McpTransport::StreamableHttp => {
            if entry.url.is_empty() {
                return Err(format!("{base}: a streamable-http server requires \"url\"").into());
            }
            reject_nonempty(&base, "command", &entry.command, "streamable-http", "stdio")?;
            if !entry.args.is_empty() {
                return Err(format!(
                    "{base}: \"args\" is not a streamable-http field — remove it or set transport: stdio"
                )
                .into());
            }
            if !entry.env.is_empty() {
                return Err(format!(
                    "{base}: \"env\" is not a streamable-http field — remove it or set transport: stdio"
                )
                .into());
            }
            reject_nonempty(&base, "cwd", &entry.cwd, "streamable-http", "stdio")?;
            validate_template(&entry.url, &format!("{base}.url"))?;
            for (key, value) in &entry.headers {
                validate_template(value, &format!("{base}.headers.{key}"))?;
            }
        }
    }
    resolve_reconnect_policy(entry.reconnect.as_ref(), &format!("{base}.reconnect"))?;
    Ok(())
}

fn validate_entry_name(name: &str) -> Result<(), BoxError> {
    if hoh_mcp_client::valid_server_name(name) {
        Ok(())
    } else {
        Err(format!(
            "servers.{name}: server name must match /^[A-Za-z0-9_-]{{1,32}}$/ — rename the entry key"
        )
        .into())
    }
}

fn validate_timeout(value: u64, path: &str) -> Result<(), BoxError> {
    if value == 0 || value > hoh_timeout::MAX_TIMER_DELAY_MS {
        Err(format!(
            "{path} must be a positive finite number no greater than {}",
            hoh_timeout::MAX_TIMER_DELAY_MS
        )
        .into())
    } else {
        Ok(())
    }
}

fn reject_nonempty(
    base: &str,
    field: &str,
    value: &str,
    transport: &str,
    alternative: &str,
) -> Result<(), BoxError> {
    if value.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{base}: \"{field}\" is not a {transport} field — remove it or set transport: {alternative}"
        )
        .into())
    }
}

fn validate_template(value: &str, path: &str) -> Result<(), BoxError> {
    assert_template_syntax(value, path).map_err(|error| Box::new(error) as BoxError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_the_source() {
        let entry: McpManagedServerEntry = serde_json::from_value(json!({
            "transport": "stdio", "command": "echo"
        }))
        .unwrap();
        assert!(entry.enabled);
        assert_eq!(entry.startup_timeout_ms, 30_000);
        assert!(entry.args.is_empty());
    }

    #[test]
    fn accepts_stdio_and_http_entries() {
        validate_entry("local", &McpManagedServerEntry::stdio("echo")).unwrap();
        validate_entry(
            "remote",
            &McpManagedServerEntry::streamable_http("http://localhost:1"),
        )
        .unwrap();
    }

    #[test]
    fn rejects_invalid_server_name() {
        assert!(
            validate_entry("bad name!", &McpManagedServerEntry::stdio("x"))
                .unwrap_err()
                .to_string()
                .contains("server name must match")
        );
    }

    #[test]
    fn invalid_name_precedes_transport_validation_after_schema_validation() {
        assert_eq!(
            validate_entry("bad name!", &McpManagedServerEntry::stdio(""))
                .unwrap_err()
                .to_string(),
            "servers.bad name!: server name must match /^[A-Za-z0-9_-]{1,32}$/ — rename the entry key"
        );
    }

    #[test]
    fn invalid_name_in_an_earlier_entry_precedes_a_later_transport_error() {
        let mut servers = IndexMap::new();
        servers.insert("bad name!".into(), McpManagedServerEntry::stdio("x"));
        servers.insert("b".into(), McpManagedServerEntry::stdio(""));

        assert_eq!(
            validate_servers(&servers).unwrap_err().to_string(),
            "servers.bad name!: server name must match /^[A-Za-z0-9_-]{1,32}$/ — rename the entry key"
        );
    }

    #[test]
    fn invalid_name_precedes_template_validation_after_schema_validation() {
        assert_eq!(
            validate_entry(
                "bad name!",
                &McpManagedServerEntry::stdio("${x${credential:A}"),
            )
            .unwrap_err()
            .to_string(),
            "servers.bad name!: server name must match /^[A-Za-z0-9_-]{1,32}$/ — rename the entry key"
        );
    }

    #[test]
    fn stdio_requires_command() {
        assert_eq!(
            validate_entry("a", &McpManagedServerEntry::stdio(""))
                .unwrap_err()
                .to_string(),
            "servers.a: a stdio server requires \"command\""
        );
    }

    #[test]
    fn transport_validation_errors_match_the_source_exactly() {
        let mut stdio_url = McpManagedServerEntry::stdio("x");
        stdio_url.url = "http://x".into();
        let mut stdio_headers = McpManagedServerEntry::stdio("x");
        stdio_headers.headers.insert("h".into(), "v".into());
        let http_missing = McpManagedServerEntry {
            transport: McpTransport::StreamableHttp,
            ..McpManagedServerEntry::stdio("")
        };
        let mut http_command = McpManagedServerEntry::streamable_http("http://x");
        http_command.command = "x".into();
        let mut http_args = McpManagedServerEntry::streamable_http("http://x");
        http_args.args.push("x".into());
        let mut http_env = McpManagedServerEntry::streamable_http("http://x");
        http_env.env.insert("k".into(), "v".into());
        let mut http_cwd = McpManagedServerEntry::streamable_http("http://x");
        http_cwd.cwd = "/tmp".into();
        let cases = [
            (
                McpManagedServerEntry::stdio(""),
                "servers.a: a stdio server requires \"command\"",
            ),
            (
                stdio_url,
                "servers.a: \"url\" is not a stdio field — remove it or set transport: streamable-http",
            ),
            (
                stdio_headers,
                "servers.a: \"headers\" is not a stdio field — remove it or set transport: streamable-http",
            ),
            (
                http_missing,
                "servers.a: a streamable-http server requires \"url\"",
            ),
            (
                http_command,
                "servers.a: \"command\" is not a streamable-http field — remove it or set transport: stdio",
            ),
            (
                http_args,
                "servers.a: \"args\" is not a streamable-http field — remove it or set transport: stdio",
            ),
            (
                http_env,
                "servers.a: \"env\" is not a streamable-http field — remove it or set transport: stdio",
            ),
            (
                http_cwd,
                "servers.a: \"cwd\" is not a streamable-http field — remove it or set transport: stdio",
            ),
        ];
        for (entry, expected) in cases {
            assert_eq!(
                validate_entry("a", &entry).unwrap_err().to_string(),
                expected
            );
        }
    }

    #[test]
    fn stdio_rejects_url() {
        let mut entry = McpManagedServerEntry::stdio("x");
        entry.url = "http://x".into();
        assert!(
            validate_entry("a", &entry)
                .unwrap_err()
                .to_string()
                .contains("not a stdio field")
        );
    }

    #[test]
    fn http_requires_url() {
        let entry = McpManagedServerEntry {
            transport: McpTransport::StreamableHttp,
            ..McpManagedServerEntry::stdio("")
        };
        assert!(
            validate_entry("a", &entry)
                .unwrap_err()
                .to_string()
                .contains("requires \"url\"")
        );
    }

    #[test]
    fn http_rejects_stdio_fields() {
        let mut entry = McpManagedServerEntry::streamable_http("http://x");
        entry.command = "x".into();
        assert!(
            validate_entry("a", &entry)
                .unwrap_err()
                .to_string()
                .contains("not a streamable-http field")
        );
    }

    #[test]
    fn rejects_legacy_env_templates() {
        let mut entry = McpManagedServerEntry::stdio("x");
        entry.env.insert("K".into(), "${env:K}".into());
        assert!(
            validate_entry("a", &entry)
                .unwrap_err()
                .to_string()
                .contains("use ${credential:REF}")
        );
    }

    #[test]
    fn rejects_nested_template_openers() {
        let entry = McpManagedServerEntry::stdio("${x${credential:A}");
        assert!(
            validate_entry("a", &entry)
                .unwrap_err()
                .to_string()
                .contains("not a template token")
        );
    }

    #[test]
    fn rejects_inverted_reconnect_delays() {
        let mut entry = McpManagedServerEntry::stdio("x");
        entry.reconnect = Some(ReconnectConfig {
            enabled: Some(true),
            initial_delay_ms: Some(5_000),
            max_delay_ms: Some(100),
            max_attempts: Some(3),
        });
        assert!(
            validate_entry("a", &entry)
                .unwrap_err()
                .to_string()
                .contains("initialDelayMs must be less than or equal")
        );
    }

    #[test]
    fn reconnect_schema_bounds_are_checked_before_entry_semantics() {
        let max = hoh_timeout::MAX_TIMER_DELAY_MS;
        let max_safe_integer = 9_007_199_254_740_991;
        for reconnect in [
            ReconnectConfig {
                enabled: None,
                initial_delay_ms: Some(0),
                max_delay_ms: None,
                max_attempts: None,
            },
            ReconnectConfig {
                enabled: None,
                initial_delay_ms: Some(max + 1),
                max_delay_ms: None,
                max_attempts: None,
            },
            ReconnectConfig {
                enabled: None,
                initial_delay_ms: None,
                max_delay_ms: Some(0),
                max_attempts: None,
            },
            ReconnectConfig {
                enabled: None,
                initial_delay_ms: None,
                max_delay_ms: Some(max + 1),
                max_attempts: None,
            },
            ReconnectConfig {
                enabled: None,
                initial_delay_ms: None,
                max_delay_ms: None,
                max_attempts: Some(0),
            },
            ReconnectConfig {
                enabled: None,
                initial_delay_ms: None,
                max_delay_ms: None,
                max_attempts: Some(max_safe_integer + 1),
            },
        ] {
            let mut entry = McpManagedServerEntry::stdio("");
            entry.reconnect = Some(reconnect);
            let error = validate_entry("bad name!", &entry).unwrap_err().to_string();
            assert!(error.starts_with("servers.bad name!.reconnect."), "{error}");
        }
    }

    #[test]
    fn transport_error_precedes_reconnect_error() {
        let mut entry = McpManagedServerEntry::stdio("");
        entry.reconnect = Some(ReconnectConfig {
            enabled: Some(true),
            initial_delay_ms: Some(5_000),
            max_delay_ms: Some(100),
            max_attempts: Some(3),
        });
        assert_eq!(
            validate_entry("a", &entry).unwrap_err().to_string(),
            "servers.a: a stdio server requires \"command\""
        );
    }

    #[test]
    fn tool_call_timeout_has_the_sources_unbounded_number_contract() {
        let mut entry = McpManagedServerEntry::stdio("x");
        entry.tool_call_timeout_ms = Some(0);
        validate_entry("a", &entry).unwrap();
    }

    #[test]
    fn rejects_zero_timeout() {
        let mut entry = McpManagedServerEntry::stdio("x");
        entry.startup_timeout_ms = 0;
        assert!(validate_entry("a", &entry).is_err());
    }

    #[test]
    fn timeout_error_precedes_invalid_entry_name_like_schemastery() {
        let mut entry = McpManagedServerEntry::stdio("x");
        entry.startup_timeout_ms = 0;
        assert_eq!(
            validate_entry("bad name!", &entry).unwrap_err().to_string(),
            "servers.bad name!.startupTimeoutMs must be a positive finite number no greater than 2147483647"
        );
    }

    #[test]
    fn all_entry_fields_are_validated_before_any_entry_name_like_schemastery() {
        let mut servers = IndexMap::new();
        servers.insert("bad name!".into(), McpManagedServerEntry::stdio("x"));
        let mut later = McpManagedServerEntry::stdio("x");
        later.startup_timeout_ms = 0;
        servers.insert("b".into(), later);

        assert_eq!(
            validate_servers(&servers).unwrap_err().to_string(),
            "servers.b.startupTimeoutMs must be a positive finite number no greater than 2147483647"
        );
    }

    #[test]
    fn field_names_are_camel_case() {
        let value = serde_json::to_value(McpManagedServerEntry::stdio("echo")).unwrap();
        assert_eq!(value["startupTimeoutMs"], 30_000);
        assert!(value.get("startup_timeout_ms").is_none());
    }
}
