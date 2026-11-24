//! [`ClaudeHarness`]: the installed `claude` CLI as an [`ExternalHarness`].
//!
//! The launch line and the mapping of HoH's policy to Claude's permission mode follow T3 Code
//! `ClaudeAdapterV2.ts` (`permissionModeForClaudeRuntimePolicy`, `claudeRuntimeQueryPolicy…`);
//! discovery follows `ClaudeProvider.ts` (`claudeAuthStatus`, capabilities probe).

use std::collections::HashMap;
use std::fs::DirBuilder;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_external_harness::{
    AccessMode, ApprovalMode, AuthState, DEFAULT_ACCESS, Discovery, DiscoveryFailure,
    DiscoveryOperation, ExitInfo, ExternalHarness, Frame, HarnessCommand, HarnessCommands,
    HarnessFailure, HarnessModel, HarnessPolicy, HarnessResult, InteractionMode, NetworkAccess,
    ProcessSpec, SandboxMode, StartParams, StartResult, Started, Supervisor, SupervisorLimits,
    VersionStatus, code,
};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

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
    /// The settings Claude Code reads in the human's terminal, those of
    /// [`PERSONAL_SETTING_SOURCES`]: their own (permission rules, hooks, plugins, `CLAUDE.md`) and
    /// the working folder's (`CLAUDE.md`, `.claude/settings.json`, `.claude/settings.local.json`,
    /// `.mcp.json`); no MCP server is turned off.
    Personal,
    /// Nothing of the human's: no settings file (`--setting-sources ""`) and no MCP server
    /// (`--strict-mcp-config`, none given). What the organization manages still applies.
    Isolated,
}

/// The only place the default is set: the human's own settings apply, as in T3 (the founder and
/// the CEO, 21.11). [`SettingSources::Isolated`] stays available in the profile.
pub const DEFAULT_SETTING_SOURCES: SettingSources = SettingSources::Personal;

/// The setting sources of [`SettingSources::Personal`], the only place they are named: all three,
/// as T3 runs Claude ("так же, как в T3", the founder, HOH-40, 22.11; it replaces the CEO's
/// "user only" of 21.11). T3 gives its SDK no `settingSources`, and reads the same three itself
/// when it needs to know what the session obeys (`ClaudeAdapterV2.ts:5595`, `:5628`,
/// `ClaudeDriver.ts:164`; with a home of its own, `CLAUDE_CONFIG_DIR`, only the folder's two: we
/// have none). The folder's `allow` rules and hooks then answer before the window's question, as
/// they do in the human's terminal; the mode is still only the window's (`--permission-mode`).
const PERSONAL_SETTING_SOURCES: &str = "user,project,local";

/// The effort levels of Claude Code, lowest first: `claude --help` (`--effort <level>`) and T3
/// (`CLAUDE_EFFORT_LEVELS`, `ClaudeModelCatalog.ts:366`).
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

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
    /// `resume` names one to continue (`--resume`); `effort` is one of [`EFFORT_LEVELS`]
    /// (`--effort`). The part that decides what the session may do without asking is
    /// [`launch_flags`](Self::launch_flags) of the same policy and mode, in the same place.
    pub fn argv(
        &self,
        policy: &HarnessPolicy,
        access: AccessMode,
        model: Option<&str>,
        effort: Option<&str>,
        resume: Option<&str>,
        native_id: &str,
    ) -> Vec<String> {
        let launch = launch_for(policy, access);
        let mut argv = vec![self.executable.to_string_lossy().into_owned()];
        argv.extend(session_args(
            &launch,
            self.setting_sources,
            model,
            effort,
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
    command_snapshots: Arc<CommandSnapshots>,
}

#[derive(Clone)]
struct CommandSnapshot {
    revision: u64,
    commands: HarnessCommands,
}

#[derive(Default)]
struct CommandSnapshots {
    next_revision: AtomicU64,
    by_folder: Mutex<HashMap<PathBuf, CommandSnapshot>>,
}

impl CommandSnapshots {
    fn get(&self, cwd: &Path) -> Option<CommandSnapshot> {
        self.by_folder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(cwd)
            .cloned()
    }

    fn replace(&self, cwd: PathBuf, commands: HarnessCommands) {
        let revision = self.next_revision.fetch_add(1, Ordering::Relaxed) + 1;
        self.by_folder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(cwd, CommandSnapshot { revision, commands });
    }
}

impl ClaudeHarness {
    pub fn new(config: ClaudeConfig) -> Self {
        Self {
            config,
            command_snapshots: Arc::default(),
        }
    }

    /// [`ClaudeConfig::argv`] of this harness.
    pub fn argv(
        &self,
        policy: &HarnessPolicy,
        access: AccessMode,
        model: Option<&str>,
        effort: Option<&str>,
        resume: Option<&str>,
        native_id: &str,
    ) -> Vec<String> {
        self.config
            .argv(policy, access, model, effort, resume, native_id)
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
/// tools when the session is read-only. The two modes between them (CEO 23.11 00:07, T3
/// `ClaudeAdapterV2.ts:3395-3404`): accept-edits → `acceptEdits`, auto → `auto`, each with
/// confirm's callback and network limit; a read-only session stays confirm's read-only launch.
/// `approval: never` under an asking mode is refused before this point ([`NEVER_REFUSED`]).
pub(crate) fn launch_for(policy: &HarnessPolicy, access: AccessMode) -> Launch {
    launch_for_interaction(policy, access, InteractionMode::Default)
}

fn launch_for_interaction(
    policy: &HarnessPolicy,
    access: AccessMode,
    interaction_mode: InteractionMode,
) -> Launch {
    if interaction_mode == InteractionMode::Plan {
        let read_only = policy.sandbox == SandboxMode::ReadOnly;
        return Launch {
            permission_mode: "plan",
            permission_prompt: true,
            bypass: false,
            // Plan mode changes Claude's interaction policy, not the session sandbox. T3 keeps
            // its read-only allow-list here too, so a reviewer cannot acquire Bash/Write merely
            // by asking Claude to plan.
            tools: read_only.then_some(READ_ONLY_TOOLS),
            disallowed_tools: (!read_only && policy.network == NetworkAccess::Denied)
                .then_some("WebFetch,WebSearch"),
        };
    }
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
    let permission_mode = match access {
        _ if read_only => "default",
        AccessMode::AutoAcceptEdits | AccessMode::AcceptEdits => "acceptEdits",
        AccessMode::Auto => "auto",
        // Full that does not widen is read-only, handled above.
        AccessMode::Confirm | AccessMode::Full => "default",
    };
    Launch {
        permission_mode,
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
    effort: Option<&str>,
    resume: Option<&str>,
    new_id: &str,
) -> Vec<String> {
    let mut args = base_args();
    args.extend(permission_args(launch, sources));
    if let Some(model) = model.filter(|model| *model != "default") {
        args.extend(["--model".into(), model.into()]);
    }
    // T3 hands the chosen level to its SDK (`effort`, `ClaudeAdapterV2.ts:2296`), which passes it
    // to the CLI as this switch.
    if let Some(effort) = effort {
        args.extend(["--effort".into(), effort.into()]);
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

/// The probe of the slash commands and skills of a folder (HOH-41), T3
/// `buildClaudeCapabilitiesProbeQueryOptions` with the folder as its cwd: before the person's
/// first message it uses user settings but never the untrusted folder's project/local settings,
/// no hooks, no MCP server, no stored session, and nothing allowed without a question. Once a
/// real session starts for that message, its `initialize` and `reload_skills` snapshot replaces
/// this probe. The probe itself runs no turn.
fn commands_probe_args(sources: SettingSources) -> Vec<String> {
    let mut args = base_args();
    // Before a turn Personal may see the person's own commands, while Isolated must stay as
    // empty as the real session. Neither profile reads project/local sources in this probe.
    let sources = match sources {
        SettingSources::Personal => "user",
        SettingSources::Isolated => "",
    };
    args.extend(
        [
            "--permission-mode",
            "default",
            "--permission-prompt-tool",
            "stdio",
            "--setting-sources",
            sources,
        ]
        .map(str::to_owned),
    );
    args.extend(
        [
            "--settings",
            "{\"disableAllHooks\":true}",
            "--no-session-persistence",
            "--mcp-config",
            "{\"mcpServers\":{}}",
        ]
        .map(str::to_owned),
    );
    args.push("--strict-mcp-config".to_owned());
    args
}

/// Commands as Claude Code lists them (`initialize` → `commands`, `reload_skills` → `skills`):
/// each with a name, trimmed; empty parts dropped; one per name regardless of case, the first,
/// filled with a later one's description and hint (T3 `parseClaudeInitializationCommands`,
/// `dedupeSlashCommands`). Nothing is added.
pub(crate) fn commands_from(list: Option<&Value>) -> Vec<HarnessCommand> {
    let field = |command: &Value, key: &str| {
        command
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let mut found: Vec<HarnessCommand> = Vec::new();
    for command in list.and_then(Value::as_array).into_iter().flatten() {
        let Some(name) = field(command, "name") else {
            continue;
        };
        let description = field(command, "description");
        let argument_hint = field(command, "argumentHint");
        match found
            .iter_mut()
            .find(|seen| seen.name.to_lowercase() == name.to_lowercase())
        {
            Some(seen) => {
                if seen.description.is_none() {
                    seen.description = description;
                }
                if seen.argument_hint.is_none() {
                    seen.argument_hint = argument_hint;
                }
            }
            None => found.push(HarnessCommand {
                name,
                description,
                argument_hint,
            }),
        }
    }
    found
}

/// What Claude Code declares for `cwd`, from [`commands_probe_args`]: the commands of its answer
/// to `initialize`, the skills of its answer to `reload_skills` (a CLI without that request gives
/// none). HoH reads none of Claude Code's files.
async fn commands_of(config: &ClaudeConfig, cwd: PathBuf) -> HarnessResult<HarnessCommands> {
    let version = match check_version(config).await {
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
    let args = commands_probe_args(config.setting_sources);
    let sup = spawn(&spec(config, args, cwd.clone(), true), None)?;
    let (session, _events, init) = ClaudeSession::start(
        sup,
        String::new(),
        cwd,
        &version,
        settings_of(config, false),
    )
    .await?;
    let skills = session.control("reload_skills", config.init_timeout).await;
    session.terminate_now().await;
    Ok(HarnessCommands {
        commands: commands_from(init.response.get("commands")),
        skills: skills
            .map(|answer| commands_from(answer.get("skills")))
            .unwrap_or_default(),
    })
}

const TITLE_SCHEMA: &str = r#"{"type":"object","properties":{"title":{"type":"string"}},"required":["title"],"additionalProperties":false}"#;

fn managed_mcp_config_path() -> &'static Path {
    if cfg!(target_os = "macos") {
        Path::new("/Library/Application Support/ClaudeCode/managed-mcp.json")
    } else if cfg!(target_os = "windows") {
        Path::new(r"C:\Program Files\ClaudeCode\managed-mcp.json")
    } else {
        Path::new("/etc/claude-code/managed-mcp.json")
    }
}

const TITLE_INPUT_MAX_BYTES: usize = 64 * 1024;
const TITLE_OUTPUT_MAX_BYTES: usize = 64 * 1024;
const TITLE_TIMEOUT: Duration = Duration::from_secs(45);
static TITLE_SLOT: Semaphore = Semaphore::const_new(1);

fn title_args(
    setting_sources: SettingSources,
    no_session_persistence: bool,
    managed_mcp_config: bool,
) -> Vec<String> {
    let mut args = [
        "-p",
        "--output-format",
        "json",
        "--json-schema",
        TITLE_SCHEMA,
        "--model",
        "claude-haiku-5-5",
        "--effort",
        "low",
        "--settings",
        r#"{"disableAllHooks":true}"#,
        "--tools",
        "",
        "--disable-slash-commands",
    ]
    .map(str::to_owned)
    .to_vec();
    if no_session_persistence {
        args.push("--no-session-persistence".into());
    }
    args.extend(
        [
            "--setting-sources",
            match setting_sources {
                SettingSources::Personal => PERSONAL_SETTING_SOURCES,
                SettingSources::Isolated => "",
            },
        ]
        .map(str::to_owned),
    );
    // Claude rejects strict MCP mode when the administrator has installed managed-mcp.json;
    // that file already excludes every other MCP source. This is T3's exact conditional.
    if !managed_mcp_config {
        args.push("--strict-mcp-config".into());
    }
    args.extend(["--permission-mode".into(), "dontAsk".into()]);
    args
}

#[derive(Debug)]
struct TitleCommandOutput {
    status: ExitInfo,
    response: Option<Value>,
    stderr: String,
}

/// A cancelled title future must not detach its CLI or any descendant. Successful/explicitly
/// terminated supervisors make this a cheap idempotent no-op.
struct AbandonTitleOnDrop<'a>(&'a Supervisor);

impl Drop for AbandonTitleOnDrop<'_> {
    fn drop(&mut self) {
        self.0.abandon();
    }
}

/// Private, cancellation-safe storage for the one-shot cwd and fallback Claude config. Drop is
/// synchronous deliberately: cancelling the future must remove persisted prompt/session data
/// before control returns to its caller.
struct TitleTempRoot {
    path: PathBuf,
    closed: bool,
}

impl TitleTempRoot {
    fn create() -> HarnessResult<Self> {
        let path = std::env::temp_dir().join(format!("hoh-claude-title-{}", random_uuid()?));
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(&path).map_err(|error| {
            HarnessFailure::new(
                code::IO,
                format!("cannot create private title storage: {error}"),
            )
        })?;
        Ok(Self {
            path,
            closed: false,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn close(mut self) -> HarnessResult<()> {
        std::fs::remove_dir_all(&self.path).map_err(|error| {
            HarnessFailure::new(
                code::IO,
                format!("cannot remove private title storage: {error}"),
            )
        })?;
        self.closed = true;
        Ok(())
    }

    /// Remove storage before returning either a successful title response or its failure. This
    /// keeps ordinary errors and timeouts on the same explicit, fallible cleanup path.
    fn finish<T>(self, outcome: HarnessResult<T>) -> HarnessResult<T> {
        self.close()?;
        outcome
    }
}

impl Drop for TitleTempRoot {
    fn drop(&mut self) {
        if !self.closed
            && let Err(error) = std::fs::remove_dir_all(&self.path)
        {
            // Drop cannot return the cleanup failure, but it must not be silent: this means
            // cancellation may have left isolated Claude persistence behind. Do not print the
            // path because it can identify a sensitive title request.
            eprintln!("cannot remove cancelled Claude title storage: {error}");
        }
    }
}

fn title_spec(
    config: &ClaudeConfig,
    cwd: &Path,
    no_session_persistence: bool,
    fallback_config_dir: Option<&Path>,
) -> ProcessSpec {
    let mut env: Vec<_> = config
        .extra_env
        .iter()
        .filter(|(name, _)| fallback_config_dir.is_none() || name != "CLAUDE_CONFIG_DIR")
        .cloned()
        .collect();
    if let Some(directory) = fallback_config_dir {
        env.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            directory.to_string_lossy().into_owned(),
        ));
    }
    ProcessSpec {
        program: config.executable.clone(),
        args: title_args(
            config.setting_sources,
            no_session_persistence,
            managed_mcp_config_path().exists(),
        ),
        cwd: Some(cwd.to_owned()),
        env,
        env_remove: Vec::new(),
    }
}

fn title_limits() -> SupervisorLimits {
    SupervisorLimits {
        max_frame_bytes: TITLE_OUTPUT_MAX_BYTES,
        frame_queue: 2,
        stdin_queue: 1,
        stderr_lines: 20,
        stderr_line_bytes: 2048,
    }
}

async fn run_title_command(
    config: &ClaudeConfig,
    cwd: &Path,
    prompt: &str,
    no_session_persistence: bool,
    fallback_config_dir: Option<&Path>,
) -> Result<TitleCommandOutput, HarnessFailure> {
    let spec = title_spec(config, cwd, no_session_persistence, fallback_config_dir);
    run_title_process(&spec, prompt, TITLE_TIMEOUT).await
}

async fn run_title_process(
    spec: &ProcessSpec,
    prompt: &str,
    deadline: Duration,
) -> Result<TitleCommandOutput, HarnessFailure> {
    let supervisor = Supervisor::spawn(spec, title_limits(), None)?;
    let _abandon_on_drop = AbandonTitleOnDrop(&supervisor);
    let Some(mut frames) = supervisor.take_frames() else {
        supervisor.terminate(Duration::from_millis(200)).await;
        return Err(HarnessFailure::new(
            code::CLOSED,
            "Claude title output is unavailable",
        ));
    };
    let outcome = tokio::time::timeout(deadline, async {
        supervisor
            .send_input_and_close(prompt.as_bytes(), TITLE_INPUT_MAX_BYTES)
            .await?;
        let mut response = None;
        loop {
            match frames.recv().await {
                Some(Frame::Message(value)) if response.is_none() => response = Some(value),
                Some(Frame::Message(_)) => {
                    return Err(HarnessFailure::new(
                        code::PROTOCOL,
                        "Claude returned more than one title response",
                    ));
                }
                Some(Frame::Violation(detail)) => {
                    return Err(HarnessFailure::new(code::PROTOCOL, detail));
                }
                Some(Frame::Closed { truncated: true }) => {
                    return Err(HarnessFailure::new(
                        code::PROTOCOL,
                        "Claude returned a truncated title response",
                    ));
                }
                Some(Frame::Closed { truncated: false }) | None => break,
            }
        }
        Ok((supervisor.wait().await, response))
    })
    .await;
    let (status, response) = match outcome {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            supervisor.terminate(Duration::from_millis(200)).await;
            return Err(error);
        }
        Err(_) => {
            supervisor.terminate(Duration::from_millis(200)).await;
            return Err(HarnessFailure::new(
                code::HARNESS,
                "Claude title request timed out",
            ));
        }
    };
    supervisor.stderr_drained(Duration::from_millis(200)).await;
    Ok(TitleCommandOutput {
        status,
        response,
        stderr: supervisor.stderr_tail(),
    })
}

async fn generate_title(config: ClaudeConfig, prompt: String) -> HarnessResult<String> {
    let _slot = TITLE_SLOT.acquire().await.map_err(|_| {
        HarnessFailure::new(code::CLOSED, "Claude title request slot is unavailable")
    })?;
    let root = TitleTempRoot::create()?;
    let directory = root.path().join("cwd");
    let fallback_config = root.path().join("config");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| {
            HarnessFailure::new(
                code::HARNESS,
                format!("cannot create title directory: {error}"),
            )
        })?;
    let outcome = async {
        let first = run_title_command(&config, &directory, &prompt, true, None).await;
        match first {
            Ok(output)
                if !output.status.success()
                    && output.stderr.contains("--no-session-persistence") =>
            {
                tokio::fs::create_dir_all(&fallback_config)
                    .await
                    .map_err(|error| {
                        HarnessFailure::new(
                            code::HARNESS,
                            format!("cannot create isolated title configuration: {error}"),
                        )
                    })?;
                run_title_command(&config, &directory, &prompt, false, Some(&fallback_config)).await
            }
            output => output,
        }
    }
    .await;
    let output = root.finish(outcome)?;
    if !output.status.success() {
        return Err(HarnessFailure::new(
            code::HARNESS,
            format!(
                "Claude title request exited unsuccessfully: {}",
                output.stderr.trim()
            ),
        ));
    }
    let envelope = output
        .response
        .ok_or_else(|| HarnessFailure::new(code::PROTOCOL, "Claude returned no title response"))?;
    let title = envelope
        .get("structured_output")
        .and_then(|value| value.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty() && title.len() <= 120)
        .ok_or_else(|| HarnessFailure::new(code::PROTOCOL, "Claude returned an invalid title"))?;
    Ok(title.to_owned())
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
        failure: Option<DiscoveryFailure>,
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
                failure: None,
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
            failure: Some(DiscoveryFailure::Timeout {
                operation: DiscoveryOperation::Version,
                timeout_seconds: config.version_timeout.as_secs().max(1),
            }),
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
            failure: Some(DiscoveryFailure::Exit {
                operation: DiscoveryOperation::Version,
                exit_code: status.code(),
            }),
        };
    }
    match version::parse_version(&output) {
        None => VersionCheck::Refused {
            version: None,
            message: version::unrecognized_version_message(&output),
            failure: None,
        },
        Some(parsed) if version::is_supported(parsed) => {
            VersionCheck::Supported(version::format_version(parsed))
        }
        Some(parsed) => {
            let shown = version::format_version(parsed);
            VersionCheck::Refused {
                message: version::unsupported_version_message(&shown),
                version: Some(shown),
                failure: None,
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

fn spawn(spec: &ProcessSpec, params: Option<&StartParams>) -> Result<Supervisor, HarnessFailure> {
    // A missing folder fails the launch with the same `NotFound` as a missing program.
    if let Some(cwd) = &spec.cwd
        && !std::fs::metadata(cwd).is_ok_and(|meta| meta.is_dir())
    {
        return Err(HarnessFailure::new(
            code::IO,
            format!("Рабочая папка «{}» не найдена.", cwd.display()),
        ));
    }
    if let Some(params) = params {
        hoh_external_harness::validate_launch_directory(params)?;
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
        effort_levels: Vec::new(),
        default_effort: None,
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
                effort_levels: Vec::new(),
                default_effort: None,
            })
        })
        .collect();
    if reported.is_empty() {
        models.extend(["opus", "sonnet", "haiku"].map(|alias| HarnessModel {
            id: alias.into(),
            display_name: None,
            is_default: false,
            effort_levels: Vec::new(),
            default_effort: None,
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
                failure: None,
            };
            return (discovery, status);
        }
        VersionCheck::Refused {
            version,
            message,
            failure,
        } => {
            let discovery = Discovery {
                installed: true,
                version,
                auth: AuthState::Unknown,
                models: Vec::new(),
                detail: Some(hoh_external_harness::redact::redact(&message)),
                failure,
            };
            return (discovery, status);
        }
    };
    let probe = spec(config, probe_args(), std::env::temp_dir(), true);
    let started = match spawn(&probe, None) {
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
                failure: (failure.code == code::TIMEOUT).then_some(DiscoveryFailure::Timeout {
                    operation: DiscoveryOperation::Initialize,
                    timeout_seconds: config.init_timeout.as_secs().max(1),
                }),
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
        failure: None,
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

    fn effort_levels(&self) -> &'static [&'static str] {
        EFFORT_LEVELS
    }

    fn commands(&self, cwd: PathBuf) -> BoxFuture<'static, HarnessResult<HarnessCommands>> {
        if let Some(snapshot) = self.command_snapshots.get(&cwd) {
            return Box::pin(std::future::ready(Ok(snapshot.commands)));
        }
        let config = self.config.clone();
        Box::pin(async move { commands_of(&config, cwd).await })
    }

    fn commands_revision(&self, cwd: &Path) -> u64 {
        self.command_snapshots
            .get(cwd)
            .map_or(0, |snapshot| snapshot.revision)
    }

    fn generate_title(&self, prompt: String) -> BoxFuture<'static, HarnessResult<String>> {
        let config = self.config.clone();
        Box::pin(async move { generate_title(config, prompt).await })
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
        let command_snapshots = self.command_snapshots.clone();
        Box::pin(async move {
            // In full access nothing is asked by definition, so `never` changes nothing there.
            if params.policy.approval == ApprovalMode::Never && access.asks() {
                return Err(HarnessFailure::new(code::ESCALATION_DENIED, NEVER_REFUSED));
            }
            // Only a level of the list follows `--effort` (the slot passes no other).
            if let Some(effort) = params
                .effort
                .as_deref()
                .filter(|e| !EFFORT_LEVELS.contains(e))
            {
                return Err(HarnessFailure::new(
                    code::PROTOCOL,
                    format!("Claude Code has no effort level {effort:?}"),
                ));
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
            let launch = launch_for_interaction(&params.policy, access, params.interaction_mode);
            let args = session_args(
                &launch,
                config.setting_sources,
                params.model.as_deref(),
                params.effort.as_deref(),
                resume.as_deref(),
                &native,
            );
            let cwd = PathBuf::from(&params.cwd);
            // The window keys command snapshots by its canonical known folder. A session may
            // have been started through another spelling of the same folder (for example a
            // symlink), so store the real-session replacement under that same canonical key.
            let command_key = tokio::fs::canonicalize(&cwd)
                .await
                .unwrap_or_else(|_| cwd.clone());
            let sup = spawn(&spec(&config, args, cwd.clone(), false), Some(&params))?;
            let (session, events, init) = ClaudeSession::start(
                sup,
                native.clone(),
                cwd.clone(),
                &version,
                settings_of(&config, launch.bypass),
            )
            .await?;
            let skills = session.control("reload_skills", config.init_timeout).await;
            command_snapshots.replace(
                command_key,
                HarnessCommands {
                    commands: commands_from(init.response.get("commands")),
                    skills: skills
                        .map(|answer| commands_from(answer.get("skills")))
                        .unwrap_or_default(),
                },
            );
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
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

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
                    let args =
                        session_args(&launch, DEFAULT_SETTING_SOURCES, None, None, None, "id");
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
    fn plan_interaction_is_claudes_plan_permission_mode_even_under_full_access() {
        let harness = ClaudeHarness::new(ClaudeConfig::default());
        let argv = permission_args(
            &launch_for_interaction(
                &HarnessPolicy::default(),
                AccessMode::Full,
                InteractionMode::Plan,
            ),
            harness.config.setting_sources,
        );
        let at = argv
            .iter()
            .position(|arg| arg == "--permission-mode")
            .unwrap();
        assert_eq!(argv[at + 1], "plan");
        assert!(argv.contains(&"--permission-prompt-tool".to_owned()));
        assert!(!argv.contains(&"--allow-dangerously-skip-permissions".to_owned()));

        let read_only = HarnessPolicy {
            sandbox: SandboxMode::ReadOnly,
            ..HarnessPolicy::default()
        };
        let read_only_argv = permission_args(
            &launch_for_interaction(&read_only, AccessMode::Full, InteractionMode::Plan),
            harness.config.setting_sources,
        );
        let tools = read_only_argv
            .iter()
            .position(|arg| arg == "--tools")
            .unwrap();
        assert_eq!(read_only_argv[tools + 1], READ_ONLY_TOOLS);
    }

    #[test]
    fn title_attempts_are_bounded_isolated_and_have_no_tools() {
        for (sources, expected) in [
            (SettingSources::Personal, PERSONAL_SETTING_SOURCES),
            (SettingSources::Isolated, ""),
        ] {
            for no_session_persistence in [true, false] {
                let args = title_args(sources, no_session_persistence, false);
                let value = |flag: &str| {
                    let at = args.iter().position(|arg| arg == flag).unwrap();
                    args[at + 1].as_str()
                };
                assert_eq!(value("--model"), "claude-haiku-5-5");
                assert_eq!(value("--effort"), "low");
                assert_eq!(value("--tools"), "");
                assert_eq!(value("--permission-mode"), "dontAsk");
                assert_eq!(value("--setting-sources"), expected);
                assert_eq!(
                    args.contains(&"--no-session-persistence".to_owned()),
                    no_session_persistence
                );
                assert!(args.contains(&"--strict-mcp-config".to_owned()));
                assert!(args.contains(&"--disable-slash-commands".to_owned()));
                assert_eq!(value("--settings"), r#"{"disableAllHooks":true}"#);
            }
        }

        let managed_args = title_args(SettingSources::Isolated, true, true);
        assert!(!managed_args.contains(&"--strict-mcp-config".to_owned()));
        assert!(managed_args.contains(&"--no-session-persistence".to_owned()));
        assert_eq!(TITLE_INPUT_MAX_BYTES, 64 * 1024);
        assert_eq!(TITLE_OUTPUT_MAX_BYTES, 64 * 1024);
        assert_eq!(TITLE_SLOT.available_permits(), 1);
        let limits = title_limits();
        assert_eq!(limits.max_frame_bytes, TITLE_OUTPUT_MAX_BYTES);
        assert_eq!(limits.frame_queue, 2);
        assert_eq!(limits.stdin_queue, 1);

        let mut config = ClaudeConfig {
            setting_sources: SettingSources::Isolated,
            ..ClaudeConfig::default()
        };
        config.extra_env.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            "/human/claude-state".to_owned(),
        ));
        let fallback = title_spec(
            &config,
            Path::new("/isolated/title-cwd"),
            false,
            Some(Path::new("/isolated/title-config")),
        );
        assert_eq!(
            fallback.cwd.as_deref(),
            Some(Path::new("/isolated/title-cwd"))
        );
        assert!(
            !fallback
                .args
                .contains(&"--no-session-persistence".to_owned())
        );
        assert_eq!(
            fallback.env,
            vec![(
                "CLAUDE_CONFIG_DIR".to_owned(),
                "/isolated/title-config".to_owned()
            )]
        );
    }

    #[test]
    fn title_temp_storage_cleans_on_normal_error_and_drop() {
        let dropped_path = {
            let root = TitleTempRoot::create().unwrap();
            let path = root.path().to_owned();
            #[cfg(unix)]
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            std::fs::create_dir(path.join("config")).unwrap();
            std::fs::write(path.join("config/session.json"), b"persisted prompt").unwrap();
            path
        };
        assert!(!dropped_path.exists());

        let root = TitleTempRoot::create().unwrap();
        let closed_path = root.path().to_owned();
        std::fs::create_dir(closed_path.join("cwd")).unwrap();
        root.close().unwrap();
        assert!(!closed_path.exists());

        let root = TitleTempRoot::create().unwrap();
        let failed_path = root.path().to_owned();
        std::fs::create_dir(failed_path.join("config")).unwrap();
        std::fs::write(failed_path.join("config/session.json"), b"persisted prompt").unwrap();
        let failed: HarnessResult<()> = Err(HarnessFailure::new(code::HARNESS, "title timed out"));
        assert!(root.finish(failed).is_err());
        assert!(!failed_path.exists());
    }

    #[tokio::test]
    async fn title_deadline_includes_stdin_and_cancelled_storage_is_removed() {
        let spec = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_owned(), "exec sleep 30".to_owned()],
            ..ProcessSpec::default()
        };
        let prompt = "x".repeat(TITLE_INPUT_MAX_BYTES);
        let error = run_title_process(&spec, &prompt, Duration::from_millis(20))
            .await
            .expect_err("one deadline covers stdin and process wait");
        assert!(error.message.contains("timed out"), "{error}");

        let root = TitleTempRoot::create().unwrap();
        let path = root.path().to_owned();
        std::fs::create_dir(path.join("config")).unwrap();
        std::fs::write(path.join("config/session.json"), b"persisted prompt").unwrap();
        let pending = tokio::spawn(async move {
            let _root = root;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        pending.abort();
        let _ = pending.await;
        assert!(!path.exists());
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
                        let argv =
                            config.argv(&policy, access, Some("opus"), Some("high"), resume, "new");
                        let mut expected = vec!["claude".to_owned()];
                        expected.extend(base_args());
                        expected.extend(config.launch_flags(&policy, access));
                        expected.extend(["--model", "opus", "--effort", "high"].map(str::to_owned));
                        match resume {
                            Some(id) => expected.extend(["--resume".to_owned(), id.to_owned()]),
                            None => expected.extend(["--session-id".to_owned(), "new".to_owned()]),
                        }
                        assert_eq!(argv, expected, "{setting_sources:?} {policy:?} {access:?}");
                    }
                }
            }
        }
        // What the window shows for each mode, with the default sources (the human's and the
        // folder's settings).
        let config = ClaudeConfig::default();
        assert_eq!(config.setting_sources, SettingSources::Personal);
        assert_eq!(
            config.launch_flags(&HarnessPolicy::default(), AccessMode::Confirm),
            [
                "--permission-mode",
                "default",
                "--setting-sources",
                "user,project,local",
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
                "user,project,local",
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
                "user,project,local",
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

    /// CEO 23.11 00:07, T3 `ClaudeAdapterV2.ts:3395-3404`: the CLI's own mode for each of the
    /// window's four, the two in between with confirm's question channel and closed network; a
    /// read-only session stays confirm's read-only launch under them.
    #[test]
    fn each_mode_of_the_window_is_its_permission_mode() {
        let policy = HarnessPolicy::default();
        for (access, mode) in [
            (AccessMode::Confirm, "default"),
            (AccessMode::AcceptEdits, "acceptEdits"),
            (AccessMode::Auto, "auto"),
            (AccessMode::Full, "bypassPermissions"),
        ] {
            let launch = launch_for(&policy, access);
            assert_eq!(launch.permission_mode, mode);
            assert!(launch.permission_prompt);
            assert_eq!(launch.bypass, access == AccessMode::Full);
            assert_eq!(
                launch.disallowed_tools,
                access.asks().then_some("WebFetch,WebSearch")
            );
        }
        let read_only = HarnessPolicy {
            sandbox: SandboxMode::ReadOnly,
            ..HarnessPolicy::default()
        };
        for between in [AccessMode::AcceptEdits, AccessMode::Auto] {
            assert_eq!(
                launch_for(&read_only, between),
                launch_for(&read_only, AccessMode::Confirm)
            );
        }
    }

    #[test]
    fn default_model_adds_no_model_flag() {
        let launch = launch_for(&HarnessPolicy::default(), AccessMode::Confirm);
        let sources = DEFAULT_SETTING_SOURCES;
        assert!(
            !session_args(&launch, sources, Some("default"), None, None, "id")
                .contains(&"--model".into())
        );
        assert!(
            session_args(&launch, sources, Some("opus"), None, None, "id")
                .contains(&"--model".into())
        );
    }
    /// HOH-40: the level the window chose follows `--effort`; none, no switch (the CLI's own).
    #[test]
    fn the_effort_follows_its_switch() {
        let launch = launch_for(&HarnessPolicy::default(), AccessMode::Confirm);
        let sources = DEFAULT_SETTING_SOURCES;
        for level in EFFORT_LEVELS {
            let args = session_args(&launch, sources, None, Some(level), None, "id");
            let at = args.iter().position(|a| a == "--effort").unwrap();
            assert_eq!(args[at + 1], *level);
        }
        let args = session_args(&launch, sources, None, None, None, "id");
        assert!(!args.contains(&"--effort".into()), "{args:?}");
        assert_eq!(
            ClaudeHarness::new(ClaudeConfig::default()).effort_levels(),
            ["low", "medium", "high", "xhigh", "max"]
        );
    }

    /// HOH-40: a level that is not Claude's is refused before anything starts.
    #[tokio::test]
    async fn a_level_claude_does_not_have_starts_nothing() {
        let harness = ClaudeHarness::new(ClaudeConfig {
            executable: PathBuf::from("/nonexistent/claude"),
            ..ClaudeConfig::default()
        });
        let params = StartParams {
            session_id: "s".into(),
            cwd: "/".into(),
            cwd_identity: None,
            model: None,
            effort: Some("--dangerously-skip-permissions".into()),
            interaction_mode: InteractionMode::Default,
            policy: HarnessPolicy::default(),
            resume: None,
        };
        let failure = match harness.start_with_access(params, AccessMode::Confirm).await {
            Err(failure) => failure,
            Ok(_) => panic!("started"),
        };
        assert_eq!(failure.code, code::PROTOCOL, "{failure}");
    }
}
