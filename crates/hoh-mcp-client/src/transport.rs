use crate::Config;
use hoh_error::BoxError;
use http::{HeaderName, HeaderValue};
use rmcp::transport::{
    StreamableHttpClientTransport, TokioChildProcess,
    streamable_http_client::StreamableHttpClientTransportConfig,
};
use std::collections::HashMap;

pub fn stdio_transport(config: &Config) -> Result<TokioChildProcess, BoxError> {
    TokioChildProcess::new(stdio_command(config)?).map_err(|error| Box::new(error) as BoxError)
}

fn stdio_command(config: &Config) -> Result<tokio::process::Command, BoxError> {
    stdio_command_with_environment(config, hoh_subprocess::scrubbed_parent_env(None))
}

fn stdio_command_with_environment(
    config: &Config,
    parent: indexmap::IndexMap<String, String>,
) -> Result<tokio::process::Command, BoxError> {
    let Config::Stdio {
        command,
        args,
        env,
        cwd,
        ..
    } = config
    else {
        return Err("expected stdio MCP configuration".into());
    };
    let mut process = tokio::process::Command::new(command);
    process.args(args).env_clear();
    for (name, value) in parent {
        process.env(name, value);
    }
    for (name, value) in env {
        process.env(name, value);
    }
    if !cwd.is_empty() {
        process.current_dir(cwd);
    }
    Ok(process)
}

pub fn http_transport(
    config: &Config,
) -> Result<StreamableHttpClientTransport<reqwest::Client>, BoxError> {
    let Config::StreamableHttp { url, headers, .. } = config else {
        return Err("expected streamable-http MCP configuration".into());
    };
    let mut custom = HashMap::new();
    for (name, value) in headers {
        custom.insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value)?,
        );
    }
    Ok(StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.clone()).custom_headers(custom),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn stdio_command_scrubs_then_overrides_environment_and_keeps_cwd() {
        let config = Config::Stdio {
            server_name: "test".into(),
            command: "server".into(),
            args: vec!["--stdio".into()],
            env: IndexMap::from_iter([
                ("PATH".into(), "/explicit/bin".into()),
                ("MCP_ONLY".into(), "yes".into()),
            ]),
            cwd: "/tmp".into(),
            tool_call_timeout_ms: 10,
            startup_timeout_ms: None,
            close_timeout_ms: 10,
            expand_templates: false,
            fail_on_startup_error: false,
            max_instruction_bytes: None,
            reconnect: None,
        };
        let parent = hoh_subprocess::scrubbed_parent_env_from(
            [
                ("SAFE_PARENT".into(), Some("kept".into())),
                ("API_SECRET".into(), Some("removed".into())),
                ("DSH_INTERNAL".into(), Some("removed".into())),
                ("PATH".into(), Some("/parent/bin".into())),
            ],
            None,
        );
        let command = stdio_command_with_environment(&config, parent).unwrap();
        let command = command.as_std();
        assert_eq!(command.get_program(), "server");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--stdio"]);
        assert_eq!(
            command.get_current_dir().unwrap(),
            std::path::Path::new("/tmp")
        );
        let env = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(env.get("PATH"), Some(&Some("/explicit/bin".into())));
        assert_eq!(env.get("MCP_ONLY"), Some(&Some("yes".into())));
        assert_eq!(env.get("SAFE_PARENT"), Some(&Some("kept".into())));
        assert!(!env.contains_key("API_SECRET"));
        assert!(!env.contains_key("DSH_INTERNAL"));
    }
}
