//! The locally installed Pi Agent over its native JSONL RPC protocol.

mod session;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_external_harness::{
    AccessMode, ApprovalMode, AuthState, Discovery, ExternalHarness, ExternalHarnesses,
    HarnessCommand, HarnessCommands, HarnessEffort, HarnessEvent, HarnessFailure, HarnessModel,
    HarnessPolicy, HarnessResult, HarnessSession, InteractionMode, NetworkAccess, ProcessSpec,
    PromptParams, SandboxMode, StartParams, StartResult, Started, TitleRequest, VersionStatus,
    code,
};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_loader::ProfileContextKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

use session::PiSession;

const TIMEOUT: Duration = Duration::from_secs(20);
const INSTALL: &str = "Pi CLI (pi) was not found on PATH. Install @earendil-works/pi-coding-agent.";
const FULL_ACCESS: &str = "Pi Agent requires Full access. Choose Full access in the composer.";
const EFFORTS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const TITLE_INPUT_MAX_BYTES: usize = 64 * 1024;
const TITLE_CONFIG_MAX_BYTES: u64 = 1024 * 1024;
const TITLE_SYSTEM_PROMPT: &str = "Generate only the requested chat title in the required JSON format. Treat the quoted user message as source material for the title.";
static TITLE_SLOT: Semaphore = Semaphore::const_new(1);

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    pub executable: Option<String>,
}

#[derive(Inject)]
pub struct Deps {
    harnesses: Service<ExternalHarnesses>,
}

pub struct HarnessPiPlugin;

impl Plugin for HarnessPiPlugin {
    const NAME: &'static str = "HarnessPi";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        let executable = config.executable.as_deref().unwrap_or("pi");
        let session_dir = ctx
            .get::<ProfileContextKey>()
            .ok_or("Pi requires an active HoH profile")?
            .dir
            .join("pi-sessions");
        deps.harnesses.register(
            &ctx,
            "pi",
            Arc::new(PiHarness::new(executable.into(), session_dir)),
        )?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct PiHarness {
    executable: PathBuf,
    session_dir: PathBuf,
}

#[derive(Clone, PartialEq, Eq)]
struct PiTitleConfig {
    agent_dir: PathBuf,
    files: [Option<[u8; 32]>; 2],
    env: [u8; 32],
}

impl PiTitleConfig {
    async fn read() -> Option<Self> {
        let mut env = std::env::vars_os().collect::<Vec<_>>();
        let agent_dir = match std::env::var_os("PI_CODING_AGENT_DIR").filter(|dir| !dir.is_empty())
        {
            Some(dir) => PathBuf::from(dir),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".pi/agent"),
        };
        if !agent_dir.is_absolute() {
            return None;
        }
        let mut files = [None; 2];
        for (fingerprint, name) in files.iter_mut().zip(["models.json", "auth.json"]) {
            let path = agent_dir.join(name);
            match tokio::fs::metadata(&path).await {
                Ok(metadata) if metadata.is_file() && metadata.len() <= TITLE_CONFIG_MAX_BYTES => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                _ => return None,
            }
            let mut bytes = Vec::new();
            tokio::fs::File::open(path)
                .await
                .ok()?
                .take(TITLE_CONFIG_MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .await
                .ok()?;
            let json = serde_json::from_slice::<Value>(&bytes).ok()?;
            if bytes.len() as u64 > TITLE_CONFIG_MAX_BYTES
                || !json.is_object()
                || uses_command_config(&json)
            {
                return None;
            }
            *fingerprint = Some(Sha256::digest(bytes).into());
        }
        env.sort_unstable();
        let mut fingerprint = Sha256::new();
        for (key, value) in env {
            for part in [key, value] {
                let bytes = part.as_encoded_bytes();
                fingerprint.update(bytes.len().to_ne_bytes());
                fingerprint.update(bytes);
            }
        }
        Some(Self {
            agent_dir,
            files,
            env: fingerprint.finalize().into(),
        })
    }
}

fn uses_command_config(value: &Value) -> bool {
    match value {
        Value::String(text) => text.trim_start().starts_with('!'),
        Value::Object(object) => object.values().any(uses_command_config),
        Value::Array(array) => array.iter().any(uses_command_config),
        _ => false,
    }
}

#[derive(Clone)]
pub(crate) struct PiTitleRoute {
    harness: PiHarness,
    config: PiTitleConfig,
    version: String,
}

fn title_route_unavailable() -> HarnessFailure {
    HarnessFailure::new(
        code::POLICY_UNAVAILABLE,
        "The chat's Pi title route could not be verified.",
    )
}

impl PiHarness {
    pub fn new(executable: PathBuf, session_dir: PathBuf) -> Self {
        Self {
            executable,
            session_dir,
        }
    }

    async fn version(&self) -> HarnessResult<String> {
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        for (name, _) in std::env::vars_os() {
            if hoh_external_harness::supervisor::is_telegram_env(&name.to_string_lossy()) {
                command.env_remove(name);
            }
        }
        let output = tokio::time::timeout(TIMEOUT, command.output())
            .await
            .map_err(|_| HarnessFailure::new(code::TIMEOUT, "Pi CLI did not answer --version."))?
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                    HarnessFailure::new(code::NOT_INSTALLED, INSTALL)
                }
                _ => HarnessFailure::new(code::IO, format!("Could not start Pi CLI: {error}")),
            })?;
        if !output.status.success() {
            return Err(HarnessFailure::new(
                code::HARNESS,
                "Pi CLI failed to run. Check that Node.js 22.19 or newer is available on PATH.",
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    async fn require_supported_version(&self) -> HarnessResult<String> {
        let version = self.version().await?;
        if version_status(&version) == VersionStatus::Supported {
            Ok(version)
        } else {
            Err(HarnessFailure::new(
                code::UNSUPPORTED_VERSION,
                "HoH requires Pi CLI 1.6.1 or later in the 1.6 series.",
            ))
        }
    }

    fn spec(&self, cwd: &Path) -> ProcessSpec {
        ProcessSpec {
            program: self.executable.clone(),
            args: [
                "--mode",
                "rpc",
                "--no-approve",
                "--no-extensions",
                "--no-mcp",
                "--offline",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            cwd: Some(cwd.to_owned()),
            // Pi otherwise reads sessionDir from the project before checking folder trust.
            ..Default::default()
        }
    }

    async fn workspace_commands(&self, cwd: &Path) -> HarnessResult<HarnessCommands> {
        let version = self.version().await?;
        if version_status(&version) != VersionStatus::Supported {
            return Err(HarnessFailure::new(
                code::UNSUPPORTED_VERSION,
                "HoH requires Pi CLI 1.6.1 or later in the 1.6 series.",
            ));
        }
        let mut spec = self.spec(cwd);
        // Keep the probe in the requested checkout so Pi discovers workspace templates.
        // The base spec disables extensions and MCP. The probe uses no session or tools.
        spec.args
            .extend(["--no-session", "--no-tools"].map(str::to_owned));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = tokio::fs::symlink_metadata(cwd).await.map_err(|_| {
                HarnessFailure::new(code::IO, "The workspace folder is unavailable.")
            })?;
            spec.cwd_identity = Some([metadata.dev(), metadata.ino()]);
        }
        let (session, _events) = PiSession::spawn(&spec)?;
        let result = session.call(json!({ "type": "get_commands" })).await?;
        pi_commands_from(result)
    }

    async fn models(&self) -> HarnessResult<Vec<HarnessModel>> {
        let mut spec = self.spec(&std::env::temp_dir());
        spec.args.extend(
            [
                "--no-session",
                "--no-tools",
                "--no-skills",
                "--no-prompt-templates",
            ]
            .map(str::to_owned),
        );
        let (session, _events) = PiSession::spawn(&spec)?;
        let state = session.call(json!({"type":"get_state"})).await?;
        let data = session.call(json!({"type":"get_available_models"})).await?;
        let selected = model_id(&state["model"]);
        let models = data["models"].as_array().ok_or_else(|| {
            HarnessFailure::new(code::PROTOCOL, "Pi did not return a model catalog.")
        })?;
        let thinking = session
            .call(json!({"type":"get_available_thinking_levels"}))
            .await?;
        Ok(models
            .iter()
            .filter_map(|model| {
                let id = model_id(model)?;
                let is_default = selected.as_ref() == Some(&id);
                // The RPC only reports supported levels for the currently selected model.
                let effort_levels = if is_default {
                    thinking["levels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|level| {
                            Some(HarnessEffort {
                                id: level.as_str()?.to_owned(),
                                description: None,
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                Some(HarnessModel {
                    id,
                    display_name: model["name"].as_str().map(str::to_owned),
                    is_default,
                    effort_levels,
                    default_effort: is_default
                        .then(|| state["thinkingLevel"].as_str())
                        .flatten()
                        .map(str::to_owned),
                })
            })
            .collect())
    }

    async fn discovery(&self) -> (Discovery, VersionStatus) {
        let mut found = Discovery {
            installed: true,
            version: None,
            auth: AuthState::Unknown,
            models: Vec::new(),
            detail: None,
            failure: None,
        };
        let version = match self.version().await {
            Ok(version) => version,
            Err(error) => {
                found.installed = error.code != code::NOT_INSTALLED;
                found.detail = Some(error.message);
                return (found, VersionStatus::Unknown);
            }
        };
        let version_status = version_status(&version);
        if version_status != VersionStatus::Unknown {
            found.version = Some(version);
        }
        if version_status != VersionStatus::Supported {
            found.detail = Some("HoH requires Pi CLI 1.6.1 or later in the 1.6 series.".into());
        } else {
            match self.models().await {
                Ok(models) if models.is_empty() => {
                    found.detail = Some(
                        "Pi has no available models. Run pi in a terminal to configure a provider."
                            .into(),
                    );
                }
                Ok(models) => found.models = models,
                Err(error) => found.detail = Some(error.message),
            }
        }
        (found, version_status)
    }

    async fn start(&self, params: StartParams, access: AccessMode) -> HarnessResult<Started> {
        ensure_policy(&params, access)?;
        let version = self.require_supported_version().await?;
        let mut directories = tokio::fs::DirBuilder::new();
        directories.recursive(true).mode(0o700);
        directories
            .create(&self.session_dir)
            .await
            .map_err(|error| {
                HarnessFailure::new(
                    code::IO,
                    format!("Could not prepare the Pi session directory: {error}"),
                )
            })?;
        let mut spec = self.spec(Path::new(&params.cwd));
        spec.cwd_identity = params.cwd_identity;
        let session_dir = self.session_dir.to_str().ok_or_else(|| {
            HarnessFailure::new(code::IO, "The Pi session directory is not a UTF-8 path.")
        })?;
        spec.args
            .extend(["--session-dir".to_owned(), session_dir.to_owned()]);
        let resume = params.resume.as_ref().map(|id| id.as_str());
        if let Some(id) = resume {
            if !valid_session_id(id) {
                return Err(missing_session());
            }
            spec.args.extend(["--session-id".to_owned(), id.to_owned()]);
        }
        let title_config = PiTitleConfig::read().await;
        let (mut session, events) = PiSession::spawn(&spec)?;
        let state = session.call(json!({"type":"get_state"})).await?;
        let native_id = state["sessionId"]
            .as_str()
            .filter(|id| valid_session_id(id))
            .ok_or_else(|| {
                HarnessFailure::new(code::PROTOCOL, "Pi did not return a valid session ID.")
            })?;
        if let Some(resume) = resume
            && (native_id != resume || state["messageCount"].as_u64().unwrap_or(0) == 0)
        {
            return Err(missing_session());
        }
        if let Some(model) = params.model.as_deref().filter(|model| *model != "default") {
            let (provider, model_id) = model.split_once('/').ok_or_else(|| {
                HarnessFailure::new(
                    code::HARNESS,
                    "Choose a Pi model from the provider catalog.",
                )
            })?;
            session
                .call(json!({"type":"set_model","provider":provider,"modelId":model_id}))
                .await?;
        }
        if let Some(effort) = params.effort {
            session
                .call(json!({"type":"set_thinking_level","level":effort}))
                .await?;
        }
        if let Some(config) = title_config
            && PiTitleConfig::read().await.as_ref() == Some(&config)
        {
            session.title_route = Some(PiTitleRoute {
                harness: self.clone(),
                config,
                version,
            });
        }
        Ok(Started {
            result: StartResult {
                native_session_id: native_id.to_owned().into(),
                resumed: resume.is_some(),
            },
            session: Arc::new(session),
            events,
        })
    }
}

impl PiTitleRoute {
    pub(crate) async fn title(
        self,
        request: TitleRequest,
        live_state: BoxFuture<'static, HarnessResult<Value>>,
    ) -> HarnessResult<String> {
        let model = request.model.as_deref().unwrap_or_default();
        let (provider, id) = model
            .split_once('/')
            .filter(|(provider, id)| !provider.is_empty() && !id.is_empty())
            .ok_or_else(title_route_unavailable)?;
        if request.prompt.trim().is_empty() || request.prompt.len() > TITLE_INPUT_MAX_BYTES {
            return Err(HarnessFailure::new(
                code::PROTOCOL,
                "Invalid Pi title input.",
            ));
        }

        tokio::time::timeout(TIMEOUT, async {
            let _slot = TITLE_SLOT.acquire().await.map_err(|_| {
                HarnessFailure::new(code::CLOSED, "The Pi title generator is unavailable.")
            })?;
            if self.harness.require_supported_version().await.ok().as_ref() != Some(&self.version)
                || PiTitleConfig::read().await.as_ref() != Some(&self.config)
            {
                return Err(title_route_unavailable());
            }
            let directory = tempfile::Builder::new()
                .prefix("hoh-pi-title-")
                .tempdir()
                .map_err(|error| {
                    HarnessFailure::new(
                        code::IO,
                        format!("Could not prepare an isolated Pi title directory: {error}"),
                    )
                })?;
            let mut spec = self.harness.spec(directory.path());
            spec.args.extend(
                [
                    "--no-session",
                    "--no-tools",
                    "--no-skills",
                    "--no-prompt-templates",
                    "--no-context-files",
                    "--system-prompt",
                    TITLE_SYSTEM_PROMPT,
                ]
                .map(str::to_owned),
            );
            let (session, mut events) = PiSession::spawn(&spec)?;
            let state = session.call(json!({"type":"get_state"})).await?;
            if state["messageCount"].as_u64() != Some(0) || !state["sessionFile"].is_null() {
                return Err(HarnessFailure::new(
                    code::POLICY_UNAVAILABLE,
                    "Pi did not start an empty, temporary title session.",
                ));
            }
            let selected = session
                .call(json!({"type":"set_model","provider":provider,"modelId":id}))
                .await
                .map_err(|_| title_route_unavailable())?;
            if model_id(&selected).as_deref() != Some(model) {
                return Err(title_route_unavailable());
            }
            session
                .call(json!({"type":"set_thinking_level","level":"off"}))
                .await?;
            let temporary = session.call(json!({"type":"get_state"})).await?;
            let live = tokio::time::timeout(Duration::from_secs(2), live_state)
                .await
                .ok()
                .and_then(Result::ok)
                .ok_or_else(title_route_unavailable)?;
            let effective = &live["model"];
            if model_id(effective).as_deref() != Some(model)
                || effective["baseUrl"].as_str().is_none_or(str::is_empty)
                || effective["api"].as_str().is_none_or(str::is_empty)
                || effective != &temporary["model"]
                || PiTitleConfig::read().await.as_ref() != Some(&self.config)
            {
                return Err(title_route_unavailable());
            }
            session
                .prompt(PromptParams {
                    text: request.prompt,
                    turn_id: "title".into(),
                    ..Default::default()
                })
                .await?;

            let mut answer = String::new();
            while let Some(event) = events.recv().await {
                match event {
                    HarnessEvent::TextFinal { text, .. } => answer = text,
                    HarnessEvent::TurnCompleted { .. } => return parse_title(&answer),
                    HarnessEvent::TurnFailed { error, .. } => return Err(error),
                    HarnessEvent::TurnAborted { .. } | HarnessEvent::SessionEnded { .. } => break,
                    HarnessEvent::ItemStarted { .. }
                    | HarnessEvent::ApprovalRequested { .. }
                    | HarnessEvent::QuestionAsked { .. } => {
                        return Err(HarnessFailure::new(
                            code::PROTOCOL,
                            "The Pi title request attempted to use a tool or ask a question.",
                        ));
                    }
                    _ => {}
                }
            }
            Err(HarnessFailure::new(
                code::CLOSED,
                "The Pi title request ended without a title.",
            ))
        })
        .await
        .map_err(|_| HarnessFailure::new(code::TIMEOUT, "The Pi title request timed out."))?
    }
}

fn pi_commands_from(result: Value) -> HarnessResult<HarnessCommands> {
    let items = result["commands"].as_array().ok_or_else(|| {
        HarnessFailure::new(code::PROTOCOL, "Pi did not return a command catalog.")
    })?;
    let mut found = HarnessCommands::default();
    let mut names = HashSet::new();
    for value in items {
        let Some(name) = value["name"].as_str().map(str::trim) else {
            continue;
        };
        let is_skill = match value["source"].as_str() {
            Some("skill") => true,
            Some("prompt") => false,
            // The chat process also disables extensions.
            _ => continue,
        };
        let name = if is_skill {
            name.strip_prefix("skill:").unwrap_or(name)
        } else {
            name
        };
        if name.is_empty()
            || name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
            || !names.insert((name, is_skill))
        {
            continue;
        }
        let command = HarnessCommand {
            name: name.to_owned(),
            description: value["description"]
                .as_str()
                .map(str::trim)
                .filter(|description| !description.is_empty())
                .map(str::to_owned),
            argument_hint: None,
        };
        if is_skill {
            found.skills.push(command);
        } else {
            found.commands.push(command);
        }
    }
    Ok(found)
}

impl ExternalHarness for PiHarness {
    fn commands(&self, cwd: PathBuf) -> BoxFuture<'static, HarnessResult<HarnessCommands>> {
        let harness = self.clone();
        Box::pin(async move { harness.workspace_commands(&cwd).await })
    }

    fn discover(&self) -> BoxFuture<'static, HarnessResult<Discovery>> {
        let harness = self.clone();
        Box::pin(async move { Ok(harness.discovery().await.0) })
    }

    fn discover_with_status(
        &self,
    ) -> BoxFuture<'static, HarnessResult<(Discovery, VersionStatus)>> {
        let harness = self.clone();
        Box::pin(async move { Ok(harness.discovery().await) })
    }

    fn start_or_resume(&self, params: StartParams) -> BoxFuture<'static, HarnessResult<Started>> {
        self.start_with_access(params, hoh_external_harness::DEFAULT_ACCESS)
    }

    fn start_with_access(
        &self,
        params: StartParams,
        access: AccessMode,
    ) -> BoxFuture<'static, HarnessResult<Started>> {
        let harness = self.clone();
        Box::pin(async move { harness.start(params, access).await })
    }

    fn launch_flags(&self, _policy: &HarnessPolicy, access: AccessMode) -> Vec<String> {
        if access != AccessMode::Full {
            return Vec::new();
        }
        vec![
            "--no-approve".into(),
            "--no-extensions".into(),
            "--no-mcp".into(),
        ]
    }

    fn effort_levels(&self) -> &'static [&'static str] {
        EFFORTS
    }

    fn stop_ends_process(&self) -> bool {
        true
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleOutput {
    title: String,
}

fn parse_title(answer: &str) -> HarnessResult<String> {
    serde_json::from_str::<TitleOutput>(answer)
        .ok()
        .map(|answer| answer.title.trim().to_owned())
        .filter(|title| {
            !title.is_empty() && title.len() <= 256 && !title.chars().any(char::is_control)
        })
        .ok_or_else(|| HarnessFailure::new(code::PROTOCOL, "Pi returned an invalid chat title."))
}

fn ensure_policy(params: &StartParams, access: AccessMode) -> HarnessResult<()> {
    if params.interaction_mode == InteractionMode::Plan {
        return Err(HarnessFailure::new(
            code::POLICY_UNAVAILABLE,
            "Pi Agent does not support Plan mode in HoH.",
        ));
    }
    if access != AccessMode::Full {
        return Err(HarnessFailure::new(code::POLICY_UNAVAILABLE, FULL_ACCESS));
    }
    if params.policy.sandbox != SandboxMode::FullAccess
        || params.policy.network != NetworkAccess::Allowed
        || params.policy.approval == ApprovalMode::Never
    {
        return Err(HarnessFailure::new(
            code::POLICY_UNAVAILABLE,
            "Pi Agent cannot enforce a filesystem or network sandbox.",
        ));
    }
    Ok(())
}

fn model_id(model: &Value) -> Option<String> {
    let provider = model["provider"]
        .as_str()
        .filter(|s| !s.is_empty() && !s.contains('/'))?;
    let id = model["id"].as_str().filter(|s| !s.is_empty())?;
    Some(format!("{provider}/{id}"))
}

fn version_status(version: &str) -> VersionStatus {
    let parts = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>();
    match parts.as_deref() {
        Ok([1, 6, patch]) if *patch >= 1 => VersionStatus::Supported,
        Ok([_, _, _]) => VersionStatus::Unsupported,
        _ => VersionStatus::Unknown,
    }
}

fn valid_session_id(id: &str) -> bool {
    // Pi generates UUIDs for new sessions; accept only that form before passing one back as argv.
    id.len() == 36
        && id.bytes().enumerate().all(|(i, byte)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn missing_session() -> HarnessFailure {
    HarnessFailure::new(
        code::SESSION_NOT_FOUND,
        "Pi could not find the previous session in this folder.",
    )
}
