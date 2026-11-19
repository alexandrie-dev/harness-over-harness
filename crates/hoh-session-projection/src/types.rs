//! Open projection vocabulary. Summary membership is the runtime wire flag.
use crate::Result;
use hoh_session::{
    Session, SessionEvent, SessionHeader, SessionLogOffset, SessionSeq, SessionSeqCursor,
};
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub type SessionProjectionMap = IndexMap<String, Arc<Value>>;
pub type SessionProjectionViewValue<T> = T;
pub type SessionProjectionSummaryExcluded<T> = T;
pub type SessionProjectionViewValues = SessionProjectionMap;
pub type SessionProjectionSummaryValues = SessionProjectionMap;
pub type SessionProjectionSummaryExcludedKey = String;
pub type ProjectionStateSchema<S> = Arc<dyn Fn(Option<&Value>) -> Result<Arc<S>> + Send + Sync>;
pub type ProjectionInit<S> =
    Arc<dyn Fn(&SessionHeader, SessionLogOffset) -> Result<Arc<S>> + Send + Sync>;
pub type ProjectionApply<S> = Arc<dyn Fn(&Arc<S>, &SessionEvent) -> Result<Arc<S>> + Send + Sync>;
pub type ProjectionView<S> = Arc<dyn Fn(&Arc<S>) -> Result<Option<Arc<Value>>> + Send + Sync>;
pub type ProjectionViewSchema = Arc<dyn Fn(Option<&Value>) -> Result<Arc<Value>> + Send + Sync>;

pub struct ProjectionWire<S> {
    pub view_schema: ProjectionViewSchema,
    pub view: ProjectionView<S>,
    pub summary: bool,
}
impl<S> Clone for ProjectionWire<S> {
    fn clone(&self) -> Self {
        Self {
            view_schema: self.view_schema.clone(),
            view: self.view.clone(),
            summary: self.summary,
        }
    }
}
pub struct ProjectionDefinition<S> {
    pub key: String,
    pub state_schema: ProjectionStateSchema<S>,
    pub init: ProjectionInit<S>,
    /// Reuse the input Arc for an unchanged object-valued state.
    pub apply: ProjectionApply<S>,
    pub wire: Option<ProjectionWire<S>>,
    pub state_version: u64,
}
impl<S> Clone for ProjectionDefinition<S> {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            state_schema: self.state_schema.clone(),
            init: self.init.clone(),
            apply: self.apply.clone(),
            wire: self.wire.clone(),
            state_version: self.state_version,
        }
    }
}
impl<S> ProjectionWire<S> {
    pub fn new(view_schema: ProjectionViewSchema, view: ProjectionView<S>) -> Self {
        Self {
            view_schema,
            view,
            summary: true,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionSnapshot {
    pub as_of_seq: SessionSeqCursor,
    pub values: SessionProjectionMap,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectionCheckpointRow {
    pub ver: u64,
    pub seq: SessionSeqCursor,
    /// None is JS undefined, distinct from Some(Value::Null).
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub val: Option<Value>,
}
fn present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Value>, D::Error> {
    Ok(Some(Value::deserialize(deserializer)?))
}
pub type ProjectionCheckpoint = IndexMap<String, ProjectionCheckpointRow>;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectionRestoreResult {
    pub snapshot: ProjectionSnapshot,
    pub checkpoint: ProjectionCheckpoint,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectionChange {
    pub key: String,
    pub val: Arc<Value>,
}
pub type ProjectionChangeListener =
    Arc<dyn Fn(&Session, &str, Arc<Value>, SessionSeq) -> Result<()> + Send + Sync>;
pub type ProjectionEventChangeListener =
    Arc<dyn Fn(&Session, &[ProjectionChange], SessionSeq) -> Result<()> + Send + Sync>;
pub type ProjectionReader<S> = Arc<dyn Fn() -> Result<Arc<S>> + Send + Sync>;

#[derive(Clone, Copy)]
pub enum ProjectionKeySelection<'a> {
    List(&'a [String]),
    Set(&'a IndexSet<String>),
}
impl ProjectionKeySelection<'_> {
    pub(crate) fn contains(&self, key: &str) -> bool {
        match self {
            Self::List(keys) => keys.iter().any(|entry| entry == key),
            Self::Set(keys) => keys.contains(key),
        }
    }
}
