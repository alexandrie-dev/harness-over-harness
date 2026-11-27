//! Optional titles from a single ephemeral Codex invocation.

use std::path::Path;
use std::time::Duration;

use hoh_external_harness::{
    Frame, HarnessFailure, HarnessResult, ProcessSpec, Supervisor, SupervisorLimits, TitleRequest,
    code,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::harness::{CodexConfig, app_server_spec, projects_config};
use crate::session::CodexSession;

const TITLE_SCHEMA: &str = r#"{"type":"object","properties":{"title":{"type":"string"}},"required":["title"],"additionalProperties":false}"#;
const INPUT_LIMIT: usize = 64 * 1024;
const FRAME_LIMIT: usize = 128;
const TITLE_TIMEOUT: Duration = Duration::from_secs(35);
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/";
const CHATGPT_ORIGIN: &str = "https://chatgpt.com";
const CREDENTIAL_OVERRIDES: &[&str] = &["CODEX_API_KEY", "CODEX_ACCESS_TOKEN", "OPENAI_API_KEY"];
static TITLE_WORKERS: Semaphore = Semaphore::const_new(2);

const DISABLED_FEATURES: &[&str] = &[
    "apps",
    "code_mode",
    "code_mode_host",
    "hooks",
    "memories",
    "multi_agent",
    "multi_agent_v2",
    "plugins",
    "request_permissions_tool",
    "shell_snapshot",
    "shell_tool",
    "view_image",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleResponse {
    title: String,
}

struct OneShotProcess(Supervisor);

impl Drop for OneShotProcess {
    fn drop(&mut self) {
        self.0.abandon();
    }
}

pub(crate) fn unavailable() -> HarnessFailure {
    HarnessFailure::new(
        code::POLICY_UNAVAILABLE,
        "An isolated Codex title request is unavailable for this conversation's configuration.",
    )
}

pub(crate) fn verified_auth_store(
    config: &Value,
    requirements: &Value,
) -> HarnessResult<&'static str> {
    let config = config
        .get("config")
        .and_then(Value::as_object)
        .ok_or_else(unavailable)?;
    let is_default = |key| config.get(key).is_none_or(Value::is_null);
    let openai_is_overridden = match config.get("model_providers") {
        None | Some(Value::Null) => false,
        Some(Value::Object(providers)) => providers.contains_key("openai"),
        _ => true,
    };
    // The title launch skips user configuration but uses the same Codex authentication and
    // environment. Only the standard OpenAI route can be reproduced that way.
    if !requirements.get("requirements").is_some_and(Value::is_null)
        || config
            .get("model_provider")
            .filter(|value| !value.is_null())
            .is_some_and(|value| value.as_str() != Some("openai"))
        || openai_is_overridden
        || !is_default("openai_base_url")
        || !is_default("experimental_realtime_ws_base_url")
        || !is_default("forced_login_method")
        || !is_default("forced_chatgpt_workspace_id")
        || config
            .get("chatgpt_base_url")
            .is_some_and(|url| !url.is_null() && url.as_str() != Some(CHATGPT_BASE_URL))
    {
        return Err(unavailable());
    }
    match config
        .get("cli_auth_credentials_store")
        .and_then(Value::as_str)
    {
        None if is_default("cli_auth_credentials_store") => Ok("file"),
        Some("file") => Ok("file"),
        // The isolated command ignores user config, which can select a different keyring backend.
        _ => Err(unavailable()),
    }
}

pub(crate) fn verified_account(account: Value) -> HarnessResult<Value> {
    if account["requiresOpenaiAuth"].as_bool() != Some(true) {
        return Err(unavailable());
    }
    let routing = account.get("workspaceRouting").ok_or_else(unavailable)?;
    let standard = match account.pointer("/account/type").and_then(Value::as_str) {
        Some("chatgpt") if routing.is_null() => account
            .pointer("/account/email")
            .and_then(Value::as_str)
            .is_some_and(|email| !email.is_empty()),
        Some("chatgpt") => {
            routing["backendOrigin"].as_str() == Some(CHATGPT_ORIGIN)
                && routing["chatgptAccountId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty())
                && routing["accountRoutingOverride"].as_str() == Some("NO_CONSTRAINT")
        }
        _ => false,
    };
    if !standard {
        return Err(unavailable());
    }
    Ok(json!({"account": account["account"], "workspaceRouting": routing}))
}

fn route_settings(auth_store: &str) -> [(&'static str, &str); 3] {
    [
        ("openai_base_url", ""),
        ("chatgpt_base_url", CHATGPT_BASE_URL),
        ("cli_auth_credentials_store", auth_store),
    ]
}

fn title_spec(
    config: &CodexConfig,
    cwd: &Path,
    schema: &Path,
    model: &str,
    auth_store: &str,
) -> ProcessSpec {
    let mut args = [
        "exec",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--json",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "--color",
        "never",
        "--output-schema",
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(schema.to_string_lossy().into_owned());
    args.push(format!("--model={model}"));
    for feature in DISABLED_FEATURES {
        args.extend(["--disable".to_owned(), (*feature).to_owned()]);
    }
    for setting in [
        r#"web_search="disabled""#,
        r#"approval_policy="never""#,
        "mcp_servers={}",
        "skills.bundled.enabled=false",
        "skills.include_instructions=false",
        "project_doc_max_bytes=0",
        r#"model_reasoning_effort="low""#,
        r#"model_provider="openai""#,
    ] {
        args.extend(["-c".to_owned(), setting.to_owned()]);
    }
    for (key, value) in route_settings(auth_store) {
        args.extend(["-c".to_owned(), format!("{key}={value:?}")]);
    }
    args.extend(["-c".to_owned(), projects_config(cwd, false), "-".to_owned()]);
    ProcessSpec {
        program: config.executable.clone(),
        args,
        cwd: Some(cwd.to_owned()),
        env: config.extra_env.clone(),
        env_remove: CREDENTIAL_OVERRIDES
            .iter()
            .map(|name| (*name).into())
            .collect(),
        ..ProcessSpec::default()
    }
}

fn invalid_output() -> HarnessFailure {
    HarnessFailure::new(code::PROTOCOL, "Codex returned an invalid title response.")
}

async fn one_shot(spec: &ProcessSpec, prompt: &str) -> HarnessResult<String> {
    let limits = SupervisorLimits {
        max_frame_bytes: 64 * 1024,
        frame_queue: 8,
        stdin_queue: 1,
        stderr_lines: 4,
        stderr_line_bytes: 512,
    };
    let process = OneShotProcess(Supervisor::spawn(spec, limits, None)?);
    let mut frames = process.0.take_frames().ok_or_else(invalid_output)?;
    process
        .0
        .send_input_and_close(prompt.as_bytes(), INPUT_LIMIT)
        .await?;
    let mut response = None;
    let mut complete = false;
    let mut frame_count = 0;
    while let Some(frame) = frames.recv().await {
        let value = match frame {
            Frame::Message(value) => value,
            Frame::Closed { truncated: false } => break,
            _ => return Err(invalid_output()),
        };
        frame_count += 1;
        if frame_count > FRAME_LIMIT {
            return Err(invalid_output());
        }
        let item = value.pointer("/item/type").and_then(Value::as_str);
        match value.get("type").and_then(Value::as_str) {
            Some("item.started" | "item.completed")
                if matches!(
                    item,
                    Some(
                        "command_execution"
                            | "file_change"
                            | "mcp_tool_call"
                            | "web_search"
                            | "collab_tool_call"
                    )
                ) =>
            {
                return Err(HarnessFailure::new(
                    code::POLICY_UNAVAILABLE,
                    "The Codex title request attempted to use a tool.",
                ));
            }
            Some("item.completed") if item == Some("agent_message") => {
                response = value
                    .pointer("/item/text")
                    .and_then(Value::as_str)
                    .and_then(|text| serde_json::from_str::<TitleResponse>(text).ok())
                    .map(|response| response.title);
            }
            Some("turn.completed") => complete = true,
            Some("turn.failed") => {
                return Err(HarnessFailure::new(
                    code::HARNESS,
                    "The Codex title request failed.",
                ));
            }
            _ => {}
        }
    }
    if !process.0.wait().await.success() || !complete {
        return Err(HarnessFailure::new(
            code::HARNESS,
            "The Codex title request failed.",
        ));
    }
    let title = response.ok_or_else(invalid_output)?;
    let title = title.trim();
    if title.is_empty() || title.len() > 256 || title.chars().any(char::is_control) {
        return Err(invalid_output());
    }
    Ok(title.to_owned())
}

pub(crate) fn supports_environment(config: &CodexConfig) -> bool {
    CREDENTIAL_OVERRIDES.iter().all(|name| {
        std::env::var_os(name).is_none() && config.extra_env.iter().all(|(key, _)| key != name)
    }) && ["CODEX_HOME", "HOME"].into_iter().all(|name| {
        config
            .extra_env
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| std::ffi::OsString::from(value))
            .or_else(|| std::env::var_os(name))
            .is_none_or(|path| Path::new(&path).is_absolute())
    })
}

pub(crate) struct TitleRoute {
    config: CodexConfig,
    version: String,
    auth_store: &'static str,
}

impl TitleRoute {
    pub(crate) fn new(config: CodexConfig, version: String, auth_store: &'static str) -> Self {
        Self {
            config,
            version,
            auth_store,
        }
    }

    pub(crate) fn pin(&self, config: &mut Value) {
        for (key, value) in route_settings(self.auth_store) {
            config[key] = json!(value);
        }
    }

    pub(crate) async fn generate(
        &self,
        request: TitleRequest,
        live: &CodexSession,
    ) -> HarnessResult<String> {
        let config = &self.config;
        let (live_cwd, live_trusted) = live.title_workspace();
        let model = request
            .model
            .as_deref()
            .filter(|model| !model.is_empty())
            .ok_or_else(unavailable)?;
        let workspace = request
            .workspace
            .as_ref()
            .filter(|workspace| {
                workspace.path() == Path::new(live_cwd) && workspace.path().is_absolute()
            })
            .ok_or_else(unavailable)?;
        if request.prompt.is_empty() || request.prompt.len() > INPUT_LIMIT {
            return Err(HarnessFailure::new(
                code::PROTOCOL,
                "Invalid Codex title input length.",
            ));
        }
        tokio::time::timeout(TITLE_TIMEOUT, async {
            let _worker = TITLE_WORKERS.acquire().await.map_err(|_| unavailable())?;
            if !live.is_running() || !supports_environment(config) {
                return Err(unavailable());
            }
            let (probe, trusted) = workspace
                .spawn_probe(config.limits, |cwd, trusted| {
                    app_server_spec(config, cwd, trusted)
                })
                .await?;
            let (auth_store, account) = CodexSession::probe_title_route(
                probe,
                config.request_timeout.min(PROBE_TIMEOUT),
                live_cwd.to_owned(),
                trusted,
                &self.version,
            )
            .await
            .map_err(|_| unavailable())?;
            if auth_store != self.auth_store
                || trusted != live_trusted
                || live.title_account().await.map_err(|_| unavailable())? != account
            {
                return Err(unavailable());
            }
            let directory = tempfile::Builder::new()
                .prefix("hoh-codex-title-")
                .tempdir()
                .map_err(|error| {
                    HarnessFailure::new(
                        code::IO,
                        format!("Cannot create Codex title workspace: {error}"),
                    )
                })?;
            let schema = directory.path().join("title-schema.json");
            tokio::fs::write(&schema, TITLE_SCHEMA)
                .await
                .map_err(|error| {
                    HarnessFailure::new(
                        code::IO,
                        format!("Cannot write the Codex title schema: {error}"),
                    )
                })?;
            let spec = title_spec(config, directory.path(), &schema, model, self.auth_store);
            if !live.is_running() || !supports_environment(config) {
                return Err(unavailable());
            }
            one_shot(&spec, &request.prompt).await
        })
        .await
        .map_err(|_| HarnessFailure::new(code::TIMEOUT, "The Codex title request timed out."))?
    }
}
