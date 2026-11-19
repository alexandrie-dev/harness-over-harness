//! Durable task storage value types.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_llm::MessageId;
use hoh_session::SessionId;
use hoh_storage_domain::{DomainSpec, DomainSpecInput, ValueSchema, define_domain, domain_table};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{
    ScheduleDeliveryReceipt, ScheduleDeliveryRecord, ScheduleRecord, ScheduleStatus, ScheduleTarget,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryHistory {
    pub records: Vec<ScheduleDeliveryRecord>,
    pub earlier_records_unavailable: bool,
    #[serde(default)]
    pub earlier_records_pruned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleTask {
    pub record: ScheduleRecord,
    pub target: ScheduleTarget,
    pub involved_session_ids: Vec<SessionId>,
    pub status: ScheduleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivery: Option<ScheduleDeliveryReceipt>,
    #[serde(default)]
    pub delivery_history: DeliveryHistory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_delivery: Option<PendingDelivery>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDelivery {
    pub target_session_id: SessionId,
}

fn failure(message: impl Into<String>) -> BoxError {
    Box::new(crate::ScheduleLogError::new(message))
}

fn object(value: &Value) -> Result<&Map<String, Value>, BoxError> {
    value
        .as_object()
        .ok_or_else(|| failure("schedule task must be an object"))
}

fn string(map: &Map<String, Value>, key: &str) -> Result<Option<String>, BoxError> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(failure(format!("{key} must be a string"))),
    }
}

fn instant(map: &Map<String, Value>, key: &str) -> Result<Option<String>, BoxError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => crate::decode_instant(value)
            .map(Some)
            .map_err(|error| Box::new(error) as BoxError),
    }
}

pub fn default_session_id(id: &crate::ScheduleId) -> SessionId {
    let digest = Sha256::digest(id.as_str().as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    SessionId::from(format!("session-schedule-{hex}"))
}

fn decode_create(value: Option<&Value>) -> Result<crate::ScheduleSessionCreateSpec, BoxError> {
    let Some(value) = value else {
        return Ok(crate::ScheduleSessionCreateSpec::default());
    };
    let map = object(value)?;
    if map.keys().any(|key| key != "cwd" && key != "agentPreset") {
        return Err(failure("target create contains an unknown field"));
    }
    let cwd = string(map, "cwd")?;
    let agent_preset = string(map, "agentPreset")?;
    if cwd.as_deref() == Some("") || agent_preset.as_deref() == Some("") {
        return Err(failure(
            "target create cwd and agentPreset must be non-empty when present",
        ));
    }
    Ok(crate::ScheduleSessionCreateSpec { cwd, agent_preset })
}

fn decode_target(value: &Value, id: &crate::ScheduleId) -> Result<ScheduleTarget, BoxError> {
    let map = object(value)?;
    match map.get("kind").and_then(Value::as_str) {
        Some("session") if map.len() == 2 => Ok(ScheduleTarget::Session {
            session_id: SessionId::from(
                string(map, "sessionId")?
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| failure("target sessionId must be non-empty"))?,
            ),
        }),
        Some("new_once")
            if map
                .keys()
                .all(|key| matches!(key.as_str(), "kind" | "sessionId" | "create")) =>
        {
            let session_id = match string(map, "sessionId")? {
                Some(value) if !value.is_empty() => SessionId::from(value),
                Some(_) => return Err(failure("target sessionId must be non-empty")),
                None => default_session_id(id),
            };
            Ok(ScheduleTarget::NewOnce {
                session_id,
                create: decode_create(map.get("create"))?,
            })
        }
        Some("new")
            if map
                .keys()
                .all(|key| matches!(key.as_str(), "kind" | "create")) =>
        {
            Ok(ScheduleTarget::New {
                create: decode_create(map.get("create"))?,
            })
        }
        _ => Err(failure("schedule target does not match its schema")),
    }
}

fn decode_receipt(
    value: &Value,
    fallback: Option<&SessionId>,
    prompt: bool,
) -> Result<crate::ScheduleDeliveryRecord, BoxError> {
    let map = object(value)?;
    let allowed = if prompt {
        &[
            "scheduledAt",
            "deliveredAt",
            "messageId",
            "targetSessionId",
            "prompt",
        ][..]
    } else {
        &["scheduledAt", "deliveredAt", "messageId", "targetSessionId"][..]
    };
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(failure("delivery receipt contains an unknown field"));
    }
    let scheduled_at = crate::decode_instant(map.get("scheduledAt").unwrap_or(&Value::Null))
        .map_err(|error| Box::new(error) as BoxError)?;
    let delivered_at = crate::decode_instant(map.get("deliveredAt").unwrap_or(&Value::Null))
        .map_err(|error| Box::new(error) as BoxError)?;
    let message_id = string(map, "messageId")?
        .filter(|v| !v.is_empty() && hoh_util_values::js_trim(v) == v)
        .ok_or_else(|| failure("messageId must be non-empty without surrounding whitespace"))?;
    let target_session_id = string(map, "targetSessionId")?
        .filter(|value| !value.is_empty())
        .map(SessionId::from)
        .or_else(|| fallback.cloned())
        .ok_or_else(|| {
            failure("A saved delivery without an existing destination must name its target Session")
        })?;
    let saved_prompt = if prompt { string(map, "prompt")? } else { None };
    Ok(crate::ScheduleDeliveryRecord {
        scheduled_at,
        delivered_at,
        message_id: MessageId::from(message_id),
        target_session_id,
        prompt: saved_prompt,
    })
}

fn receipt_of(record: &crate::ScheduleDeliveryRecord) -> ScheduleDeliveryReceipt {
    ScheduleDeliveryReceipt {
        scheduled_at: record.scheduled_at.clone(),
        delivered_at: record.delivered_at.clone(),
        message_id: record.message_id.clone(),
        target_session_id: record.target_session_id.clone(),
    }
}

pub fn decode_schedule_task(value: &Value) -> Result<ScheduleTask, BoxError> {
    let map = object(value)?;
    let allowed = [
        "record",
        "sessionId",
        "target",
        "involvedSessionIds",
        "status",
        "createdAt",
        "updatedAt",
        "lastDelivery",
        "deliveryHistory",
        "pendingDelivery",
    ];
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(failure("schedule task contains an unknown field"));
    }
    let record = crate::decode_schedule_record(map.get("record").unwrap_or(&Value::Null))
        .map_err(|error| Box::new(error) as BoxError)?;
    let saved_target = map.get("target");
    let legacy_session = if saved_target.is_none() {
        match string(map, "sessionId")? {
            Some(value) if !value.is_empty() => Some(SessionId::from(value)),
            Some(_) => return Err(failure("sessionId must be non-empty")),
            None => None,
        }
    } else {
        None
    };
    let target = match saved_target {
        Some(value) => decode_target(value, record.id())?,
        None => match legacy_session.clone() {
            Some(session_id) => ScheduleTarget::Session { session_id },
            None => ScheduleTarget::NewOnce {
                session_id: default_session_id(record.id()),
                create: crate::ScheduleSessionCreateSpec::default(),
            },
        },
    };
    let normalized_fallback = match &target {
        ScheduleTarget::Session { session_id } | ScheduleTarget::NewOnce { session_id, .. } => {
            Some(session_id)
        }
        ScheduleTarget::New { .. } => None,
    };
    let receipt_fallback = match saved_target.and_then(Value::as_object) {
        None => legacy_session.as_ref(),
        Some(saved) if saved.get("kind").and_then(Value::as_str) == Some("session") => {
            normalized_fallback
        }
        Some(saved)
            if saved.get("kind").and_then(Value::as_str) == Some("new_once")
                && saved.contains_key("sessionId") =>
        {
            normalized_fallback
        }
        _ => None,
    };
    let status = match map.get("status") {
        None => ScheduleStatus::Active,
        Some(Value::String(value)) if value == "active" => ScheduleStatus::Active,
        Some(Value::String(value)) if value == "paused" => ScheduleStatus::Paused,
        Some(Value::String(value)) if value == "ended" || value == "inactive" => {
            ScheduleStatus::Ended
        }
        Some(_) => return Err(failure("status must be active, paused, ended, or inactive")),
    };
    let last_delivery = map
        .get("lastDelivery")
        .map(|value| {
            decode_receipt(value, receipt_fallback, false).map(|record| receipt_of(&record))
        })
        .transpose()?;
    let delivery_history = match map.get("deliveryHistory") {
        None => DeliveryHistory {
            records: last_delivery
                .clone()
                .into_iter()
                .map(|receipt| crate::ScheduleDeliveryRecord {
                    scheduled_at: receipt.scheduled_at,
                    delivered_at: receipt.delivered_at,
                    message_id: receipt.message_id,
                    target_session_id: receipt.target_session_id,
                    prompt: None,
                })
                .collect(),
            earlier_records_unavailable: true,
            earlier_records_pruned: false,
        },
        Some(value) => {
            let history = object(value)?;
            if history.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "records" | "earlierRecordsUnavailable" | "earlierRecordsPruned"
                )
            }) {
                return Err(failure("deliveryHistory contains an unknown field"));
            }
            let records = history
                .get("records")
                .and_then(Value::as_array)
                .ok_or_else(|| failure("deliveryHistory.records must be an array"))?
                .iter()
                .map(|value| decode_receipt(value, receipt_fallback, true))
                .collect::<Result<Vec<_>, _>>()?;
            let mut ids = std::collections::BTreeSet::new();
            if records
                .iter()
                .any(|record| !ids.insert(record.message_id.clone()))
            {
                return Err(failure(
                    "Delivery history message identities must be unique within a task",
                ));
            }
            DeliveryHistory {
                records,
                earlier_records_unavailable: history
                    .get("earlierRecordsUnavailable")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| failure("earlierRecordsUnavailable must be a boolean"))?,
                earlier_records_pruned: history
                    .get("earlierRecordsPruned")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }
        }
    };
    let latest = delivery_history.records.last().map(receipt_of);
    if latest != last_delivery {
        return Err(failure(
            "Last delivery must match the latest saved delivery receipt",
        ));
    }
    let involved_session_ids = match map.get("involvedSessionIds") {
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|text| !text.is_empty())
                    .map(SessionId::from)
                    .ok_or_else(|| failure("involvedSessionIds entries must be non-empty strings"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(failure("involvedSessionIds must be an array")),
        None => normalized_fallback.cloned().into_iter().collect(),
    };
    Ok(ScheduleTask {
        record,
        target,
        involved_session_ids,
        status,
        created_at: instant(map, "createdAt")?,
        updated_at: instant(map, "updatedAt")?,
        last_delivery,
        delivery_history,
        pending_delivery: match map.get("pendingDelivery") {
            None => None,
            Some(value) => {
                let pending = object(value)?;
                if pending.len() != 1 {
                    return Err(failure(
                        "pendingDelivery must contain exactly targetSessionId",
                    ));
                }
                Some(PendingDelivery {
                    target_session_id: SessionId::from(
                        string(pending, "targetSessionId")?
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| {
                                failure("pendingDelivery targetSessionId must be non-empty")
                            })?,
                    ),
                })
            }
        },
    })
}

pub fn schedule_domain_spec() -> Result<Arc<DomainSpec>, BoxError> {
    let table =
        domain_table::<crate::ScheduleId, ScheduleTask>(ValueSchema::new(decode_schedule_task));
    let mut input = DomainSpecInput::new(
        "schedule",
        2,
        IndexMap::from([("tasks".to_owned(), table.into())]),
    );
    input.compatible_versions = Some(vec![Value::from(1)]);
    define_domain(input).map(Arc::new)
}
