//! Discovery and process launch for the installed Codex CLI.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_external_harness::{
    AccessMode, AuthState, Discovery, ExternalHarness, HarnessEffort, HarnessFailure, HarnessModel,
    HarnessPolicy, HarnessResult, ProcessSpec, StartParams, StartResult, Started, Supervisor,
    SupervisorLimits, VersionStatus, code,
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

enum VersionCheck {
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

async fn check_version(config: &CodexConfig) -> VersionCheck {
    let mut command = tokio::process::Command::new(&config.executable);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .envs(config.extra_env.iter().map(|(name, value)| (name, value)));
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

fn app_server_spec(config: &CodexConfig, cwd: &Path) -> ProcessSpec {
    ProcessSpec {
        program: config.executable.clone(),
        args: vec![
            "app-server".to_owned(),
            "-c".to_owned(),
            "thread_unload_delay_secs=0".to_owned(),
        ],
        cwd: Some(cwd.to_owned()),
        env: config.extra_env.clone(),
        env_remove: Vec::new(),
    }
}

fn spawn(config: &CodexConfig, cwd: &Path) -> HarnessResult<Supervisor> {
    Supervisor::spawn(&app_server_spec(config, cwd), config.limits, None)
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
            },
            status,
        ),
    }
}

impl ExternalHarness for CodexHarness {
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
        let config = self.config.clone();
        Box::pin(async move {
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
            let supervisor = spawn(&config, &cwd)?;
            let resume = params.resume.as_ref().map(|id| String::from(id.clone()));
            let started = CodexSession::start(
                supervisor,
                StartSettings {
                    event_queue: config.event_queue,
                    request_timeout: config.request_timeout,
                    mode: mode_settings(access),
                    cwd: params.cwd,
                    model: params.model,
                    effort: params.effort,
                    resume,
                },
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
        ]
    }

    fn stop_ends_process(&self) -> bool {
        true
    }

    fn effort_levels(&self) -> &'static [&'static str] {
        EFFORT_LEVELS
    }
}
