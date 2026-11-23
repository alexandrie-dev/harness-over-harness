use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    Claude,
    Codex,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenStats {
    /// The most recently completed conversation turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TokenUsage>,
    /// Sum of provider-reported per-turn usage. For Claude this is main-agent `result.usage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<TokenUsage>,
    /// Provider ledger deltas accumulated across process restarts, kept separate from
    /// `conversation` to prevent double counting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_total: Option<TokenUsage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlanSource {
    TodoWrite,
    ExitPlanMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSnapshot {
    pub source: PlanSource,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact_threshold: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInvocation {
    pub name: String,
    pub call_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invoked: Option<Vec<SkillInvocation>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpServerStatus {
    Connected,
    Pending,
    NeedsAuth,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStats {
    pub name: String,
    pub status: McpServerStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityStatus {
    Running,
    Finished,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundKind {
    ScheduleWakeup,
    Cron,
    Bash,
    Monitor,
    Agent,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundJobStats {
    pub id: String,
    pub kind: BackgroundKind,
    pub label: String,
    pub status: ActivityStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubagentKind {
    Claude,
    Codex,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentStats {
    /// Stable reference to the launcher tool call.
    pub call_id: String,
    pub kind: SubagentKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: ActivityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatsSnapshot {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// `None` is rendered as “нет”; the reducer never synthesizes a plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanSnapshot>,
    pub tokens: TokenStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_count: Option<u64>,
    pub skills: SkillStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<McpServerStats>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_jobs: Option<Vec<BackgroundJobStats>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<SubagentStats>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatsEvent {
    UserPrompt {
        text: String,
    },
    TurnStarted {
        id: String,
        at_ms: Option<i64>,
    },
    TurnFinished {
        id: String,
        at_ms: Option<i64>,
    },
    RuntimeMetadata {
        provider: Option<ProviderKind>,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<String>,
    },
    ContextUpdated(ContextStats),
    AvailableSkills(Vec<String>),
    McpServers(Vec<McpServerStats>),
    ToolCall {
        id: String,
        name: String,
        input: Value,
        parent_tool_use_id: Option<String>,
    },
    ToolResult {
        id: String,
        is_error: Option<bool>,
    },
    SystemTask {
        id: String,
        tool_use_id: Option<String>,
        label: Option<String>,
        status: ActivityStatus,
    },
    BackgroundTasksSnapshot {
        active_task_ids: Vec<String>,
    },
    ClaudeResult {
        id: String,
        /// Stable CLI-process identity. A new value starts a fresh cumulative ledger segment.
        ledger_id: Option<String>,
        usage: TokenUsage,
        model_usage: Option<TokenUsage>,
        total_cost_usd: Option<f64>,
        duration_ms: Option<u64>,
    },
    CodexTurnStarted {
        id: String,
    },
    CodexTokenUsageUpdated {
        turn_id: String,
        total: TokenUsage,
        last: TokenUsage,
    },
    CodexTurnFinished {
        id: String,
    },
}
