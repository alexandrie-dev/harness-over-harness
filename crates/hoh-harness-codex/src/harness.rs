//! Discovery and process launch for the installed Codex CLI.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_external_harness::{
    AccessMode, AuthState, Discovery, ExecutorLimitWindow, ExternalHarness, HarnessCommand,
    HarnessCommands, HarnessEffort, HarnessFailure, HarnessModel, HarnessPolicy, HarnessResult,
    ProcessSpec, StartParams, StartResult, Started, Supervisor, SupervisorLimits, VersionStatus,
    code,
};
use serde_json::Value;

use crate::session::{CodexSession, StartSettings, mode_settings};
use crate::version;

#[derive(Clone)]
pub struct CodexConfig {
    pub executable: PathBuf,
    pub extra_env: Vec<(String, String)>,
    pub version_timeout: Duration,
    pub request_timeout: Duration,
    pub event_queue: usize,
    pub limits: SupervisorLimits,
}

impl std::fmt::Debug for CodexConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexConfig")
            .field("executable", &self.executable)
            .field(
                "extra_env",
                &self
                    .extra_env
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .field("version_timeout", &self.version_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("event_queue", &self.event_queue)
            .field("limits", &self.limits)
            .finish()
    }
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("codex"),
            extra_env: Vec::new(),
            version_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(30),
            event_queue: 256,
            limits: SupervisorLimits::default(),
        }
    }
}

pub struct CodexHarness {
    config: CodexConfig,
}

impl CodexHarness {
    pub fn new(config: CodexConfig) -> Self {
        Self { config }
    }
}

pub(crate) enum VersionCheck {
    Supported(String),
    NotInstalled,
    Refused {
        version: Option<String>,
        message: String,
    },
}

impl VersionCheck {
    fn status(&self) -> VersionStatus {
        match self {
            Self::Supported(_) => VersionStatus::Supported,
            Self::Refused {
                version: Some(_), ..
            } => VersionStatus::Unsupported,
            Self::NotInstalled | Self::Refused { version: None, .. } => VersionStatus::Unknown,
        }
    }
}

pub(crate) async fn check_version(config: &CodexConfig) -> VersionCheck {
    let mut command = tokio::process::Command::new(&config.executable);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .envs(
            config
                .extra_env
                .iter()
                .filter(|(name, _)| !hoh_external_harness::supervisor::is_telegram_env(name))
                .map(|(name, value)| (name, value)),
        );
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("TELEGRAM_") {
            command.env_remove(name);
        }
    }
    let output = match tokio::time::timeout(config.version_timeout, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            return VersionCheck::NotInstalled;
        }
        Ok(Err(error)) => {
            return VersionCheck::Refused {
                version: None,
                message: format!("Codex CLI не запускается: {error}"),
            };
        }
        Err(_) => {
            return VersionCheck::Refused {
                version: None,
                message: "Codex CLI не ответил на --version".to_owned(),
            };
        }
    };
    let mut banner = String::from_utf8_lossy(&output.stdout).into_owned();
    if banner.trim().is_empty() {
        banner = String::from_utf8_lossy(&output.stderr).into_owned();
    }
    let Some(parsed) = version::parse_version(&banner) else {
        return VersionCheck::Refused {
            version: None,
            message: version::unrecognized_version_message(&banner),
        };
    };
    let shown = version::format_version(parsed);
    if !output.status.success() || !version::is_supported(parsed) {
        return VersionCheck::Refused {
            version: Some(shown.clone()),
            message: version::unsupported_version_message(&shown),
        };
    }
    VersionCheck::Supported(shown)
}

pub(crate) fn projects_config(cwd: &Path, trusted: bool) -> String {
    format!(
        "projects={{{}}}",
        crate::session::project_trust(cwd, trusted)
            .keys()
            .map(|path| format!(
                "{}={{trust_level=\"{}\"}}",
                serde_json::to_string(path).expect("path serializes"),
                if trusted { "trusted" } else { "untrusted" }
            ))
            .collect::<Vec<_>>()
            .join(",")
    )
}

pub(crate) fn app_server_spec(config: &CodexConfig, cwd: &Path, trusted: bool) -> ProcessSpec {
    ProcessSpec {
        program: config.executable.clone(),
        args: vec![
            "app-server".to_owned(),
            "-c".to_owned(),
            "thread_unload_delay_secs=0".to_owned(),
            "-c".to_owned(),
            projects_config(cwd, trusted),
        ],
        cwd: Some(cwd.to_owned()),
        cwd_identity: None,
        env: config.extra_env.clone(),
        env_remove: Vec::new(),
    }
}

#[cfg(test)]
mod folder_trust_tests {
    use super::*;

    #[test]
    fn app_server_receives_explicit_project_trust() {
        let spec = app_server_spec(&CodexConfig::default(), Path::new("/project"), false);
        assert!(
            spec.args
                .iter()
                .any(|arg| arg.contains("\"/project\"={trust_level=\"untrusted\"}"))
        );
        assert!(
            spec.args
                .iter()
                .any(|arg| arg.contains("\"/\"={trust_level=\"untrusted\"}"))
        );
        let trusted = app_server_spec(&CodexConfig::default(), Path::new("/project"), true);
        assert!(
            trusted
                .args
                .iter()
                .any(|arg| arg.contains("\"/project\"={trust_level=\"trusted\"}"))
        );
    }
}

fn spawn(config: &CodexConfig, cwd: &Path) -> HarnessResult<Supervisor> {
    Supervisor::spawn(&app_server_spec(config, cwd, false), config.limits, None)
}

fn auth_from(account: &Value) -> AuthState {
    if account
        .get("account")
        .is_some_and(|account| !account.is_null())
    {
        AuthState::Authenticated
    } else if account.get("requiresOpenaiAuth").and_then(Value::as_bool) == Some(true) {
        AuthState::Unauthenticated
    } else {
        AuthState::Unknown
    }
}

fn models_from(values: Vec<Value>) -> Vec<HarnessModel> {
    values
        .into_iter()
        .filter(|value| value.get("hidden").and_then(Value::as_bool) != Some(true))
        .filter_map(|value| {
            let id = value
                .get("model")
                .or_else(|| value.get("id"))?
                .as_str()?
                .trim()
                .to_owned();
            if id.is_empty() {
                return None;
            }
            let effort_levels = value
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|effort| {
                    Some(HarnessEffort {
                        id: effort.get("reasoningEffort")?.as_str()?.to_owned(),
                        description: effort
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect();
            Some(HarnessModel {
                id,
                display_name: value
                    .get("displayName")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                is_default: value
                    .get("isDefault")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                effort_levels,
                default_effort: value
                    .get("defaultReasoningEffort")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

async fn discover_codex(config: &CodexConfig) -> (Discovery, VersionStatus) {
    let checked = check_version(config).await;
    let status = checked.status();
    let version = match checked {
        VersionCheck::Supported(version) => version,
        VersionCheck::NotInstalled => {
            return (
                Discovery {
                    installed: false,
                    version: None,
                    auth: AuthState::Unknown,
                    models: Vec::new(),
                    detail: Some(version::not_installed_message()),
                    failure: None,
                },
                status,
            );
        }
        VersionCheck::Refused { version, message } => {
            return (
                Discovery {
                    installed: true,
                    version,
                    auth: AuthState::Unknown,
                    models: Vec::new(),
                    detail: Some(hoh_external_harness::redact::redact(&message)),
                    failure: None,
                },
                status,
            );
        }
    };
    let cwd = std::env::temp_dir();
    let probe = match spawn(config, &cwd) {
        Ok(supervisor) => CodexSession::probe(supervisor, config.request_timeout).await,
        Err(error) => Err(error),
    };
    match probe {
        Ok(probe) => {
            let auth = auth_from(&probe.account);
            (
                Discovery {
                    installed: true,
                    version: Some(version),
                    auth,
                    models: models_from(probe.models),
                    detail: (auth == AuthState::Unauthenticated).then(|| {
                        "Вход в Codex не выполнен: запустите `codex` в терминале и войдите."
                            .to_owned()
                    }),
                    failure: None,
                },
                status,
            )
        }
        Err(error) => (
            Discovery {
                installed: true,
                version: Some(version),
                auth: AuthState::Unknown,
                models: Vec::new(),
                detail: Some(hoh_external_harness::redact::redact(&error.message)),
                failure: None,
            },
            status,
        ),
    }
}

fn rate_limit_windows(response: &Value) -> Vec<ExecutorLimitWindow> {
    let mut windows = Vec::new();
    let snapshots = response
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object);
    let main = response.get("rateLimits");
    let main_id = main
        .and_then(|snapshot| snapshot.get("limitId"))
        .and_then(Value::as_str);
    let main_is_in_map = main_id
        .zip(snapshots)
        .is_some_and(|(id, snapshots)| snapshots.contains_key(id));
    let snapshots = main
        .filter(|_| !main_is_in_map)
        .into_iter()
        .chain(snapshots.into_iter().flat_map(|entries| entries.values()));

    for snapshot in snapshots {
        let name = snapshot.get("limitName").and_then(Value::as_str);
        let id = snapshot.get("limitId").and_then(Value::as_str);
        let name = name
            .or(id.filter(|id| *id != "codex"))
            .map(|label| {
                label
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(80)
                    .collect::<String>()
            })
            .filter(|label| !label.trim().is_empty());
        for key in ["primary", "secondary"] {
            let Some(window) = snapshot.get(key) else {
                continue;
            };
            let Some(used) = window.get("usedPercent").and_then(Value::as_u64) else {
                continue;
            };
            let remaining_percent = 100_u8.saturating_sub(used.min(100) as u8);
            let window_minutes = window
                .get("windowDurationMins")
                .and_then(Value::as_u64)
                .and_then(|minutes| u32::try_from(minutes).ok())
                .filter(|minutes| *minutes > 0);
            let resets_at = window
                .get("resetsAt")
                .and_then(Value::as_u64)
                .filter(|seconds| *seconds > 0 && *seconds <= 8_640_000_000_000);
            windows.push(ExecutorLimitWindow {
                name: name.clone(),
                remaining_percent,
                window_minutes,
                resets_at,
            });
        }
    }
    windows
}

fn skills_from(answer: &Value, cwd: &str) -> HarnessResult<Vec<HarnessCommand>> {
    let entries = answer["data"].as_array().ok_or_else(|| {
        HarnessFailure::new(code::PROTOCOL, "Codex did not return a skill catalog.")
    })?;
    let entry = entries
        .iter()
        .find(|entry| entry["cwd"].as_str() == Some(cwd))
        .ok_or_else(|| {
            HarnessFailure::new(code::PROTOCOL, "Codex did not list skills for this folder.")
        })?;
    let skills = entry["skills"].as_array().ok_or_else(|| {
        HarnessFailure::new(
            code::PROTOCOL,
            "Codex did not return skills for this folder.",
        )
    })?;
    let mut names = HashSet::new();
    Ok(skills
        .iter()
        .filter_map(|skill| {
            if skill["enabled"].as_bool() != Some(true) {
                return None;
            }
            let name = skill["name"].as_str()?.trim();
            if name.is_empty()
                || name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
                || !names.insert(name.to_owned())
            {
                return None;
            }
            let description = skill["shortDescription"]
                .as_str()
                .or_else(|| skill["description"].as_str())
                .map(str::trim)
                .filter(|description| !description.is_empty())
                .map(str::to_owned);
            Some(HarnessCommand {
                name: name.to_owned(),
                description,
                argument_hint: None,
            })
        })
        .collect())
}

impl ExternalHarness for CodexHarness {
    fn commands(&self, cwd: PathBuf) -> BoxFuture<'static, HarnessResult<HarnessCommands>> {
        let config = self.config.clone();
        Box::pin(async move {
            match check_version(&config).await {
                VersionCheck::Supported(_) => {}
                VersionCheck::NotInstalled => {
                    return Err(HarnessFailure::new(
                        code::NOT_INSTALLED,
                        version::not_installed_message(),
                    ));
                }
                VersionCheck::Refused { message, .. } => {
                    return Err(HarnessFailure::new(code::UNSUPPORTED_VERSION, message));
                }
            }
            let cwd_string = cwd.to_str().ok_or_else(|| {
                HarnessFailure::new(code::IO, "The workspace folder is not a UTF-8 path.")
            })?;
            // App-server accepts a cwd per catalog request. Start outside the project, with
            // the target and its ancestors explicitly untrusted, and never start a turn.
            let mut spec = app_server_spec(&config, &cwd, false);
            spec.cwd = Some(std::env::temp_dir());
            let supervisor = Supervisor::spawn(&spec, config.limits, None)?;
            let result = CodexSession::read_skills(
                supervisor,
                config.request_timeout,
                cwd_string.to_owned(),
            )
            .await?;
            Ok(HarnessCommands {
                commands: Vec::new(),
                skills: skills_from(&result, cwd_string)?,
            })
        })
    }

    fn rate_limits(&self) -> BoxFuture<'static, HarnessResult<Option<Vec<ExecutorLimitWindow>>>> {
        let config = self.config.clone();
        Box::pin(async move {
            let cwd = std::env::temp_dir();
            let supervisor = spawn(&config, &cwd)?;
            let response = CodexSession::read_rate_limits(
                supervisor,
                config.request_timeout,
                cwd.to_string_lossy().into_owned(),
            )
            .await?;
            Ok(Some(rate_limit_windows(&response)))
        })
    }

    fn discover(&self) -> BoxFuture<'static, HarnessResult<Discovery>> {
        let config = self.config.clone();
        Box::pin(async move { Ok(discover_codex(&config).await.0) })
    }

    fn discover_with_status(
        &self,
    ) -> BoxFuture<'static, HarnessResult<(Discovery, VersionStatus)>> {
        let config = self.config.clone();
        Box::pin(async move { Ok(discover_codex(&config).await) })
    }

    fn start_or_resume(&self, params: StartParams) -> BoxFuture<'static, HarnessResult<Started>> {
        self.start_with_access(params, hoh_external_harness::DEFAULT_ACCESS)
    }

    fn start_with_access(
        &self,
        params: StartParams,
        access: AccessMode,
    ) -> BoxFuture<'static, HarnessResult<Started>> {
        self.start_with_folder_trust(params, access, false)
    }

    fn start_with_folder_trust(
        &self,
        params: StartParams,
        access: AccessMode,
        trusted: bool,
    ) -> BoxFuture<'static, HarnessResult<Started>> {
        let config = self.config.clone();
        Box::pin(async move {
            if params.interaction_mode == hoh_external_harness::InteractionMode::Plan {
                return Err(HarnessFailure::new(
                    code::POLICY_UNAVAILABLE,
                    "Plan mode for Codex is coming soon",
                ));
            }
            let version = match check_version(&config).await {
                VersionCheck::Supported(version) => version,
                VersionCheck::NotInstalled => {
                    return Err(HarnessFailure::new(
                        code::NOT_INSTALLED,
                        version::not_installed_message(),
                    ));
                }
                VersionCheck::Refused { message, .. } => {
                    return Err(HarnessFailure::new(code::UNSUPPORTED_VERSION, message));
                }
            };
            let cwd = PathBuf::from(&params.cwd);
            let mut process = app_server_spec(&config, &cwd, trusted);
            process.cwd_identity = params.cwd_identity;
            let supervisor = Supervisor::spawn(&process, config.limits, None)?;
            let resume = params.resume.as_ref().map(|id| String::from(id.clone()));
            let started = CodexSession::start(
                supervisor,
                StartSettings {
                    event_queue: config.event_queue,
                    request_timeout: config.request_timeout,
                    mode: mode_settings(access),
                    cwd: params.cwd,
                    trusted,
                    model: params.model,
                    effort: params.effort,
                    resume,
                },
                config,
                &version,
            )
            .await
            .map_err(|error| {
                if error.code == code::PROTOCOL {
                    HarnessFailure::new(
                        error.code,
                        version::protocol_message(&version, &error.message),
                    )
                } else {
                    error
                }
            })?;
            let (session, events, thread_id, resumed) = started;
            Ok(Started {
                result: StartResult {
                    native_session_id: thread_id.into(),
                    resumed,
                },
                session: Arc::clone(&session) as Arc<dyn hoh_external_harness::HarnessSession>,
                events,
            })
        })
    }

    fn launch_flags(&self, _policy: &HarnessPolicy, access: AccessMode) -> Vec<String> {
        let settings = mode_settings(access);
        vec![
            format!("approval={}", settings.approval_policy),
            format!("reviewer={}", settings.reviewer),
            format!("sandbox={}", settings.sandbox),
            "project-trust=untrusted".to_owned(),
        ]
    }

    fn launch_flags_for_folder_trust(
        &self,
        policy: &HarnessPolicy,
        access: AccessMode,
        trusted: bool,
    ) -> Vec<String> {
        let mut flags = self.launch_flags(policy, access);
        if trusted {
            *flags.last_mut().expect("trust flag") = "project-trust=trusted".to_owned();
        }
        flags
    }

    fn stop_ends_process(&self) -> bool {
        true
    }

    fn accepts_effort(&self, _effort: &str) -> bool {
        // Codex's model/list is the authority. The host only offers values returned by it, and
        // app-server deliberately permits future model-specific effort names.
        true
    }
}
