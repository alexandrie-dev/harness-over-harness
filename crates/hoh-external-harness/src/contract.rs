//! The contract between HoH and an external agent harness, version [`CONTRACT_VERSION`].
//!
//! Every type here is plain JSON (serde, camelCase keys, tagged enums): the same messages an
//! in-process adapter exchanges through [`crate::ExternalHarness`] go over JSON-RPC 2.0, one
//! message per line (`docs/packages.md` §3), when a harness runs out of process — see
//! [`crate::StdioHarness`] and the [`method`] names. Nothing here names a concrete harness.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The contract version this crate speaks. A peer announces the versions it speaks in
/// [`InitializeParams::protocol_versions`] and answers with the one chosen.
pub const CONTRACT_VERSION: u32 = 1;

// `NativeSessionId`: the harness's own id of its session or thread — the only thing HoH keeps to
// continue that session after a restart.
hoh_brand::branded_string!(pub NativeSessionId);

/// JSON-RPC method names of the contract.
pub mod method {
    /// Host → harness, first request: version negotiation.
    pub const INITIALIZE: &str = "initialize";
    /// Host → harness: [`super::Discovery`], over stdio with an optional `versionStatus`
    /// ([`super::VersionStatus`], `unknown` when absent).
    pub const DISCOVER: &str = "harness/discover";
    /// Host → harness: [`super::StartParams`] → [`super::StartResult`]; over stdio the params
    /// also carry `access` ([`super::AccessMode`]).
    pub const START: &str = "harness/start";
    /// Host → harness: [`super::PromptParams`] → `{}`; the turn then streams events.
    pub const PROMPT: &str = "harness/prompt";
    /// Host → harness: [`super::SessionParams`] → `{}`; the native interrupt of the running turn.
    pub const INTERRUPT: &str = "harness/interrupt";
    /// Host → harness: [`super::RespondParams`] → `{}`; the answer to a request event.
    pub const RESPOND: &str = "harness/respond";
    /// Host → harness: [`super::SessionParams`] → `{}`; graceful end before the process exits.
    pub const STOP: &str = "harness/stop";
    /// Harness → host notification: [`super::EventParams`].
    pub const EVENT: &str = "harness/event";
}

/// Error codes of [`HarnessFailure`].
pub mod code {
    /// The harness program is not installed (or not executable).
    pub const NOT_INSTALLED: &str = "not-installed";
    /// `resume` named a native session the harness no longer has.
    pub const SESSION_NOT_FOUND: &str = "session-not-found";
    /// The peer broke the contract (bad frame, unknown version, unexpected answer).
    pub const PROTOCOL: &str = "protocol-error";
    /// A bounded harness operation did not answer before its deadline.
    pub const TIMEOUT: &str = "timeout";
    /// No common contract version.
    pub const UNSUPPORTED_VERSION: &str = "unsupported-version";
    /// The harness session or its process is gone.
    pub const CLOSED: &str = "closed";
    /// The harness reported a failure of its own.
    pub const HARNESS: &str = "harness-error";
    /// The user did not allow a policy wider than the safe default.
    pub const ESCALATION_DENIED: &str = "escalation-denied";
    /// Input or output on the process failed.
    pub const IO: &str = "io-error";
    /// Under `approval: never` the harness ran a command or a file change without the allowed
    /// request the contract requires (HoH stops it).
    pub const APPROVAL_BYPASSED: &str = "approval-bypassed";
    /// The policy is not available in this build (`approval: never` before its own review), or
    /// the harness has no launch for the access mode the user chose: no native session is
    /// started under it.
    pub const POLICY_UNAVAILABLE: &str = "policy-unavailable";
}

/// A failure, as an error value and as JSON (`{code, message}`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct HarnessFailure {
    pub code: String,
    pub message: String,
}

impl HarnessFailure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// `initialize` parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_versions: Vec<u32>,
    pub host: PeerInfo,
}

/// `initialize` result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub harness: PeerInfo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Who settles the harness's permission checks. Chosen only by the user in the window
/// (`hoh/executor/access/set`, a privileged command); nothing in a session — the chat, the model,
/// the harness, a file in the working folder, the parameters a session is created with — sets it.
///
/// The four modes T3 Code exposes, with both executors' native spelling of auto-accept edits.
/// The middle modes keep confirm's sandbox (the session folder, no network) and its questions for
/// what the CLI does not settle itself. `AutoAcceptEdits` is Codex's wire value; `AcceptEdits` is
/// Claude's. They have the same security boundary and remain distinct so persisted choices cannot
/// silently change provider-native meaning.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
pub enum AccessMode {
    /// «С подтверждением» (T3 Supervised): the CLI keeps its sandbox (the session folder, no
    /// network) and puts every command, write and network step it does not settle itself to the
    /// user through HoH.
    #[serde(rename = "confirm")]
    Confirm,
    /// T3 «Auto-accept edits»: Codex may write in the workspace, while commands and wider
    /// permissions still reach the person.
    #[serde(rename = "auto-accept-edits")]
    AutoAcceptEdits,
    /// T3 «Auto-accept edits»: as confirm, but the CLI makes file edits in its working folders
    /// without asking (Claude Code `acceptEdits`).
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    /// T3 «Auto»: as confirm, but the CLI itself decides which steps are safe to run without
    /// asking (Codex's auto-reviewer or Claude Code `auto`); the others still reach the user.
    #[serde(rename = "auto")]
    Auto,
    /// «Полный доступ», as in T3 Code: the CLI runs commands without questions and without a
    /// sandbox. A session configured read-only stays read-only (a reviewer stays one): full access
    /// only takes the questions away (review 232500, item 4).
    #[serde(rename = "full")]
    Full,
}

/// The access mode until the user chooses another one in the window: the one place it is set.
pub const DEFAULT_ACCESS: AccessMode = AccessMode::Confirm;

impl Default for AccessMode {
    fn default() -> Self {
        DEFAULT_ACCESS
    }
}

impl AccessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirm => "confirm",
            Self::AutoAcceptEdits => "auto-accept-edits",
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::Full => "full",
        }
    }

    /// The CLI's questions it does not settle itself reach the user: every mode but full access.
    pub fn asks(self) -> bool {
        self != Self::Full
    }

    /// A session started with `policy` under this mode runs with the harness's full-access
    /// launch: [`AccessMode::Full`], except a read-only session, which full access does not widen.
    pub fn widens(self, policy: &HarnessPolicy) -> bool {
        self == Self::Full && policy.sandbox != SandboxMode::ReadOnly
    }
}

/// Where a found version stands against the range the harness was checked with. A version
/// outside the range is refused when a chat starts (`unsupported-version`); `discover` reports the
/// status so the window can say so before that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VersionStatus {
    Supported,
    Unsupported,
    Unknown,
}

/// The fixed, non-secret stage of a harness discovery probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryOperation {
    Version,
    Initialize,
}

/// Machine-readable probe failures for a window that must explain the cause without parsing
/// localized diagnostics. Raw stdout/stderr, paths and account data never belong here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DiscoveryFailure {
    Timeout {
        operation: DiscoveryOperation,
        #[serde(rename = "timeoutSeconds")]
        timeout_seconds: u64,
    },
    Exit {
        operation: DiscoveryOperation,
        #[serde(rename = "exitCode")]
        exit_code: Option<i32>,
    },
}

/// What the window may know about a harness: installed, signed in, models. Never a secret: a
/// harness reports the sign-in state its CLI reports and does not read token files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub auth: AuthState,
    #[serde(default)]
    pub models: Vec<HarnessModel>,
    /// Why it is not usable, in words (missing program, unsupported version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// A stable safe reason for failures whose useful facts cannot be recovered from `detail`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<DiscoveryFailure>,
}

/// What the harness itself declares for a folder (HOH-41): its slash commands and its skills, as
/// the window's `/` and `$` offer them. Only what the harness answered; HoH reads none of its files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCommands {
    pub commands: Vec<HarnessCommand>,
    pub skills: Vec<HarnessCommand>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCommand {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthState {
    Authenticated,
    Unauthenticated,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_default: bool,
    /// Reasoning levels the native model reports, in its own order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effort_levels: Vec<HarnessEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessEffort {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// What the harness may do on its own. The default is the safe one of the HOH-18 note:
/// writes only inside the folder, no network, ask before anything else.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessPolicy {
    #[serde(default)]
    pub sandbox: SandboxMode,
    #[serde(default)]
    pub approval: ApprovalMode,
    #[serde(default)]
    pub network: NetworkAccess,
}

impl HarnessPolicy {
    /// Wider than the safe default (full access, approval `never`, network). The configured
    /// policy is not what decides this: under [`AccessMode::Confirm`] HoH starts a native
    /// session with the sandbox and network of the safe default whatever the configuration says,
    /// and only [`AccessMode::Full`], chosen by the user in the window, starts one without them.
    pub fn is_escalated(&self) -> bool {
        self.sandbox == SandboxMode::FullAccess
            || self.approval == ApprovalMode::Never
            || self.network == NetworkAccess::Allowed
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    #[default]
    WorkspaceWrite,
    FullAccess,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    /// The harness asks when it needs more than the sandbox gives.
    #[default]
    OnRequest,
    /// The harness asks before anything but trusted reads.
    Untrusted,
    /// The harness does not ask the user itself (escalated): it forwards every command and file
    /// change to HoH as [`HarnessEvent::ApprovalRequested`] with the step's `itemId`, and runs it
    /// only after `allow`. HoH answers by itself only for a step the destructive classifier finds
    /// harmless and that stays inside the session folder; it puts everything else to the user.
    /// The CLI must run in a mode that keeps asking (not one that bypasses its permission
    /// checks). A command or file change that shows output or completes (`completed`/`failed`)
    /// without an allowed request for its id breaks the contract: HoH stops the harness and ends
    /// the turn with [`code::APPROVAL_BYPASSED`]. The harness's other tools are not checked.
    ///
    /// Turned off until its own review (feature `never-mode`, off by default): HoH refuses to
    /// start a native session under it ([`code::POLICY_UNAVAILABLE`]).
    Never,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkAccess {
    #[default]
    Denied,
    Allowed,
}

/// `harness/start`: start a native session, or continue `resume`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartParams {
    /// The HoH session the native session serves.
    pub session_id: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd_identity: Option<hoh_working_directory::WorkingDirectoryRootIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The effort level of the session, one of the harness's
    /// [`effort_levels`](crate::ExternalHarness::effort_levels); none: the harness's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The interaction mode selected for this turn. A changed mode starts another native
    /// process on the same continued conversation.
    #[serde(default, skip_serializing_if = "InteractionMode::is_default")]
    pub interaction_mode: InteractionMode,
    pub policy: HarnessPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<NativeSessionId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InteractionMode {
    #[default]
    Default,
    Plan,
}

impl InteractionMode {
    fn is_default(value: &Self) -> bool {
        *value == Self::Default
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResult {
    pub native_session_id: NativeSessionId,
    /// The native session was continued (not started anew).
    pub resumed: bool,
}

/// `harness/prompt`: one user turn.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptParams {
    pub session_id: String,
    /// HoH's id of the turn; every turn event carries it back.
    pub turn_id: String,
    pub text: String,
    /// The pictures the person attached to the turn's messages, in order (HOH-41): the copies the
    /// engine's attachment store keeps, never a path the window named.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<PromptImage>,
}

/// One picture of a turn: its media type (`image/png`, `image/jpeg`, `image/gif`, `image/webp`)
/// and its bytes in standard base64.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptImage {
    pub media_type: String,
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Parameters of `harness/interrupt` and `harness/stop`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionParams {
    pub session_id: String,
}

/// `harness/respond`: the answer to [`HarnessEvent::ApprovalRequested`] or
/// [`HarnessEvent::QuestionAsked`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RespondParams {
    pub session_id: String,
    pub request_id: String,
    pub response: HarnessResponse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HarnessResponse {
    Approval {
        decision: ApprovalDecision,
    },
    Answer {
        answers: Vec<QuestionAnswer>,
    },
    /// The request was withdrawn on HoH's side (turn stopped, no answer possible).
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswer {
    pub id: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// `harness/event` notification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventParams {
    pub session_id: String,
    pub event: HarnessEvent,
}

/// Ignorable session-log record that carries one raw provider telemetry frame.
pub const TELEMETRY_RECORD: &str = "plugin:hoh-external-harness/telemetry";

/// Durable record written for raw provider telemetry. The session log supplies ordering; these
/// fields preserve the provider identity needed to recognise a replay or a process/session reset
/// without deriving totals in the harness adapter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessTelemetryRecord {
    pub provenance: HarnessTelemetryProvenance,
    pub payload: HarnessTelemetry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessTelemetryProvenance {
    /// Unique for one native process launch, including when it resumes the same native session.
    pub launch_id: String,
    pub native_session_id: NativeSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// The provider frame UUID or message id. It is absent rather than invented when the
    /// provider supplied no stable id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_item_id: Option<String>,
}

/// Raw telemetry only. Conversation totals, ledger deltas, task reconciliation and
/// deduplication belong to the session-stats projection that folds these durable records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum HarnessTelemetry {
    AssistantUsage {
        usage: HarnessTokenUsage,
    },
    ResultUsage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<HarnessTokenUsage>,
        /// `None` means the field was absent; `Some([])` preserves an explicitly empty ledger.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_usage: Option<Vec<HarnessModelUsage>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_cost_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        user_message_uuids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<String>,
    },
    McpServers {
        servers: Vec<HarnessMcpServer>,
    },
    BackgroundTask {
        update: HarnessBackgroundTaskUpdate,
    },
    BackgroundTaskUpdated {
        task_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        patch: HarnessBackgroundTaskPatch,
    },
    BackgroundTasksSnapshot {
        tasks: Vec<HarnessBackgroundTask>,
    },
}

/// Token counts exactly as Claude reported them. Missing is not zero.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessTokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessModelUsage {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Only public identity/status from `system/init.mcp_servers`; command, args, env and transport
/// configuration never enter the session log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessMcpServer {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessBackgroundTask {
    pub task_id: String,
    /// A resumed task may reuse `task_id`; when Claude supplies this identity it distinguishes
    /// the concrete run without making the harness infer lifecycle state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessBackgroundTaskUpdate {
    pub subtype: String,
    pub task: HarnessBackgroundTask,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_backgrounded: Option<bool>,
    /// Typed-safe subset of a workflow progress roster. Unknown item shapes are omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_progress: Vec<HarnessWorkflowProgress>,
}

/// Typed-safe subset of Claude's `task_updated.patch`. Missing fields remain unknown; aliases
/// seen across CLI versions are normalized by the adapter before this record is emitted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessBackgroundTaskPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessWorkflowProgress {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
}

/// One thing that happened in a native session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum HarnessEvent {
    /// The native id of the session, when it is assigned or changes.
    NativeSession {
        native_session_id: NativeSessionId,
    },
    /// Opens the turn `harness/prompt` named, before any other event of that turn. A request
    /// that reaches HoH before it was made between turns and is withdrawn (`cancelled`), however
    /// late it arrives.
    TurnStarted {
        turn_id: String,
    },
    TurnCompleted {
        turn_id: String,
    },
    /// The turn stopped after an interrupt.
    TurnAborted {
        turn_id: String,
    },
    TurnFailed {
        turn_id: String,
        error: HarnessFailure,
    },
    /// Visible answer text as it is written.
    TextDelta {
        message_id: String,
        text: String,
        /// The item whose native tool call owns this subagent event.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_item_id: Option<String>,
    },
    /// The complete text of one answer message; authoritative over the deltas.
    TextFinal {
        message_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_item_id: Option<String>,
    },
    ReasoningDelta {
        message_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_item_id: Option<String>,
    },
    ReasoningFinal {
        message_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_item_id: Option<String>,
    },
    /// A progress line (not part of the answer).
    Status {
        text: String,
    },
    /// Provider accounting and background lifecycle that does not belong in the visible turn
    /// stream. The external agent persists it as an ignorable plugin record.
    Telemetry {
        record: HarnessTelemetryRecord,
    },
    /// The latest root model-message context snapshot. These are per-message counts, not the
    /// aggregate usage of every model call in a turn.
    Usage {
        input_tokens: u64,
        cache_creation_input_tokens: u64,
        cache_read_input_tokens: u64,
        output_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u64>,
    },
    /// A step began: a command, a file change, another tool.
    ItemStarted {
        item_id: String,
        action: HarnessAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_item_id: Option<String>,
    },
    /// Output of a running step (command output).
    ItemOutput {
        item_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_item_id: Option<String>,
    },
    ItemCompleted {
        item_id: String,
        outcome: ItemOutcome,
        /// The whole output, when the harness has it; otherwise the streamed output is used.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_item_id: Option<String>,
    },
    /// The native permission request: HoH answers with `harness/respond`. A `request_id` is used
    /// once per turn: a repeated one breaks the contract (HoH ends the turn with
    /// [`code::PROTOCOL`] and stops the harness). Under [`ApprovalMode::Never`] a request for a
    /// step carries the step's `item_id`; the step may be reported started before it, but runs
    /// only after `allow`.
    ApprovalRequested {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        action: HarnessAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_item_id: Option<String>,
    },
    /// A question to the user: HoH answers with `harness/respond`. Its `request_id` is unique in
    /// the turn, as for [`HarnessEvent::ApprovalRequested`].
    QuestionAsked {
        request_id: String,
        questions: Vec<HarnessQuestion>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_item_id: Option<String>,
    },
    /// The harness no longer waits for this request (answered elsewhere, timed out, turn over).
    RequestResolved {
        request_id: String,
    },
    /// Written by HoH's side, never by a harness: the session's process or stream ended.
    SessionEnded {
        reason: EndReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i64>,
    },
}

/// What a step or a permission request is about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum HarnessAction {
    Command {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    FileChange {
        changes: Vec<FileChange>,
    },
    /// Any other tool of the harness, by its own name and raw input.
    Tool {
        name: String,
        input: Value,
    },
    /// A wider permission (network, a folder outside the sandbox), in the harness's words.
    Permission {
        description: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,
    /// The new path when the change moves the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_path: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileChangeKind {
    Add,
    Update,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemOutcome {
    Completed,
    Failed,
    /// The user (or policy) did not allow it.
    Declined,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessQuestion {
    pub id: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multi_select: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EndReason {
    /// The process exited on its own after the stream ended.
    Exited,
    /// The process died with an error status or a signal.
    Crashed,
    /// The output stream broke (closed, cut off mid-frame) while the process may still run.
    StreamClosed,
    /// The process wrote something that is not a contract message.
    ProtocolError,
    /// HoH stopped it (interrupt escalation or teardown).
    Terminated,
}
