use crate::{Result, error};
use hoh_session::{MAX_SAFE_INTEGER, SessionLogOffset, SessionSeq, SessionSeqCursor};
use hoh_session_projection::{ProjectionCheckpoint, ProjectionCheckpointRow};
use hoh_storage_domain::{DomainSpec, DomainSpecInput, ValueSchema, define_domain, domain_table};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_version: Option<u64>,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_seeded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_event_count: Option<SessionLogOffset>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointRecord {
    pub identity: CheckpointIdentity,
    pub rows: ProjectionCheckpoint,
}

fn natural(value: Option<&Value>, subject: &str) -> Result<u64> {
    let Some(value) = value else {
        return Err(error(format!("{subject} is required")));
    };
    value
        .as_f64()
        .filter(|number| {
            number.is_finite()
                && *number >= 0.0
                && number.fract() == 0.0
                && *number <= MAX_SAFE_INTEGER as f64
        })
        .map(|number| number as u64)
        .ok_or_else(|| error(format!("{subject} must be a non-negative integer")))
}

fn object<'a>(value: &'a Value, subject: &str) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| error(format!("{subject} must be an object")))
}

pub fn checkpoint_row(value: &Value) -> Result<ProjectionCheckpointRow> {
    let object = object(value, "checkpoint row")?;
    let ver = natural(object.get("ver"), "checkpoint row ver")?;
    let seq = match object.get("seq").and_then(Value::as_f64) {
        Some(-1.0) => SessionSeqCursor::BeforeFirst,
        Some(number)
            if number.is_finite()
                && number >= 0.0
                && number.fract() == 0.0
                && number <= MAX_SAFE_INTEGER as f64 =>
        {
            SessionSeqCursor::Seq(SessionSeq::new(number as u64)?)
        }
        _ => {
            return Err(error(
                "checkpoint row seq must be an integer greater than or equal to -1",
            ));
        }
    };
    let val = object
        .get("val")
        .filter(|value| hoh_util_values::is_json_value(value))
        .cloned()
        .ok_or_else(|| error("checkpoint state must be losslessly JSON-serializable"))?;
    Ok(ProjectionCheckpointRow {
        ver,
        seq,
        val: Some(val),
    })
}

pub fn checkpoint_identity(value: &Value) -> Result<CheckpointIdentity> {
    let object = object(value, "checkpoint identity")?;
    let format_version = object
        .get("formatVersion")
        .map(|value| natural(Some(value), "checkpoint identity formatVersion"))
        .transpose()?;
    let created_at = natural(object.get("createdAt"), "checkpoint identity createdAt")?;
    let cwd = match object.get("cwd") {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err(error("checkpoint identity cwd must be a string")),
    };
    let is_seeded = match object.get("isSeeded") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err(error("checkpoint identity isSeeded must be a boolean")),
    };
    let inherited_event_count = object
        .get("inheritedEventCount")
        .map(|value| {
            SessionLogOffset::new(natural(
                Some(value),
                "checkpoint identity inheritedEventCount",
            )?)
        })
        .transpose()?;
    Ok(CheckpointIdentity {
        format_version,
        created_at,
        cwd,
        is_seeded,
        inherited_event_count,
    })
}

pub fn checkpoint_record(value: &Value) -> Result<CheckpointRecord> {
    let record = object(value, "checkpoint record")?;
    let identity = checkpoint_identity(
        record
            .get("identity")
            .ok_or_else(|| error("checkpoint record identity is required"))?,
    )?;
    let rows_object = object(
        record
            .get("rows")
            .ok_or_else(|| error("checkpoint record rows is required"))?,
        "checkpoint record rows",
    )?;
    let mut rows = IndexMap::new();
    for (key, value) in rows_object {
        rows.insert(key.clone(), checkpoint_row(value)?);
    }
    Ok(CheckpointRecord { identity, rows })
}

pub fn projection_cache_domain_spec() -> Result<Arc<DomainSpec>> {
    let mut input = DomainSpecInput::new(
        "session_projcache",
        7,
        IndexMap::from([(
            "sessions".into(),
            domain_table::<hoh_session::SessionId, CheckpointRecord>(ValueSchema::new(
                checkpoint_record,
            ))
            .into(),
        )]),
    );
    input.compatible_versions = Some(vec![3.into(), 4.into(), 5.into(), 6.into()]);
    input.invalid_records = Some("backup-and-skip".into());
    input.layout = Some("per-record".into());
    Ok(Arc::new(define_domain(input)?))
}
