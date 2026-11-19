use hoh_session::SessionId;
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};

hoh_brand::branded_string!(pub GoalId);

pub const GOAL_CHANGE_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalRef {
    pub id: GoalId,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateGoalRequest {
    pub objective: String,
    pub max_goal_rounds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateGoalResult {
    pub r#ref: GoalRef,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditGoalRequest {
    pub objective: Option<String>,
    pub max_goal_rounds: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalPhase {
    Active,
    Paused,
    Blocked,
    Complete,
}
impl std::fmt::Display for GoalPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::Complete => "complete",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalBlockReason {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalSnapshot {
    pub id: GoalId,
    pub revision: u64,
    pub objective: String,
    pub phase: GoalPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<GoalBlockReason>,
    pub max_goal_rounds: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalActivation {
    Armed,
    Disarmed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalView {
    #[serde(flatten)]
    pub goal: GoalSnapshot,
    pub rounds_started: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub activation: GoalActivation,
}
impl GoalView {
    pub fn as_ref(&self) -> GoalRef {
        GoalRef {
            id: self.goal.id.clone(),
            revision: self.goal.revision,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalProjection {
    pub goal: GoalSnapshot,
    pub rounds_started: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalProjectionState {
    pub current: Option<GoalProjection>,
    pub seen_goal_ids: Vec<GoalId>,
    pub failure: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalOperation {
    Create,
    Edit,
    Pause,
    Resume,
    Complete,
    Block,
    Clear,
}
impl std::fmt::Display for GoalOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Create => "create",
            Self::Edit => "edit",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Complete => "complete",
            Self::Block => "block",
            Self::Clear => "clear",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalSnapshotChangeMeta {
    pub kind: String,
    pub version: u64,
    pub operation: GoalOperation,
    pub goal: GoalSnapshot,
    pub rounds_started: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalClearChangeMeta {
    pub kind: String,
    pub version: u64,
    pub operation: GoalOperation,
    pub cleared: GoalRef,
    pub cleared_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GoalChangeMeta {
    Snapshot(GoalSnapshotChangeMeta),
    Clear(GoalClearChangeMeta),
}
impl GoalChangeMeta {
    pub fn operation(&self) -> GoalOperation {
        match self {
            Self::Snapshot(change) => change.operation,
            Self::Clear(change) => change.operation,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalMessageSource {
    pub goal_id: GoalId,
    pub revision: u64,
    pub round: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldedGoal {
    pub goal: Option<GoalSnapshot>,
    pub rounds_started: u64,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub last_ref: Option<GoalRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalChanged {
    pub operation: GoalOperation,
    pub r#ref: GoalRef,
    pub goal: Option<GoalView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalActivationChanged {
    pub session_id: SessionId,
    pub goal: Option<GoalActivationView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalActivationView {
    pub id: GoalId,
    pub revision: u64,
    pub activation: GoalActivation,
}

#[derive(Clone, Debug)]
pub struct GoalFoldState {
    pub goal: Option<GoalSnapshot>,
    pub rounds_started: u64,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub last_ref: Option<GoalRef>,
    pub seen_goal_ids: IndexSet<GoalId>,
}

pub fn empty_goal_fold_state() -> GoalFoldState {
    GoalFoldState {
        goal: None,
        rounds_started: 0,
        created_at: None,
        updated_at: None,
        last_ref: None,
        seen_goal_ids: IndexSet::new(),
    }
}
