//! The locally installed Pi Agent over its native JSONL RPC protocol.

mod session;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_external_harness::{
    AccessMode, ApprovalMode, AuthState, Discovery, ExternalHarness, ExternalHarnesses,
    HarnessEffort, HarnessFailure, HarnessModel, HarnessPolicy, HarnessResult, InteractionMode,
    NetworkAccess, ProcessSpec, SandboxMode, StartParams, StartResult, Started, VersionStatus,
    code,
};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_loader::ProfileContextKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use session::PiSession;

const TIMEOUT: Duration = Duration::from_secs(20);
const INSTALL: &str = "Pi CLI (pi) was not found on PATH. Install @earendil-works/pi-coding-agent.";
const FULL_ACCESS: &str = "Pi Agent requires Full access. Choose Full access in the composer.";
const EFFORTS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

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
        let version = self.version().await?;
        if version_status(&version) != VersionStatus::Supported {
            return Err(HarnessFailure::new(
                code::UNSUPPORTED_VERSION,
                "HoH requires Pi CLI 1.6.1 or later in the 1.6 series.",
            ));
        }
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
        let (session, events) = PiSession::spawn(&spec)?;
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

impl ExternalHarness for PiHarness {
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
