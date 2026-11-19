//! Typed timing construction and edits shared by Schedule management callers.

use crate::{
    ScheduleId, ScheduleInputError, ScheduleInputErrorCode, SchedulePatch, ScheduleRecord,
    ScheduleTiming, create_after_schedule_record, create_at_schedule_record,
    create_cron_schedule_record, create_daily_schedule_record, create_every_schedule_record,
    create_rrule_schedule_record, create_weekly_schedule_record, normalize_rrule, parse_at_input,
    parse_cron_input, parse_daily_input, parse_weekly_input, schedule_title,
};
use hoh_util_values::js_trim;

pub fn create_schedule_record(
    id: ScheduleId,
    title: &str,
    prompt: &str,
    timing: &ScheduleTiming,
    now: i64,
) -> Result<ScheduleRecord, ScheduleInputError> {
    match timing {
        ScheduleTiming::After { after_seconds } => {
            create_after_schedule_record(id, prompt, *after_seconds, now, title)
        }
        ScheduleTiming::At { at } => create_at_schedule_record(id, prompt, at, now, title),
        ScheduleTiming::Every { every_seconds } => {
            create_every_schedule_record(id, prompt, *every_seconds, now, title)
        }
        ScheduleTiming::Daily { daily } => {
            create_daily_schedule_record(id, prompt, daily, now, title)
        }
        ScheduleTiming::Weekly { weekly } => {
            create_weekly_schedule_record(id, prompt, weekly, now, title)
        }
        ScheduleTiming::Cron { cron } => create_cron_schedule_record(id, prompt, cron, now, title),
        ScheduleTiming::Rrule { rrule } => {
            create_rrule_schedule_record(id, title, prompt, rrule, now)
        }
    }
}

pub fn timing_of(record: &ScheduleRecord) -> ScheduleTiming {
    match record {
        ScheduleRecord::After { scheduled_at, .. } | ScheduleRecord::At { scheduled_at, .. } => {
            ScheduleTiming::At {
                at: crate::AtInput::Instant(scheduled_at.clone()),
            }
        }
        ScheduleRecord::Every { every_seconds, .. } => ScheduleTiming::Every {
            every_seconds: *every_seconds,
        },
        ScheduleRecord::Daily {
            time, time_zone, ..
        } => ScheduleTiming::Daily {
            daily: crate::DailyInput {
                time: time.clone(),
                time_zone: time_zone.clone(),
            },
        },
        ScheduleRecord::Weekly {
            time,
            time_zone,
            weekdays,
            ..
        } => ScheduleTiming::Weekly {
            weekly: crate::WeeklyInput {
                time: time.clone(),
                time_zone: time_zone.clone(),
                weekdays: weekdays.clone(),
            },
        },
        ScheduleRecord::Cron {
            expression,
            time_zone,
            ..
        } => ScheduleTiming::Cron {
            cron: crate::CronInput {
                expression: expression.clone(),
                time_zone: time_zone.clone(),
            },
        },
        ScheduleRecord::Rrule { rrule, .. } => ScheduleTiming::Rrule {
            rrule: rrule.clone(),
        },
    }
}

fn normalized_timing(value: &ScheduleTiming) -> Result<String, ScheduleInputError> {
    let value = match value {
        ScheduleTiming::At { at } => format!("at:{}", parse_at_input(at)?),
        ScheduleTiming::Daily { daily } => format!("daily:{:?}", parse_daily_input(daily)?),
        ScheduleTiming::Weekly { weekly } => format!("weekly:{:?}", parse_weekly_input(weekly)?),
        ScheduleTiming::Cron { cron } => format!("cron:{:?}", parse_cron_input(cron)?),
        ScheduleTiming::Rrule { rrule } => format!("rrule:{}", normalize_rrule(rrule)?),
        other => format!("{other:?}"),
    };
    Ok(value)
}

pub fn update_schedule_record(
    current: &ScheduleRecord,
    patch: &SchedulePatch,
    now: i64,
    resume: bool,
) -> Result<ScheduleRecord, ScheduleInputError> {
    let title = schedule_title(patch.title.as_deref().unwrap_or(current.title()))?;
    let raw_prompt = patch.prompt.as_deref().unwrap_or(current.prompt());
    let prompt = js_trim(raw_prompt);
    if prompt.is_empty() {
        return Err(ScheduleInputError::new(
            ScheduleInputErrorCode::InvalidPrompt,
            "prompt must be non-empty after trimming.",
        ));
    }
    let old_timing = timing_of(current);
    let timing = patch.timing.as_ref().unwrap_or(&old_timing);
    let changed =
        patch.timing.is_some() && normalized_timing(timing)? != normalized_timing(&old_timing)?;
    if matches!(current, ScheduleRecord::Rrule { .. })
        && let ScheduleTiming::Rrule { rrule } = timing
    {
        return crate::update_rrule_schedule_record(
            current,
            &title,
            prompt,
            changed.then_some(rrule.as_str()),
            now,
            changed || resume,
        )
        .map_err(|error| {
            if resume && !changed && error.code() == ScheduleInputErrorCode::NotFuture {
                ScheduleInputError::new(
                    ScheduleInputErrorCode::NotFuture,
                    "This task has no future occurrence. Set a future schedule before resuming it.",
                )
            } else {
                error
            }
        });
    }
    if changed || resume {
        return create_schedule_record(current.id().clone(), &title, prompt, timing, now).map_err(|error| {
            if resume && !changed && error.code() == ScheduleInputErrorCode::NotFuture {
                ScheduleInputError::new(ScheduleInputErrorCode::NotFuture, "This task has no future occurrence. Set a future schedule before resuming it.")
            } else {
                error
            }
        });
    }
    if title == current.title() && prompt == current.prompt() {
        return Ok(current.clone());
    }
    let mut value = serde_json::to_value(current).expect("schedule record is serializable");
    value["title"] = serde_json::Value::String(title);
    value["prompt"] = serde_json::Value::String(prompt.to_owned());
    serde_json::from_value(value).map_err(|error| {
        ScheduleInputError::new(ScheduleInputErrorCode::InvalidRule, error.to_string())
    })
}
