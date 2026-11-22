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
    AccessMode, ApprovalMode, AuthState, DEFAULT_ACCESS, Discovery, ExternalHarness,
    HarnessFailure, HarnessModel, HarnessPolicy, HarnessResult, NetworkAccess, ProcessSpec,
    SandboxMode, StartParams, StartResult, Started, Supervisor, SupervisorLimits, VersionStatus,
    code,
};
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::session::{ClaudeSession, SessionSettings};
use crate::version;

/// Which of the human's own Claude Code settings a session runs with. Chosen in the profile (the
/// plugin's configuration, `settingSources`), never by the workspace or the chat; the access mode
/// is not one of them (the CLI is always given it on the command line).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum SettingSources {
    /// The human's own settings: those of [`PERSONAL_SETTING_SOURCES`] (their permission rules,
    /// hooks, plugins, `CLAUDE.md`); their MCP servers are not turned off. The settings of the
    /// working folder (`.claude/settings.json`, `.claude/settings.local.json`) are not among them.
    Personal,
    /// Nothing of the human's: no settings file (`--setting-sources ""`) and no MCP server
    /// (`--strict-mcp-config`, none given). What the organization manages still applies.
    Isolated,
}

/// The only place the default is set: the human's own settings apply, as in T3 (the founder and
/// the CEO, 21.11). [`SettingSources::Isolated`] stays available in the profile.
pub const DEFAULT_SETTING_SOURCES: SettingSources = SettingSources::Personal;

/// The setting sources of [`SettingSources::Personal`], the only place they are named: the
/// human's user settings only (the CEO, 21.11 ~02:00). The project and local settings live in the
/// working folder, where anyone who wrote to it (another person, the model itself) could put an
/// `allow` rule or a hook that answers in place of the window's question. The other choice (as T3
/// reads them, `ClaudeAdapterV2.ts:5595`) is this constant set to `"user,project,local"`.
const PERSONAL_SETTING_SOURCES: &str = "user";

impl SettingSources {
    /// The switches of this choice, in the order they are passed.
    fn args(self) -> Vec<String> {
        let args: &[&str] = match self {
            // `--setting-sources` loads only the listed ones (`claude --help`).
            Self::Personal => &["--setting-sources", PERSONAL_SETTING_SOURCES],
            // "Only use MCP servers from --mcp-config, ignoring all other MCP configurations"
            // (`claude --help`); none is given.
            Self::Isolated => &["--setting-sources", "", "--strict-mcp-config"],
        };
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }
}

/// How a [`ClaudeHarness`] finds and starts the CLI. It has no access mode: what a session may
/// do without asking comes only from the slot ([`ExternalHarness::start_with_access`]), which
/// takes it from the user's choice in the window.
#[derive(Clone)]
pub struct ClaudeConfig {
    /// The `claude` program (a name looked up in `PATH`, or a path).
    pub executable: PathBuf,
    /// Whose settings the CLI reads ([`DEFAULT_SETTING_SOURCES`] unless the profile says).
    pub setting_sources: SettingSources,
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
            .field("executable", &self.executable)
            .field("setting_sources", &self.setting_sources)
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
            executable: PathBuf::from("claude"),
            setting_sources: DEFAULT_SETTING_SOURCES,
            extra_env: Vec::new(),
            version_timeout: Duration::from_secs(20),
            init_timeout: Duration::from_secs(25),
            event_queue: 256,
            interrupt_timeout: Duration::from_secs(10),
        }
    }
}

impl ClaudeConfig {
    /// The command line a session is started with under `access`, program first: exactly what is
    /// run for these arguments. `native_id` is the id of a new session (`--session-id`), unless
    /// `resume` names one to continue (`--resume`). The part that decides what the session may do
    /// without asking is [`launch_flags`](Self::launch_flags) of the same policy and mode, in the
    /// same place.
    pub fn argv(
        &self,
        policy: &HarnessPolicy,
        access: AccessMode,
        model: Option<&str>,
        resume: Option<&str>,
        native_id: &str,
    ) -> Vec<String> {
        let launch = launch_for(policy, access);
        let mut argv = vec![self.executable.to_string_lossy().into_owned()];
        argv.extend(session_args(
            &launch,
            self.setting_sources,
            model,
            resume,
            native_id,
        ));
        argv
    }

    /// The switches that decide what a session started with `policy` under `access` may do
    /// without asking (permission mode, setting sources, permission prompt, tool limits): the
    /// slice of [`argv`](Self::argv) between the fixed stream flags and the model and session ids.
    /// No ids, no secrets.
    pub fn launch_flags(&self, policy: &HarnessPolicy, access: AccessMode) -> Vec<String> {
        permission_args(&launch_for(policy, access), self.setting_sources)
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
        access: AccessMode,
        model: Option<&str>,
        resume: Option<&str>,
        native_id: &str,
    ) -> Vec<String> {
        self.config.argv(policy, access, model, resume, native_id)
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
/// contract's policy and the access mode the slot passes (the human's choice in the window, never
/// the plugin's configuration). Confirm: `default` + `can_use_tool`, so every command, write and
/// network call the CLI does not settle itself is a question to the slot, which has no automatic
/// "yes" here; read-only is `default` too, with `--tools` limited to the read tools and no
/// `--allowedTools`. Full: `bypassPermissions` when it widens the policy, `dontAsk` with the read
/// tools when the session is read-only. `approval: never` under confirm is refused before this
/// point ([`NEVER_REFUSED`]).
pub(crate) fn launch_for(policy: &HarnessPolicy, access: AccessMode) -> Launch {
    if access == AccessMode::Full && policy.sandbox == SandboxMode::ReadOnly {
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
    if access.widens(policy) {
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
    sources: SettingSources,
    model: Option<&str>,
    resume: Option<&str>,
    new_id: &str,
) -> Vec<String> {
    let mut args = base_args();
    args.extend(permission_args(launch, sources));
    if let Some(model) = model.filter(|model| *model != "default") {
        args.extend(["--model".into(), model.into()]);
    }
    match resume {
        Some(id) => args.extend(["--resume".into(), id.into()]),
        None => args.extend(["--session-id".into(), new_id.into()]),
    }
    args
}

/// The permission switches of a launch, in the order they are passed.
fn permission_args(launch: &Launch, sources: SettingSources) -> Vec<String> {
    let mut args = vec![
        "--permission-mode".to_owned(),
        launch.permission_mode.to_owned(),
    ];
    // Whose settings the CLI reads (the profile's choice). Under `Personal` the human's own
    // `allow` rules and hooks answer before `can_use_tool`, as in their terminal and in T3 (SDK
    // `types.py` `can_use_tool`); managed settings apply under both (`claude --help`).
    args.extend(sources.args());
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

/// The form of a Claude Code session id (`8-4-4-4-12` hex digits). A stored id of another form is
/// never put after `--resume` (it could read as a switch): such a conversation counts as lost.
fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(crate) fn random_uuid() -> Result<String, HarnessFailure> {
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

impl VersionCheck {
    /// Where the found version stands against the range of `version.rs`: a version outside it is
    /// `unsupported`; no version read (not installed, unreadable, failing) is `unknown`.
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

/// Installed, version, sign-in and models, from `claude --version` and the CLI's own answer to
/// `initialize` of a probe that starts nothing; and where the version stands against the range of
/// `version.rs`. No token or sign-in file is read.
async fn discover_claude(config: &ClaudeConfig) -> (Discovery, VersionStatus) {
    let checked = check_version(config).await;
    let status = checked.status();
    let version = match checked {
        VersionCheck::Supported(version) => version,
        VersionCheck::NotInstalled => {
            let discovery = Discovery {
                installed: false,
                version: None,
                auth: AuthState::Unknown,
                models: Vec::new(),
                detail: Some(version::not_installed_message()),
            };
            return (discovery, status);
        }
        VersionCheck::Refused { version, message } => {
            let discovery = Discovery {
                installed: true,
                version,
                auth: AuthState::Unknown,
                models: Vec::new(),
                detail: Some(hoh_external_harness::redact::redact(&message)),
            };
            return (discovery, status);
        }
    };
    let probe = spec(config, probe_args(), std::env::temp_dir(), true);
    let started = match spawn(&probe).await {
        Ok(sup) => {
            ClaudeSession::start(
                sup,
                String::new(),
                std::env::temp_dir(),
                &version,
                settings_of(config, false),
            )
            .await
        }
        Err(failure) => Err(failure),
    };
    let (session, _events, init) = match started {
        Ok(started) => started,
        Err(failure) => {
            let discovery = Discovery {
                installed: true,
                version: Some(version),
                auth: AuthState::Unknown,
                models: Vec::new(),
                detail: Some(hoh_external_harness::redact::redact(&failure.message)),
            };
            return (discovery, status);
        }
    };
    session.terminate_now().await;
    let auth = auth_from(init.response.get("account"));
    let discovery = Discovery {
        installed: true,
        version: Some(version),
        auth,
        models: models_from(&init.response),
        detail: (auth == AuthState::Unauthenticated).then(|| {
            "Вход в Claude Code не выполнен: запустите `claude` в терминале и войдите.".to_owned()
        }),
    };
    (discovery, status)
}

impl ExternalHarness for ClaudeHarness {
    fn discover(&self) -> BoxFuture<'static, HarnessResult<Discovery>> {
        let config = self.config.clone();
        Box::pin(async move { Ok(discover_claude(&config).await.0) })
    }

    fn discover_with_status(
        &self,
    ) -> BoxFuture<'static, HarnessResult<(Discovery, VersionStatus)>> {
        let config = self.config.clone();
        Box::pin(async move { Ok(discover_claude(&config).await) })
    }

    /// Pilot 22.11: after the CLI's own interrupt a running command went on (`sleep 120`), and its
    /// late result became the next answer. «Стоп» ends the CLI and every command under it; the
    /// next turn continues the conversation with `--resume`.
    fn stop_ends_process(&self) -> bool {
        true
    }

    /// Without a mode from the slot: the default one ([`DEFAULT_ACCESS`], confirm).
    fn start_or_resume(&self, params: StartParams) -> BoxFuture<'static, HarnessResult<Started>> {
        self.start_with_access(params, DEFAULT_ACCESS)
    }

    /// The mode is the slot's argument and nothing else: not this package's configuration, not
    /// the session's policy, not what the CLI says. Full access widens only an ordinary session
    /// ([`AccessMode::widens`]); a read-only one stays read-only.
    fn start_with_access(
        &self,
        params: StartParams,
        access: AccessMode,
    ) -> BoxFuture<'static, HarnessResult<Started>> {
        let config = self.config.clone();
        Box::pin(async move {
            // In full access nothing is asked by definition, so `never` changes nothing there.
            if params.policy.approval == ApprovalMode::Never && access == AccessMode::Confirm {
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
            if resume.as_deref().is_some_and(|id| !is_uuid(id)) {
                return Err(HarnessFailure::new(
                    code::SESSION_NOT_FOUND,
                    "Сессия Claude Code не найдена: её нельзя продолжить.",
                ));
            }
            let native = match &resume {
                Some(id) => id.clone(),
                None => random_uuid()?,
            };
            let launch = launch_for(&params.policy, access);
            let args = session_args(
                &launch,
                config.setting_sources,
                params.model.as_deref(),
                resume.as_deref(),
                &native,
            );
            let cwd = PathBuf::from(&params.cwd);
            let sup = spawn(&spec(&config, args, cwd.clone(), false)).await?;
            let (session, events, _init) = ClaudeSession::start(
                sup,
                native.clone(),
                cwd,
                &version,
                settings_of(&config, launch.bypass),
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

    /// The same switches [`start_with_access`](Self::start_with_access) passes (for the window).
    fn launch_flags(&self, policy: &HarnessPolicy, access: AccessMode) -> Vec<String> {
        self.config.launch_flags(policy, access)
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
    fn only_a_session_id_of_the_cli_form_is_resumed() {
        assert!(is_uuid(&random_uuid().unwrap()));
        assert!(is_uuid("67ED0A1B-2c3d-4e5f-8a9b-0c1d2e3f4a5b"));
        for bad in [
            "",
            "--dangerously-skip-permissions",
            "67ed0a1b-2c3d-4e5f-8a9b-0c1d2e3f4a5",
            "67ed0a1b-2c3d-4e5f-8a9b-0c1d2e3f4a5bz",
            "67ed0a1b2c3d-4e5f-8a9b-0c1d2e3f4a5b",
            "g7ed0a1b-2c3d-4e5f-8a9b-0c1d2e3f4a5b",
        ] {
            assert!(!is_uuid(bad), "{bad}");
        }
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
                        AccessMode::Confirm,
                    );
                    assert_eq!(launch.permission_mode, "default");
                    let args = session_args(&launch, DEFAULT_SETTING_SOURCES, None, None, "id");
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
        let config = ClaudeConfig::default();
        let argv = config.argv(
            &HarnessPolicy::default(),
            AccessMode::Full,
            None,
            None,
            "id",
        );
        let at = argv.iter().position(|a| a == "--permission-mode").unwrap();
        assert_eq!(argv[at + 1], "bypassPermissions");
        assert!(argv.contains(&"--allow-dangerously-skip-permissions".to_owned()));
        assert!(argv.contains(&"--permission-prompt-tool".to_owned()));
        assert!(!argv.contains(&"--disallowedTools".to_owned()));
        assert!(!argv.contains(&"--tools".to_owned()));
    }

    #[test]
    fn the_launch_flags_are_the_slice_of_the_argv_between_the_stream_flags_and_the_ids() {
        let read_only = HarnessPolicy {
            sandbox: SandboxMode::ReadOnly,
            ..HarnessPolicy::default()
        };
        let full_sandbox = HarnessPolicy {
            sandbox: SandboxMode::FullAccess,
            network: NetworkAccess::Allowed,
            ..HarnessPolicy::default()
        };
        for setting_sources in [SettingSources::Personal, SettingSources::Isolated] {
            let config = ClaudeConfig {
                setting_sources,
                ..ClaudeConfig::default()
            };
            for policy in [HarnessPolicy::default(), read_only, full_sandbox] {
                for access in [AccessMode::Confirm, AccessMode::Full] {
                    for resume in [None, Some("old")] {
                        let argv = config.argv(&policy, access, Some("opus"), resume, "new");
                        let mut expected = vec!["claude".to_owned()];
                        expected.extend(base_args());
                        expected.extend(config.launch_flags(&policy, access));
                        expected.extend(["--model".to_owned(), "opus".to_owned()]);
                        match resume {
                            Some(id) => expected.extend(["--resume".to_owned(), id.to_owned()]),
                            None => expected.extend(["--session-id".to_owned(), "new".to_owned()]),
                        }
                        assert_eq!(argv, expected, "{setting_sources:?} {policy:?} {access:?}");
                    }
                }
            }
        }
        // What the window shows for each mode, with the default sources (the human's settings).
        let config = ClaudeConfig::default();
        assert_eq!(config.setting_sources, SettingSources::Personal);
        assert_eq!(
            config.launch_flags(&HarnessPolicy::default(), AccessMode::Confirm),
            [
                "--permission-mode",
                "default",
                "--setting-sources",
                "user",
                "--permission-prompt-tool",
                "stdio",
                "--disallowedTools",
                "WebFetch,WebSearch"
            ]
        );
        assert_eq!(
            config.launch_flags(&full_sandbox, AccessMode::Full),
            [
                "--permission-mode",
                "bypassPermissions",
                "--setting-sources",
                "user",
                "--permission-prompt-tool",
                "stdio",
                "--allow-dangerously-skip-permissions"
            ]
        );
        assert_eq!(
            config.launch_flags(&read_only, AccessMode::Full),
            [
                "--permission-mode",
                "dontAsk",
                "--setting-sources",
                "user",
                "--tools",
                "Read,Glob,Grep,LS,NotebookRead,TodoWrite"
            ]
        );
        // Isolated: nothing of the human's.
        let isolated = ClaudeConfig {
            setting_sources: SettingSources::Isolated,
            ..ClaudeConfig::default()
        };
        assert_eq!(
            isolated.launch_flags(&HarnessPolicy::default(), AccessMode::Confirm),
            [
                "--permission-mode",
                "default",
                "--setting-sources",
                "",
                "--strict-mcp-config",
                "--permission-prompt-tool",
                "stdio",
                "--disallowedTools",
                "WebFetch,WebSearch"
            ]
        );
    }

    #[test]
    fn the_probe_does_not_read_the_settings_of_the_user_either() {
        let args = probe_args();
        let at = args.iter().position(|a| a == "--setting-sources").unwrap();
        assert_eq!(args[at + 1], "");
    }

    #[test]
    fn default_model_adds_no_model_flag() {
        let launch = launch_for(&HarnessPolicy::default(), AccessMode::Confirm);
        let sources = DEFAULT_SETTING_SOURCES;
        assert!(
            !session_args(&launch, sources, Some("default"), None, "id")
                .contains(&"--model".into())
        );
        assert!(
            session_args(&launch, sources, Some("opus"), None, "id").contains(&"--model".into())
        );
    }
}
