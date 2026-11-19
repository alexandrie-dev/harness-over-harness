use hoh_llm::MessageId;
use hoh_schedule::*;
use hoh_session::SessionId;
use serde_json::json;

fn task_json() -> serde_json::Value {
    json!({
        "record": {"id":"schedule-1","kind":"at","title":"t","prompt":"p","scheduledAt":"2025-01-01T00:00:00.000Z"},
        "status":"active"
    })
}

#[test]
fn legacy_task_gets_stable_reserved_session() {
    let first = decode_schedule_task(&task_json()).unwrap();
    let second = decode_schedule_task(&task_json()).unwrap();
    assert_eq!(first.target, second.target);
    let ScheduleTarget::NewOnce { session_id, create } = first.target else {
        panic!("wrong target")
    };
    assert!(session_id.as_str().starts_with("session-schedule-"));
    assert_eq!(create, ScheduleSessionCreateSpec::default());
}

#[test]
fn legacy_session_binding_becomes_session_target() {
    let mut value = task_json();
    value["sessionId"] = json!("session-1");
    assert_eq!(
        decode_schedule_task(&value).unwrap().target,
        ScheduleTarget::Session {
            session_id: SessionId::from("session-1")
        }
    );
}

#[test]
fn inactive_status_becomes_ended() {
    let mut value = task_json();
    value["status"] = json!("inactive");
    assert_eq!(
        decode_schedule_task(&value).unwrap().status,
        ScheduleStatus::Ended
    );
}

#[test]
fn non_string_status_is_not_defaulted_to_active() {
    let mut value = task_json();
    value["status"] = json!(null);
    assert_eq!(
        decode_schedule_task(&value).unwrap_err().to_string(),
        "status must be active, paused, ended, or inactive"
    );
}

#[test]
fn empty_legacy_session_binding_is_rejected() {
    let mut value = task_json();
    value["sessionId"] = json!("");
    assert_eq!(
        decode_schedule_task(&value).unwrap_err().to_string(),
        "sessionId must be non-empty"
    );
}

#[test]
fn explicit_empty_new_once_session_binding_is_rejected() {
    let mut value = task_json();
    value["target"] = json!({"kind":"new_once","sessionId":"","create":{}});
    assert_eq!(
        decode_schedule_task(&value).unwrap_err().to_string(),
        "target sessionId must be non-empty"
    );
}

#[test]
fn obsolete_legacy_binding_is_ignored_when_a_target_is_present() {
    let mut value = task_json();
    value["sessionId"] = json!(42);
    value["target"] = json!({"kind":"session","sessionId":"session-current"});
    assert!(matches!(
        decode_schedule_task(&value).unwrap().target,
        ScheduleTarget::Session { session_id } if session_id.as_str() == "session-current"
    ));
}

#[test]
fn history_without_history_marks_legacy_receipts_unavailable() {
    let task = decode_schedule_task(&task_json()).unwrap();
    assert!(task.delivery_history.earlier_records_unavailable);
    assert!(task.delivery_history.records.is_empty());
}

fn task() -> ScheduleTask {
    decode_schedule_task(&json!({
        "record": {"id":"schedule-1","kind":"every","title":"t","prompt":"send this","everySeconds":60,"scheduledAt":"2025-01-01T00:01:00.000Z"},
        "target":{"kind":"session","sessionId":"session-1"},
        "status":"active",
        "deliveryHistory":{"records":[],"earlierRecordsUnavailable":false,"earlierRecordsPruned":false}
    })).unwrap()
}

fn receipt(index: u8) -> ScheduleDeliveryReceipt {
    ScheduleDeliveryReceipt {
        scheduled_at: format!("2025-01-0{index}T00:00:00.000Z"),
        delivered_at: format!("2025-01-0{index}T00:00:01.000Z"),
        message_id: MessageId::from(format!("message-{index}")),
        target_session_id: SessionId::from("session-1"),
    }
}

#[test]
fn append_saves_immutable_prompt() {
    let (_last, history) = append_delivery(
        &task(),
        receipt(1),
        DeliveryRetentionBounds {
            days: 30,
            records: 10,
        },
    );
    assert_eq!(history.records[0].prompt.as_deref(), Some("send this"));
}

#[test]
fn append_prunes_record_cap_and_marks_both_flags() {
    let mut current = task();
    for index in 1..=3 {
        let (last, history) = append_delivery(
            &current,
            receipt(index),
            DeliveryRetentionBounds {
                days: 30,
                records: 2,
            },
        );
        current.last_delivery = Some(last);
        current.delivery_history = history;
    }
    assert_eq!(current.delivery_history.records.len(), 2);
    assert!(current.delivery_history.earlier_records_unavailable);
    assert!(current.delivery_history.earlier_records_pruned);
}

#[test]
fn history_page_is_newest_first_with_cursor() {
    let mut current = task();
    for index in 1..=3 {
        let (last, history) = append_delivery(
            &current,
            receipt(index),
            DeliveryRetentionBounds {
                days: 30,
                records: 10,
            },
        );
        current.last_delivery = Some(last);
        current.delivery_history = history;
    }
    let result = delivery_history_page(
        &current,
        &ScheduleDeliveryHistoryRequest {
            id: ScheduleId::from("schedule-1"),
            limit: 2,
            before: None,
        },
        DeliveryRetentionBounds {
            days: 30,
            records: 10,
        },
    );
    let ScheduleDeliveryHistoryResult::Page(page) = result else {
        panic!("wrong result")
    };
    assert_eq!(
        page.records
            .iter()
            .map(|record| record.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["message-3", "message-2"]
    );
    assert_eq!(
        page.next_before.as_ref().map(MessageId::as_str),
        Some("message-2")
    );
}

#[test]
fn unknown_history_cursor_is_non_mutating_miss() {
    let result = delivery_history_page(
        &task(),
        &ScheduleDeliveryHistoryRequest {
            id: ScheduleId::from("schedule-1"),
            limit: 10,
            before: Some(MessageId::from("missing")),
        },
        DeliveryRetentionBounds {
            days: 30,
            records: 10,
        },
    );
    assert_eq!(
        result,
        ScheduleDeliveryHistoryResult::Miss {
            id: ScheduleId::from("schedule-1"),
            code: ScheduleDeliveryHistoryMissCode::DeliveryCursorNotFound
        }
    );
}

#[test]
fn duplicate_saved_message_ids_are_rejected() {
    let mut value = task_json();
    value["target"] = json!({"kind":"session","sessionId":"session-1"});
    value["lastDelivery"] = json!({"scheduledAt":"2025-01-01T00:00:00.000Z","deliveredAt":"2025-01-01T00:00:01.000Z","messageId":"same","targetSessionId":"session-1"});
    value["deliveryHistory"] = json!({"records":[
        {"scheduledAt":"2025-01-01T00:00:00.000Z","deliveredAt":"2025-01-01T00:00:01.000Z","messageId":"same","targetSessionId":"session-1"},
        {"scheduledAt":"2025-01-01T00:00:00.000Z","deliveredAt":"2025-01-01T00:00:01.000Z","messageId":"same","targetSessionId":"session-1"}
    ],"earlierRecordsUnavailable":false});
    assert!(
        decode_schedule_task(&value)
            .unwrap_err()
            .to_string()
            .contains("unique")
    );
}

#[test]
fn unbound_new_once_legacy_receipt_must_name_target_session() {
    let mut value = task_json();
    value["target"] = json!({"kind":"new_once","create":{}});
    value["lastDelivery"] = json!({
        "scheduledAt":"2025-01-01T00:00:00.000Z",
        "deliveredAt":"2025-01-01T00:00:01.000Z",
        "messageId":"message-1"
    });
    assert!(
        decode_schedule_task(&value)
            .unwrap_err()
            .to_string()
            .contains("must name its target Session")
    );
}

#[test]
fn delivery_receipts_are_strict_objects() {
    let mut value = task_json();
    value["target"] = json!({"kind":"session","sessionId":"session-1"});
    value["lastDelivery"] = json!({
        "scheduledAt":"2025-01-01T00:00:00.000Z",
        "deliveredAt":"2025-01-01T00:00:01.000Z",
        "messageId":"message-1",
        "extra":true
    });
    assert!(
        decode_schedule_task(&value)
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );
}

#[test]
fn domain_descriptor_matches_source_versions() {
    let spec = schedule_domain_spec().unwrap();
    assert_eq!(spec.name, "schedule");
    assert_eq!(spec.version, 2);
    assert_eq!(spec.compatible_versions, Some(vec![1]));
    assert!(spec.tables.contains_key("tasks"));
}
