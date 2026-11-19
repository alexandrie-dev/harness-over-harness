//! Durable and model-facing Schedule value types.

use hoh_llm::MessageId;
use hoh_session::SessionId;
use serde::{Deserialize, Serialize};

hoh_brand::branded_string!(pub ScheduleId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleRecord {
    After {
        id: ScheduleId,
        title: String,
        prompt: String,
        #[serde(rename = "afterSeconds")]
        after_seconds: u64,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    At {
        id: ScheduleId,
        title: String,
        prompt: String,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Every {
        id: ScheduleId,
        title: String,
        prompt: String,
        #[serde(rename = "everySeconds")]
        every_seconds: u64,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Daily {
        id: ScheduleId,
        title: String,
        prompt: String,
        time: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Weekly {
        id: ScheduleId,
        title: String,
        prompt: String,
        time: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
        weekdays: Vec<u8>,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Cron {
        id: ScheduleId,
        title: String,
        prompt: String,
        expression: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Rrule {
        id: ScheduleId,
        title: String,
        prompt: String,
        rrule: String,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
}

impl ScheduleRecord {
    pub fn id(&self) -> &ScheduleId {
        match self {
            Self::After { id, .. }
            | Self::At { id, .. }
            | Self::Every { id, .. }
            | Self::Daily { id, .. }
            | Self::Weekly { id, .. }
            | Self::Cron { id, .. }
            | Self::Rrule { id, .. } => id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::After { title, .. }
            | Self::At { title, .. }
            | Self::Every { title, .. }
            | Self::Daily { title, .. }
            | Self::Weekly { title, .. }
            | Self::Cron { title, .. }
            | Self::Rrule { title, .. } => title,
        }
    }

    pub fn prompt(&self) -> &str {
        match self {
            Self::After { prompt, .. }
            | Self::At { prompt, .. }
            | Self::Every { prompt, .. }
            | Self::Daily { prompt, .. }
            | Self::Weekly { prompt, .. }
            | Self::Cron { prompt, .. }
            | Self::Rrule { prompt, .. } => prompt,
        }
    }

    pub fn scheduled_at(&self) -> &str {
        match self {
            Self::After { scheduled_at, .. }
            | Self::At { scheduled_at, .. }
            | Self::Every { scheduled_at, .. }
            | Self::Daily { scheduled_at, .. }
            | Self::Weekly { scheduled_at, .. }
            | Self::Cron { scheduled_at, .. }
            | Self::Rrule { scheduled_at, .. } => scheduled_at,
        }
    }

    pub fn set_scheduled_at(&mut self, value: String) {
        match self {
            Self::After { scheduled_at, .. }
            | Self::At { scheduled_at, .. }
            | Self::Every { scheduled_at, .. }
            | Self::Daily { scheduled_at, .. }
            | Self::Weekly { scheduled_at, .. }
            | Self::Cron { scheduled_at, .. }
            | Self::Rrule { scheduled_at, .. } => *scheduled_at = value,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LegacyScheduleRecord {
    After {
        id: ScheduleId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        prompt: String,
        #[serde(rename = "afterSeconds")]
        after_seconds: u64,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    At {
        id: ScheduleId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        prompt: String,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
    Every {
        id: ScheduleId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        prompt: String,
        #[serde(rename = "everySeconds")]
        every_seconds: u64,
        #[serde(rename = "scheduledAt")]
        scheduled_at: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ScheduleChange {
    Create {
        version: u8,
        schedule: LegacyScheduleRecord,
    },
    Delete {
        version: u8,
        id: ScheduleId,
    },
    Dispatch {
        version: u8,
        id: ScheduleId,
        #[serde(
            rename = "acceptedAt",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        accepted_at: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyInput {
    pub time: String,
    pub time_zone: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeeklyInput {
    pub time: String,
    pub time_zone: String,
    pub weekdays: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronInput {
    pub expression: String,
    pub time_zone: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAtInput {
    pub date: String,
    pub time: String,
    pub time_zone: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AtInput {
    Instant(String),
    Local(LocalAtInput),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleTiming {
    After { after_seconds: u64 },
    At { at: AtInput },
    Every { every_seconds: u64 },
    Daily { daily: DailyInput },
    Weekly { weekly: WeeklyInput },
    Cron { cron: CronInput },
    Rrule { rrule: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleSessionCreateSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_preset: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ScheduleTarget {
    Session {
        session_id: SessionId,
    },
    NewOnce {
        session_id: SessionId,
        create: ScheduleSessionCreateSpec,
    },
    New {
        create: ScheduleSessionCreateSpec,
    },
}

impl ScheduleTarget {
    pub fn session_id(&self) -> Option<&SessionId> {
        match self {
            Self::Session { session_id } | Self::NewOnce { session_id, .. } => Some(session_id),
            Self::New { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ScheduleTargetRequest {
    Session {
        session_id: SessionId,
    },
    NewOnce {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        create: Option<ScheduleSessionCreateSpec>,
    },
    New {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        create: Option<ScheduleSessionCreateSpec>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleDeliveryReceipt {
    pub scheduled_at: String,
    pub delivered_at: String,
    pub message_id: MessageId,
    pub target_session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleDeliveryRecord {
    pub scheduled_at: String,
    pub delivered_at: String,
    pub message_id: MessageId,
    pub target_session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryRetentionBounds {
    pub days: u64,
    pub records: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleDeliveryHistoryRequest {
    pub id: ScheduleId,
    pub limit: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<MessageId>,
}

/// JSON-facing history request. Numbers stay floating-point until the service
/// has applied the same `Number.isSafeInteger` checks as the TypeScript host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteScheduleDeliveryHistoryRequest {
    pub id: ScheduleId,
    pub limit: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<MessageId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleDeliveryHistoryPage {
    pub id: ScheduleId,
    pub records: Vec<ScheduleDeliveryRecord>,
    pub earlier_records_unavailable: bool,
    pub earlier_records_pruned: bool,
    pub retention: DeliveryRetentionBounds,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_before: Option<MessageId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleDeliveryHistoryMissCode {
    ScheduleNotFound,
    DeliveryCursorNotFound,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleDeliveryHistoryResult {
    Page(ScheduleDeliveryHistoryPage),
    Miss {
        id: ScheduleId,
        code: ScheduleDeliveryHistoryMissCode,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    Active,
    Paused,
    Ended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleCatalogEntry {
    #[serde(flatten)]
    pub record: ScheduleRecord,
    pub target: ScheduleTarget,
    pub involved_session_ids: Vec<SessionId>,
    pub status: ScheduleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_floor_ends_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivery: Option<ScheduleDeliveryReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleCreateRequest {
    pub title: String,
    pub prompt: String,
    pub timing: ScheduleTiming,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ScheduleTargetRequest>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleListRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

/// JSON-facing list request. Signed and fractional JSON numbers must reach the
/// schedule service so it can return the package's stable remote error.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RemoteScheduleListRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleListResult {
    pub items: Vec<ScheduleCatalogEntry>,
    pub shown: u64,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleDeliveryLimits {
    pub new_session_interval_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleViewRequest {
    pub id: ScheduleId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<ScheduleTiming>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ScheduleTargetRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ScheduleEditRequest {
    Update {
        id: ScheduleId,
        patch: SchedulePatch,
        #[serde(
            rename = "expectedUpdatedAt",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        expected_updated_at: Option<Option<String>>,
    },
    Delete {
        id: ScheduleId,
        #[serde(
            rename = "expectedUpdatedAt",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        expected_updated_at: Option<Option<String>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRunRequest {
    pub id: ScheduleId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleInputErrorCode {
    InvalidPrompt,
    InvalidSelector,
    InvalidRule,
    InvalidTimeZone,
    NotFuture,
    TimeOutOfRange,
    FrequencyTooHigh,
    SubagentSession,
}

impl ScheduleInputErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidPrompt => "invalid_prompt",
            Self::InvalidSelector => "invalid_selector",
            Self::InvalidRule => "invalid_rule",
            Self::InvalidTimeZone => "invalid_time_zone",
            Self::NotFuture => "not_future",
            Self::TimeOutOfRange => "time_out_of_range",
            Self::FrequencyTooHigh => "frequency_too_high",
            Self::SubagentSession => "subagent_session",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleToolError {
    pub code: ScheduleToolErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleToolErrorCode {
    InvalidPrompt,
    InvalidSelector,
    InvalidRule,
    InvalidTimeZone,
    NotFuture,
    TimeOutOfRange,
    FrequencyTooHigh,
    SubagentSession,
    InternalError,
}

impl From<ScheduleInputErrorCode> for ScheduleToolErrorCode {
    fn from(value: ScheduleInputErrorCode) -> Self {
        match value {
            ScheduleInputErrorCode::InvalidPrompt => Self::InvalidPrompt,
            ScheduleInputErrorCode::InvalidSelector => Self::InvalidSelector,
            ScheduleInputErrorCode::InvalidRule => Self::InvalidRule,
            ScheduleInputErrorCode::InvalidTimeZone => Self::InvalidTimeZone,
            ScheduleInputErrorCode::NotFuture => Self::NotFuture,
            ScheduleInputErrorCode::TimeOutOfRange => Self::TimeOutOfRange,
            ScheduleInputErrorCode::FrequencyTooHigh => Self::FrequencyTooHigh,
            ScheduleInputErrorCode::SubagentSession => Self::SubagentSession,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleDeleteInactiveResult {
    pub deleted: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleDeleteResult {
    Deleted {
        id: ScheduleId,
        deleted: bool,
    },
    NotFound {
        id: ScheduleId,
        deleted: bool,
        code: ScheduleNotFoundCode,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleNotFoundCode {
    ScheduleNotFound,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleNotFoundResult {
    pub id: ScheduleId,
    pub code: ScheduleNotFoundCode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleConflictResult {
    pub id: ScheduleId,
    pub code: ScheduleConflictCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleConflictCode {
    ScheduleConflict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleExecutionPendingResult {
    pub id: ScheduleId,
    pub code: ScheduleExecutionPendingCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleExecutionPendingCode {
    ScheduleExecutionPending,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleViewResult {
    Entry(Box<ScheduleCatalogEntry>),
    NotFound(ScheduleNotFoundResult),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleEditResult {
    Entry(Box<ScheduleCatalogEntry>),
    Deleted { id: ScheduleId, deleted: bool },
    NotFound(ScheduleNotFoundResult),
    Conflict(ScheduleConflictResult),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleRunResult {
    Entry(Box<ScheduleCatalogEntry>),
    NotFound(ScheduleNotFoundResult),
    Pending(ScheduleExecutionPendingResult),
}
