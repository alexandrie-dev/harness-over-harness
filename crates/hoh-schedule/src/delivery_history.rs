//! Saved delivery retention and identity-based pagination without Session reads.

use hoh_util_time::{InstantResult, four_digit_year_epoch};

use crate::{
    DeliveryHistory, DeliveryRetentionBounds, ScheduleDeliveryHistoryMissCode,
    ScheduleDeliveryHistoryPage, ScheduleDeliveryHistoryRequest, ScheduleDeliveryHistoryResult,
    ScheduleDeliveryReceipt, ScheduleTask,
};

const DAY_MS: i64 = 86_400_000;

fn epoch(value: &str) -> i64 {
    match four_digit_year_epoch(value) {
        InstantResult::Accepted { epoch } => epoch,
        InstantResult::Rejected { .. } => i64::MIN,
    }
}

pub fn append_delivery(
    task: &ScheduleTask,
    receipt: ScheduleDeliveryReceipt,
    bounds: DeliveryRetentionBounds,
) -> (ScheduleDeliveryReceipt, DeliveryHistory) {
    let mut records = task.delivery_history.records.clone();
    records.push(crate::ScheduleDeliveryRecord {
        scheduled_at: receipt.scheduled_at.clone(),
        delivered_at: receipt.delivered_at.clone(),
        message_id: receipt.message_id.clone(),
        target_session_id: receipt.target_session_id.clone(),
        prompt: Some(task.record.prompt().to_owned()),
    });
    let floor = epoch(&receipt.delivered_at).saturating_sub(
        i64::try_from(bounds.days)
            .unwrap_or(i64::MAX)
            .saturating_mul(DAY_MS),
    );
    let original_len = records.len();
    records.retain(|record| epoch(&record.delivered_at) >= floor);
    if records.len() > bounds.records {
        records.drain(..records.len() - bounds.records);
    }
    let pruned = records.len() != original_len;
    (
        receipt,
        DeliveryHistory {
            records,
            earlier_records_unavailable: task.delivery_history.earlier_records_unavailable
                || pruned,
            earlier_records_pruned: task.delivery_history.earlier_records_pruned || pruned,
        },
    )
}

pub fn delivery_history_page(
    task: &ScheduleTask,
    request: &ScheduleDeliveryHistoryRequest,
    retention: DeliveryRetentionBounds,
) -> ScheduleDeliveryHistoryResult {
    let records = &task.delivery_history.records;
    let end = match &request.before {
        None => records.len(),
        Some(before) => match records
            .iter()
            .position(|record| &record.message_id == before)
        {
            Some(index) => index,
            None => {
                return ScheduleDeliveryHistoryResult::Miss {
                    id: request.id.clone(),
                    code: ScheduleDeliveryHistoryMissCode::DeliveryCursorNotFound,
                };
            }
        },
    };
    let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
    let start = end.saturating_sub(limit);
    let page_records: Vec<_> = records[start..end].iter().rev().cloned().collect();
    let next_before = (start > 0).then(|| {
        page_records
            .last()
            .expect("nonempty page")
            .message_id
            .clone()
    });
    ScheduleDeliveryHistoryResult::Page(ScheduleDeliveryHistoryPage {
        id: request.id.clone(),
        records: page_records,
        earlier_records_unavailable: task.delivery_history.earlier_records_unavailable,
        earlier_records_pruned: task.delivery_history.earlier_records_pruned,
        retention,
        next_before,
    })
}
