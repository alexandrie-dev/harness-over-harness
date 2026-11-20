//! [`ClaudeHarness`]: the installed `claude` CLI as an [`ExternalHarness`].
//!
//! The launch line and the mapping of HoH's policy to Claude's permission mode follow T3 Code
//! `ClaudeAdapterV2.ts` (`permissionModeForClaudeRuntimePolicy`, `claudeRuntimeQueryPolicy…`);
//! discovery follows `ClaudeProvider.ts` (`claudeAuthStatus`, capabilities probe).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_external_harness::{
    ApprovalMode, AuthState, Discovery, ExternalHarness, HarnessFailure, HarnessModel,
    HarnessPolicy, HarnessResult, NetworkAccess, ProcessSpec, SandboxMode, StartParams,
    StartResult, Started, Supervisor, SupervisorLimits, code,
};
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::session::{ClaudeSession, SessionSettings};
use crate::version;

/// How much the human let Claude do, chosen in the window and handed over by the slot's
/// configuration; nothing else (the policy of a session, the CLI's answers) decides it.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// With confirmation: every command, write and network call the CLI does not settle itself
    /// is a question to the slot.
    #[default]
    Confirm,
    /// Full access: no questions for an ordinary session (a read-only one stays read-only).
    Full,
}

/// How a [`ClaudeHarness`] finds and starts the CLI.
#[derive(Clone)]
pub struct ClaudeConfig {
    /// Confirm (default) or full access.
    pub access: Access,
    /// The `claude` program (a name looked up in `PATH`, or a path).
    pub executable: PathBuf,
    /// Added to the environment of every `claude` process.
    pub extra_env: Vec<(String, String)>,
    /// How long `--version` may take.
    pub version_timeout: Duration,
    /// How long the CLI may take to answer `initialize`.
    pub init_timeout: Duration,
    /// Events a slow reader may leave waiting.
    pub event_queue: usize,
    /// How long a turn may stay open after an `interrupt` the CLI does not follow with a result
    /// (T3 bounds an unacknowledged stop at 10 s).
    pub interrupt_timeout: Duration,
}

impl std::fmt::Debug for ClaudeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Environment values may be secrets: only the names are shown.
        f.debug_struct("ClaudeConfig")
            .field("access", &self.access)
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
            .field("init_timeout", &self.init_timeout)
            .field("event_queue", &self.event_queue)
            .field("interrupt_timeout", &self.interrupt_timeout)
            .finish()
    }
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            access: Access::Confirm,
            executable: PathBuf::from("claude"),
            extra_env: Vec::new(),
            version_timeout: Duration::from_secs(20),
            init_timeout: Duration::from_secs(25),
            event_queue: 256,
            interrupt_timeout: Duration::from_secs(10),
        }
    }
}

impl ClaudeConfig {
    /// The command line a session is started with, program first: exactly what is run for these
    /// arguments, so the window can show the mode from it. `native_id` is the id of a new session
    /// (`--session-id`), unless `resume` names one to continue (`--resume`).
    pub fn argv(
        &self,
        policy: &HarnessPolicy,
        model: Option<&str>,
        resume: Option<&str>,
        native_id: &str,
    ) -> Vec<String> {
        let launch = launch_for(policy, self.access);
        let mut argv = vec![self.executable.to_string_lossy().into_owned()];
        argv.extend(session_args(&launch, model, resume, native_id));
        argv
    }
}

pub struct ClaudeHarness {
    config: ClaudeConfig,
}

impl ClaudeHarness {
    pub fn new(config: ClaudeConfig) -> Self {
        Self { config }
    }

    /// [`ClaudeConfig::argv`] of this harness.
    pub fn argv(
        &self,
        policy: &HarnessPolicy,
        model: Option<&str>,
        resume: Option<&str>,
        native_id: &str,
    ) -> Vec<String> {
        self.config.argv(policy, model, resume, native_id)
    }
}

/// `approval: never` (no question inside the folder by a list) is not accepted yet: it is a
/// separate package. Until then nothing in this crate answers "yes" by itself.
const NEVER_REFUSED: &str =
    "Этот режим пока недоступен для Claude: действия идут через подтверждение";

/// The read-only tool set of T3 Code (`CLAUDE_READ_ONLY_ALLOWED_TOOLS`) under `dontAsk`.
const READ_ONLY_TOOLS: &str = "Read,Glob,Grep,LS,NotebookRead,TodoWrite";

/// Claude's switches for one policy.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Launch {
    pub permission_mode: &'static str,
    /// `can_use_tool` over stdio: every request the mode does not settle reaches HoH.
    pub permission_prompt: bool,
    /// `--allow-dangerously-skip-permissions`, only with `bypassPermissions` (full access).
    pub bypass: bool,
    /// Read-only: the only tools the CLI is given (`--tools`).
    pub tools: Option<&'static str>,
    pub disallowed_tools: Option<&'static str>,
}

/// `permissionModeForClaudeRuntimePolicy`, T3 `ClaudeAdapterV2.ts:3358-3407`, reduced to the
/// contract's policy: `default` + `can_use_tool`, so every command, write and network call the CLI
/// does not settle itself is a question to the slot, which has no automatic "yes" here.
/// `bypassPermissions` is never used. Read-only is `default` too, with `--tools` limited to the
/// read tools (the CLI is not given the others at all) and no `--allowedTools`: a read outside the
/// folder is a question like any other. `approval: never` is refused before this point ([`NEVER_REFUSED`]).
pub(crate) fn launch_for(policy: &HarnessPolicy, access: Access) -> Launch {
    if access == Access::Full && policy.sandbox == SandboxMode::ReadOnly {
        // Full access does not widen a read-only session (a reviewer stays one): the read tools
        // only, as T3 does for read-only (`ClaudeAdapterV2.ts:3380-3386`, `dontAsk` + tools), and
        // no questions.
        return Launch {
            permission_mode: "dontAsk",
            permission_prompt: false,
            bypass: false,
            tools: Some(READ_ONLY_TOOLS),
            disallowed_tools: None,
        };
    }
    if access == Access::Full {
        // The human's choice of full access widens an ordinary session to `bypassPermissions`
        // with `allowDangerouslySkipPermissions` (the flags T3 uses for `never` with a full
        // sandbox, `ClaudeAdapterV2.ts:3364-3378`, `:3417-3468`; our choice of mode replaces
        // `never`): no permission callback, no tool limits. The network of the policy does not
        // narrow it either (`WebFetch`/`WebSearch` are allowed: it is the human's choice).
        // The callback stays: what even bypass refuses (protected paths, removal safety) reaches
        // the human, as in T3 (`:3466-3467`, `:12494`).
        return Launch {
            permission_mode: "bypassPermissions",
            permission_prompt: true,
            bypass: true,
            tools: None,
            disallowed_tools: None,
        };
    }
    let read_only = policy.sandbox == SandboxMode::ReadOnly;
    Launch {
        permission_mode: "default",
        permission_prompt: true,
        bypass: false,
        tools: read_only.then_some(READ_ONLY_TOOLS),
        // The CLI has no network switch; without network access its two network tools stay off.
        disallowed_tools: (policy.network == NetworkAccess::Denied).then_some("WebFetch,WebSearch"),
    }
}

fn base_args() -> Vec<String> {
    [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
    ]
    .map(str::to_owned)
    .to_vec()
}

pub(crate) fn session_args(
    launch: &Launch,
    model: Option<&str>,
    resume: Option<&str>,
    new_id: &str,
) -> Vec<String> {
    let mut args = base_args();
    args.extend(["--permission-mode".into(), launch.permission_mode.into()]);
    // No settings file decides for the slot: the user's own `allow: Bash(*)` would let the CLI run
    // a command without `can_use_tool`. An empty list loads none (managed policy still applies).
    args.extend(["--setting-sources".into(), String::new()]);
    if launch.permission_prompt {
        args.extend(["--permission-prompt-tool".into(), "stdio".into()]);
    }
    if launch.bypass {
        args.push("--allow-dangerously-skip-permissions".into());
    }
    if let Some(tools) = launch.tools {
        // `--tools` removes every other tool from the model's reach (T3 `:3436-3466`). No
        // `--allowedTools`: that would answer "yes" to a read of any file without a question.
        args.extend(["--tools".into(), tools.into()]);
    }
    if let Some(tools) = launch.disallowed_tools {
        args.extend(["--disallowedTools".into(), tools.into()]);
    }
    if let Some(model) = model.filter(|model| *model != "default") {
        args.extend(["--model".into(), model.into()]);
    }
    match resume {
        Some(id) => args.extend(["--resume".into(), id.into()]),
        None => args.extend(["--session-id".into(), new_id.into()]),
    }
    args
}

/// A capabilities probe starts nothing: no stored session, no MCP servers (T3
/// `buildClaudeCapabilitiesProbeQueryOptions`, `ClaudeProvider.ts:271-306`).
fn probe_args() -> Vec<String> {
    let mut args = base_args();
    args.extend(
        [
            "--permission-mode",
            "default",
            "--permission-prompt-tool",
            "stdio",
            "--setting-sources",
            "",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
        ]
        .map(str::to_owned),
    );
    args
}

fn settings_of(config: &ClaudeConfig, bypass: bool) -> SessionSettings {
    SessionSettings {
        event_queue: config.event_queue,
        init_timeout: config.init_timeout,
        interrupt_timeout: config.interrupt_timeout,
        config_dir: config
            .extra_env
            .iter()
            .find(|(name, _)| name == "CLAUDE_CONFIG_DIR")
            .map(|(_, value)| value.clone()),
        bypass,
    }
}

fn random_uuid() -> Result<String, HarnessFailure> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| HarnessFailure::new(code::IO, format!("no randomness: {error}")))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// `claude --version`, checked against the supported range.
enum VersionCheck {
    Supported(String),
    NotInstalled,
    Refused {
        version: Option<String>,
        message: String,
    },
}

async fn check_version(config: &ClaudeConfig) -> VersionCheck {
    let mut command = tokio::process::Command::new(&config.executable);
    command
        .arg("--version")
        .envs(config.extra_env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            return VersionCheck::NotInstalled;
        }
        Err(error) => {
            return VersionCheck::Refused {
                version: None,
                message: version::unrecognized_version_message(&error.to_string()),
            };
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        return VersionCheck::NotInstalled;
    };
    let mut output = String::new();
    let mut status = None;
    let read = async {
        let mut bytes = Vec::new();
        let _ = (&mut stdout).take(4096).read_to_end(&mut bytes).await;
        output = String::from_utf8_lossy(&bytes).into_owned();
        status = child.wait().await.ok();
    };
    if tokio::time::timeout(config.version_timeout, read)
        .await
        .is_err()
    {
        return VersionCheck::Refused {
            version: None,
            message: version::unrecognized_version_message("нет ответа на --version"),
        };
    }
    if let Some(status) = status.filter(|status| !status.success()) {
        // A program that fails is no proof of a version, whatever it printed.
        let shown = match status.code() {
            Some(code) => format!("код выхода {code}"),
            None => "завершён сигналом".to_owned(),
        };
        return VersionCheck::Refused {
            version: None,
            message: version::unrecognized_version_message(&shown),
        };
    }
    match version::parse_version(&output) {
        None => VersionCheck::Refused {
            version: None,
            message: version::unrecognized_version_message(&output),
        },
        Some(parsed) if version::is_supported(parsed) => {
            VersionCheck::Supported(version::format_version(parsed))
        }
        Some(parsed) => {
            let shown = version::format_version(parsed);
            VersionCheck::Refused {
                message: version::unsupported_version_message(&shown),
                version: Some(shown),
            }
        }
    }
}

fn spec(config: &ClaudeConfig, args: Vec<String>, cwd: PathBuf, probe: bool) -> ProcessSpec {
    let mut env = config.extra_env.clone();
    if probe {
        // T3 `buildClaudeCapabilitiesProbeQueryOptions`: the probe connects to nothing.
        env.extend([
            ("ENABLE_CLAUDEAI_MCP_SERVERS".to_owned(), "false".to_owned()),
            ("CLAUDE_CODE_AUTO_CONNECT_IDE".to_owned(), "0".to_owned()),
            (
                "CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL".to_owned(),
                "1".to_owned(),
            ),
        ]);
    }
    ProcessSpec {
        program: config.executable.clone(),
        args,
        cwd: Some(cwd),
        env,
        env_remove: Vec::new(),
    }
}

async fn spawn(spec: &ProcessSpec) -> Result<Supervisor, HarnessFailure> {
    // A missing folder fails the launch with the same `NotFound` as a missing program.
    if let Some(cwd) = &spec.cwd
        && !tokio::fs::metadata(cwd)
            .await
            .is_ok_and(|meta| meta.is_dir())
    {
        return Err(HarnessFailure::new(
            code::IO,
            format!("Рабочая папка «{}» не найдена.", cwd.display()),
        ));
    }
    Supervisor::spawn(spec, SupervisorLimits::default(), None).map_err(|failure| {
        if failure.code == code::NOT_INSTALLED {
            HarnessFailure::new(code::NOT_INSTALLED, version::not_installed_message())
        } else {
            failure
        }
    })
}

/// `claudeAuthStatus`, T3 `ClaudeProvider.ts:213-228`: only `tokenSource: "none"` disproves a
/// sign-in, and an API key or an account field proves it anyway. Nothing but the answer of the
/// CLI's own `initialize` is read.
pub(crate) fn auth_from(account: Option<&Value>) -> AuthState {
    let Some(account) = account.filter(|account| account.is_object()) else {
        return AuthState::Unknown;
    };
    let field = |key: &str| account.get(key).and_then(Value::as_str);
    if field("apiProvider").is_some_and(|provider| provider != "firstParty") {
        return AuthState::Authenticated;
    }
    if field("tokenSource") != Some("none") {
        return AuthState::Authenticated;
    }
    let has_key = field("apiKeySource").is_some_and(|source| source != "none");
    if has_key || field("email").is_some() || field("subscriptionType").is_some() {
        AuthState::Authenticated
    } else {
        AuthState::Unauthenticated
    }
}

/// The models `initialize` reported, `default` first. Claude Code's own aliases stand in when it
/// reports none (T3 keeps a bundled catalog for the same reason, `ClaudeModelCatalog.ts`).
pub(crate) fn models_from(response: &Value) -> Vec<HarnessModel> {
    let mut models = vec![HarnessModel {
        id: "default".into(),
        display_name: Some("По умолчанию (модель Claude Code)".into()),
        is_default: true,
    }];
    let reported: Vec<HarnessModel> = response
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("value")?.as_str()?.trim();
            // `default` is Claude Code's pointer at its own default, not a model.
            (!id.is_empty() && id != "default").then(|| HarnessModel {
                id: id.to_owned(),
                display_name: model
                    .get("displayName")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                is_default: false,
            })
        })
        .collect();
    if reported.is_empty() {
        models.extend(["opus", "sonnet", "haiku"].map(|alias| HarnessModel {
            id: alias.into(),
            display_name: None,
            is_default: false,
        }));
    } else {
        models.extend(reported);
    }
    models
}

impl ExternalHarness for ClaudeHarness {
    fn discover(&self) -> BoxFuture<'static, HarnessResult<Discovery>> {
        let config = self.config.clone();
        Box::pin(async move {
            let version = match check_version(&config).await {
                VersionCheck::Supported(version) => version,
                VersionCheck::NotInstalled => {
                    return Ok(Discovery {
                        installed: false,
                        version: None,
                        auth: AuthState::Unknown,
                        models: Vec::new(),
                        detail: Some(version::not_installed_message()),
                    });
                }
                VersionCheck::Refused { version, message } => {
                    return Ok(Discovery {
                        installed: true,
                        version,
                        auth: AuthState::Unknown,
                        models: Vec::new(),
                        detail: Some(hoh_external_harness::redact::redact(&message)),
                    });
                }
            };
            let probe = spec(&config, probe_args(), std::env::temp_dir(), true);
            let started = match spawn(&probe).await {
                Ok(sup) => {
                    ClaudeSession::start(
                        sup,
                        String::new(),
                        std::env::temp_dir(),
                        &version,
                        settings_of(&config, false),
                    )
                    .await
                }
                Err(failure) => Err(failure),
            };
            let (session, _events, init) = match started {
                Ok(started) => started,
                Err(failure) => {
                    return Ok(Discovery {
                        installed: true,
                        version: Some(version),
                        auth: AuthState::Unknown,
                        models: Vec::new(),
                        detail: Some(hoh_external_harness::redact::redact(&failure.message)),
                    });
                }
            };
            session.terminate_now().await;
            let auth = auth_from(init.response.get("account"));
            Ok(Discovery {
                installed: true,
                version: Some(version),
                auth,
                models: models_from(&init.response),
                detail: (auth == AuthState::Unauthenticated).then(|| {
                    "Вход в Claude Code не выполнен: запустите `claude` в терминале и войдите."
                        .to_owned()
                }),
            })
        })
    }

    fn start_or_resume(&self, params: StartParams) -> BoxFuture<'static, HarnessResult<Started>> {
        let config = self.config.clone();
        Box::pin(async move {
            // In full access nothing is asked by definition, so `never` changes nothing there.
            if params.policy.approval == ApprovalMode::Never && config.access == Access::Confirm {
                return Err(HarnessFailure::new(code::ESCALATION_DENIED, NEVER_REFUSED));
            }
            // No chat with a version outside the range (the CEO's condition).
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
            let resume = params.resume.as_ref().map(|id| String::from(id.clone()));
            let native = match &resume {
                Some(id) => id.clone(),
                None => random_uuid()?,
            };
            let args = config
                .argv(
                    &params.policy,
                    params.model.as_deref(),
                    resume.as_deref(),
                    &native,
                )
                .split_off(1);
            let cwd = PathBuf::from(&params.cwd);
            let sup = spawn(&spec(&config, args, cwd.clone(), false)).await?;
            let (session, events, _init) = ClaudeSession::start(
                sup,
                native.clone(),
                cwd,
                &version,
                settings_of(&config, launch_for(&params.policy, config.access).bypass),
            )
            .await?;
            Ok(Started {
                result: StartResult {
                    native_session_id: native.into(),
                    resumed: resume.is_some(),
                },
                session: Arc::clone(&session) as Arc<dyn hoh_external_harness::HarnessSession>,
                events,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_missing_token_without_other_proof_is_signed_out() {
        let cases = [
            (
                json!({ "tokenSource": "claude.ai" }),
                AuthState::Authenticated,
            ),
            (json!({ "tokenSource": "none" }), AuthState::Unauthenticated),
            (
                json!({ "tokenSource": "none", "apiKeySource": "env" }),
                AuthState::Authenticated,
            ),
            (
                json!({ "tokenSource": "none", "email": "a@b.c" }),
                AuthState::Authenticated,
            ),
            (
                json!({ "tokenSource": "none", "apiProvider": "bedrock" }),
                AuthState::Authenticated,
            ),
            (json!({}), AuthState::Authenticated),
        ];
        for (account, expected) in cases {
            assert_eq!(auth_from(Some(&account)), expected, "{account}");
        }
        assert_eq!(auth_from(None), AuthState::Unknown);
    }

    #[test]
    fn models_start_with_default_and_fall_back_to_aliases() {
        let reported = json!({ "models": [{ "value": "default" }, { "value": "opus", "displayName": "Opus" }] });
        let ids: Vec<_> = models_from(&reported).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["default", "opus"]);
        let ids: Vec<_> = models_from(&json!({})).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["default", "opus", "sonnet", "haiku"]);
    }

    #[test]
    fn no_policy_gives_the_cli_a_way_around_the_questions() {
        for sandbox in [
            SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite,
            SandboxMode::FullAccess,
        ] {
            for approval in [ApprovalMode::OnRequest, ApprovalMode::Untrusted] {
                for network in [NetworkAccess::Denied, NetworkAccess::Allowed] {
                    let launch = launch_for(
                        &HarnessPolicy {
                            sandbox,
                            approval,
                            network,
                        },
                        Access::Confirm,
                    );
                    assert_eq!(launch.permission_mode, "default");
                    let args = session_args(&launch, None, None, "id");
                    let at = args
                        .iter()
                        .position(|a| a == "--permission-prompt-tool")
                        .unwrap();
                    assert_eq!(args[at + 1], "stdio");
                    assert!(!args.iter().any(|a| a.contains("bypass")
                        || a.contains("dangerously")
                        || a == "acceptEdits"
                        || a == "dontAsk"
                        || a == "--allowedTools"));
                }
            }
        }
    }

    #[test]
    fn full_access_is_bypass_permissions_with_the_callback_kept_for_what_bypass_refuses() {
        let config = ClaudeConfig {
            access: Access::Full,
            ..ClaudeConfig::default()
        };
        let argv = config.argv(&HarnessPolicy::default(), None, None, "id");
        let at = argv.iter().position(|a| a == "--permission-mode").unwrap();
        assert_eq!(argv[at + 1], "bypassPermissions");
        assert!(argv.contains(&"--allow-dangerously-skip-permissions".to_owned()));
        assert!(argv.contains(&"--permission-prompt-tool".to_owned()));
        assert!(!argv.contains(&"--disallowedTools".to_owned()));
        assert!(!argv.contains(&"--tools".to_owned()));
        // The default is confirm.
        assert_eq!(ClaudeConfig::default().access, Access::Confirm);
    }

    #[test]
    fn the_probe_does_not_read_the_settings_of_the_user_either() {
        let args = probe_args();
        let at = args.iter().position(|a| a == "--setting-sources").unwrap();
        assert_eq!(args[at + 1], "");
    }

    #[test]
    fn default_model_adds_no_model_flag() {
        let launch = launch_for(&HarnessPolicy::default(), Access::Confirm);
        assert!(!session_args(&launch, Some("default"), None, "id").contains(&"--model".into()));
        assert!(session_args(&launch, Some("opus"), None, "id").contains(&"--model".into()));
    }
}
