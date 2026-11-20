//! HoH additions. Open design decisions are documented in PORT_NOTES.md.
use crate::{ApprovalOutcome, JsonObject, SessionEvent, choices, object, optional};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

object!(IdParams { id: String });
object!(IdRev {
    id: String,
    rev: u64
});
object!(SessionIdResult { session_id: String });
optional!(SessionListParams {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    employee_id: Option<String>,
});
object!(SessionSummary {
    id: String,
    title: String,
    employee_id: String,
    updated_at: u64
});
object!(SessionListResult { sessions: Vec<SessionSummary> });
optional!(SessionEventsParams {
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    after_seq: Option<i64>,
    limit: u32,
});
object!(SessionEventsResult { events: Vec<SessionEvent>, next_seq: i64 });
choices!(EmployeeStatus { Working => "working", Waiting => "waiting", Sleeping => "sleeping" });
choices!(InternetMode { Ask => "ask", Public => "public", Deny => "deny" });
object!(Permissions { internet: InternetMode, domains: Vec<String>, folders: Vec<String>, write_files: bool, programs: bool, program_network: Vec<String>, act_as_user: bool });
object!(EmployeeDraft { name: String, role: String, rules: Vec<String>, task_ids: Vec<String>, permissions: Permissions });
optional!(EmployeePatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    rules: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    task_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    permissions: Option<Permissions>,
});
optional!(Employee {
    id: String, rev: u64, name: String, role: String, rules: Vec<String>, task_ids: Vec<String>,
    permissions: Permissions, status: EmployeeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    next_run_at: Option<u64>,
});
object!(EmployeeCreateParams {
    draft: EmployeeDraft
});
object!(EmployeeUpdateParams {
    id: String,
    patch: EmployeePatch,
    rev: u64
});
object!(MemoryItem {
    item_id: String,
    text: String,
    updated_at: u64
});
optional!(MemoryPutParams {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    item_id: Option<String>,
    text: String,
});
object!(MemoryDeleteParams {
    id: String,
    item_id: String
});
optional!(TaskDraftStartParams {
    description: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    employee_id: Option<String>,
});
object!(TaskDraftStartResult {
    draft_id: String,
    session_id: String
});
choices!(DraftDecision { Yes => "yes", Fix => "fix", Cancel => "cancel" });
optional!(TaskDraftDecideParams {
    draft_id: String, decision: DraftDecision,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    correction: Option<String>,
});
choices!(DraftState { Asking => "asking", Researching => "researching", Building => "building", ExampleReady => "example_ready", Checking => "checking", Saved => "saved", Failed => "failed" });
object!(TaskDraftDecideResult { state: DraftState });
object!(Task {
    id: String,
    title: String,
    description: String,
    version: u64,
    permissions: Permissions,
    active: bool
});
object!(TaskVersion {
    version: u64,
    created_at: u64,
    description: String
});
object!(TaskRollbackParams {
    id: String,
    version: u64
});
optional!(TaskRunParams {
    task_id: String, input: JsonObject,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    employee_id: Option<String>,
});
object!(TaskRunResult {
    run_id: String,
    session_id: String
});
object!(TaskIdParams { task_id: String });
object!(TaskKnowledgeEditParams {
    task_id: String,
    instruction: String
});
object!(KnowledgeSource {
    title: String,
    uri: String
});
object!(TaskKnowledge { rules: Vec<String>, sources: Vec<KnowledgeSource>, artifact_ids: Vec<String> });
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ButtonTarget {
    Task { id: String },
    Prompt { text: String },
}
object!(ButtonInput {
    name: String,
    label: String,
    required: bool
});
optional!(ButtonDraft {
    label: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    icon: Option<String>,
    target: ButtonTarget,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    inputs: Option<Vec<ButtonInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    employee_id: Option<String>,
});
optional!(Button {
    id: String, label: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    icon: Option<String>,
    target: ButtonTarget,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    inputs: Option<Vec<ButtonInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    employee_id: Option<String>,
});
optional!(ButtonPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    target: Option<ButtonTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    inputs: Option<Vec<ButtonInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    employee_id: Option<String>,
});
object!(ButtonCreateParams { draft: ButtonDraft });
object!(ButtonUpdateParams {
    id: String,
    patch: ButtonPatch
});
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum ScheduleTarget {
    Task {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "employeeId")]
        employee_id: String,
    },
    Prompt {
        #[serde(rename = "employeeId")]
        employee_id: String,
        prompt: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ScheduleWhen {
    Once {
        at: u64,
    },
    Interval {
        #[serde(rename = "everyMs")]
        every_ms: u64,
    },
    Cron {
        expression: String,
        timezone: String,
    },
}
object!(ScheduleDraft {
    target: ScheduleTarget,
    when: ScheduleWhen,
    enabled: bool
});
object!(Schedule {
    id: String,
    target: ScheduleTarget,
    when: ScheduleWhen,
    enabled: bool
});
object!(ScheduleCreateParams {
    draft: ScheduleDraft
});
optional!(SchedulePatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    target: Option<ScheduleTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    when: Option<ScheduleWhen>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    enabled: Option<bool>,
});
object!(ScheduleUpdateParams {
    id: String,
    patch: SchedulePatch
});
choices!(ScheduleRunStatus { Ok => "ok", Error => "error", Missed => "missed", Running => "running" });
optional!(ScheduleRun {
    run_id: String, scheduled_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    fired_at: Option<u64>,
    status: ScheduleRunStatus,
});
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum ArtifactListParams {
    Run {
        #[serde(rename = "runId")]
        run_id: String,
    },
    Session {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    Employee {
        #[serde(rename = "employeeId")]
        employee_id: String,
    },
}
object!(Artifact {
    id: String,
    title: String,
    mime: String,
    size: u64,
    created_at: u64
});
optional!(ArtifactReadParams {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    length: Option<u32>,
});
choices!(ArtifactEncoding { Utf8 => "utf8", Base64 => "base64" });
object!(ArtifactReadResult {
    mime: String,
    size: u64,
    encoding: ArtifactEncoding,
    data: String,
    truncated: bool
});
choices!(ArtifactRenderAs { Pages => "pages", Table => "table", Text => "text" });
optional!(ArtifactRenderParams {
    id: String,
    #[serde(rename = "as")]
    r#as: ArtifactRenderAs,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    page: Option<u32>,
});
object!(RenderedPage {
    page: u32,
    mime: String,
    encoding: ArtifactEncoding,
    data: String
});
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ArtifactRenderResult {
    Pages {
        pages: Vec<RenderedPage>,
        #[serde(rename = "totalPages")]
        total_pages: u32,
    },
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
        truncated: bool,
    },
    Text {
        text: String,
        truncated: bool,
    },
}
object!(ModelProbeParams {
    base_url: String,
    model: String
});
object!(ModelProbeResult {
    ok: bool,
    latency_ms: u64,
    tool_calls: bool,
    models: Vec<String>
});
object!(ModelSettings {
    base_url: String,
    model: String
});
choices!(Theme { Light => "light", Dark => "dark", System => "system" });
object!(ViewSettings {
    theme: Theme,
    font_size: u32
});
object!(Settings {
    model: ModelSettings,
    security: Permissions,
    view: ViewSettings
});
optional!(SettingsSetParams {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    model: Option<ModelSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    security: Option<Permissions>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    view: Option<ViewSettings>,
});
choices!(Risk { Low => "low", Medium => "medium", High => "high" });
object!(SecurityModel {
    local: bool,
    host: String
});
object!(EgressSummary {
    domain: String,
    count: u64,
    last_at: u64
});
object!(PermissionSummary {
    employee_id: String,
    permissions: Permissions
});
optional!(ApprovalSummary {
    approval_id: String, session_id: String, risk: Risk, human_text: String, action_hash: String,
    created_at: u64, expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    outcome: Option<ApprovalOutcome>,
});
object!(SecuritySummary { model: SecurityModel, egress: Vec<EgressSummary>, permissions: Vec<PermissionSummary>, approvals: Vec<ApprovalSummary> });
choices!(ChangeKind { Created => "created", Updated => "updated", Deleted => "deleted" });
object!(EmployeeChanged {
    id: String,
    rev: u64,
    change: ChangeKind
});
object!(TaskChanged {
    id: String,
    version: u64,
    change: ChangeKind
});
optional!(TaskDraftState {
    draft_id: String, state: DraftState,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    error: Option<String>,
});
object!(TaskExample {
    draft_id: String,
    input: JsonObject,
    artifact_id: String
});
object!(ScheduleFired {
    id: String,
    run_id: String,
    scheduled_at: u64,
    fired_at: u64
});
object!(ScheduleChanged {
    id: String,
    change: ChangeKind
});
object!(ArtifactPresented {
    artifact_id: String,
    title: String,
    mime: String,
    run_id: String,
    session_id: String
});
object!(ApprovalPending {
    approval_id: String,
    session_id: String,
    risk: Risk,
    human_text: String,
    expires_at: u64
});
object!(ApprovalResolved {
    approval_id: String,
    outcome: ApprovalOutcome
});
object!(SecurityEgress {
    domain: String,
    employee_id: String,
    size: u64,
    at: u64
});
optional!(HostApprovalRequest {
    session_id: String, tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    reason: Option<String>,
    /// What the window shows folded away: the raw command of a bash approval. Not part of
    /// `action`, so not part of `actionHash` or the decision MAC; the host closes secret parts
    /// of addresses in it as it does in `reason`.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    #[schemars(required)]
    details: Option<String>,
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9_.:-]+$"))]
    approval_id: String,
    risk: Risk,
    #[schemars(length(min = 1, max = 4096))]
    human_text: String,
    #[schemars(length(min = 64, max = 64), regex(pattern = "^[0-9a-f]{64}$"))]
    action_hash: String,
    #[schemars(range(min = 1, max = 9007199254740991u64))]
    expires_at: u64,
    action: Value,
});
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostApprovalDecision {
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9_.:-]+$"))]
    pub approval_id: String,
    pub outcome: ApprovalOutcome,
    #[schemars(length(min = 64, max = 64), regex(pattern = "^[0-9a-f]{64}$"))]
    pub mac: String,
}
impl std::fmt::Debug for HostApprovalDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostApprovalDecision")
            .field("approval_id", &self.approval_id)
            .field("outcome", &self.outcome)
            .field("mac", &"[REDACTED]")
            .finish()
    }
}
object!(ModelKeySetParams {
    api_key: crate::SecretString,
    base_url: String
});
choices!(SetupInternetMode { Ask => "ask", Public => "public" });
object!(SettingsRulesSetParams {
    internet: SetupInternetMode,
    folder: String
});
// External agent harnesses in the window (HOH-18): the access mode of an executor (Claude Code),
// chosen only by the user; discovery and the choice of the executor that new chats go to.
choices!(ExecutorAccessMode { Confirm => "confirm", Full => "full" });
choices!(ExecutorVersionStatus { Supported => "supported", Unsupported => "unsupported", Unknown => "unknown" });
choices!(ExecutorAuth { Authenticated => "authenticated", Unauthenticated => "unauthenticated", Unknown => "unknown" });
object!(ExecutorParams { executor: String });
object!(ExecutorAccessSetParams {
    executor: String,
    mode: ExecutorAccessMode
});
object!(ExecutorAccessSetResult {
    executor: String,
    mode: ExecutorAccessMode
});
object!(ExecutorEffectiveAccess {
    mode: ExecutorAccessMode,
    flags: Vec<String>,
    text: String
});
object!(ExecutorSelection {
    executor: Option<String>,
    model: Option<String>
});
object!(ExecutorAccessGetResult {
    executor: String,
    mode: ExecutorAccessMode,
    effective: ExecutorEffectiveAccess,
    selected: ExecutorSelection
});
object!(ExecutorSelectParams {
    executor: Option<String>,
    model: String
});
optional!(ExecutorModel {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::present")]
    is_default: Option<bool>,
});
object!(ExecutorDiscoverResult {
    executor: String,
    installed: bool,
    version: Option<String>,
    version_status: ExecutorVersionStatus,
    auth: ExecutorAuth,
    models: Vec<ExecutorModel>,
    detail: Option<String>
});
