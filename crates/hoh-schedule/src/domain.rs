//! Host task decoding, timing validation, occurrence selection, and reminder framing.

use std::collections::BTreeSet;
use std::fmt;

use hoh_util_time::{
    Cron, InstantRejection, InstantResult, LocalCalendarRejection, LocalCalendarResult,
    LocalCalendarTime, MAX_FOUR_DIGIT_YEAR_MS, RRule, RRuleParseOptions, RecurrenceOccurrence,
    RecurrenceRejection, RecurrenceResult, TimeZoneRejection, TimeZoneResult,
    canonicalize_time_zone as canonicalize_shared_time_zone, format_four_digit_year_instant,
    four_digit_year_epoch, is_four_digit_year_instant, parse_local_calendar, parse_offset_instant,
    resolve_local_instant,
};
use hoh_util_values::js_trim;
use jiff::{Timestamp, civil::Date, tz::TimeZone};
use serde_json::{Map, Value, json};

use crate::{
    AtInput, CronInput, DailyInput, LocalAtInput, ScheduleId, ScheduleInputErrorCode,
    ScheduleRecord, WeeklyInput,
};

pub const MIN_EVERY_INTERVAL_SECONDS: u64 = 60;
pub const MAX_TITLE_LENGTH: usize = 120;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const REQUIRED_TITLE_MESSAGE: &str = "title is required and must be non-empty after trimming.";
const NON_CANONICAL_INSTANT_MESSAGE: &str =
    "scheduledAt must be a canonical four-digit-year RFC 3339 UTC instant";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleLogError {
    message: String,
}

impl ScheduleLogError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub const fn code(&self) -> &'static str {
        "corrupt_schedule_log"
    }
}

impl fmt::Display for ScheduleLogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for ScheduleLogError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleInputError {
    code: ScheduleInputErrorCode,
    message: String,
}

impl ScheduleInputError {
    pub fn new(code: ScheduleInputErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub const fn code(&self) -> ScheduleInputErrorCode {
        self.code
    }
}

impl fmt::Display for ScheduleInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for ScheduleInputError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecurringOccurrence {
    pub occurrence_at: String,
    pub next_scheduled_at: Option<String>,
}

fn input(code: ScheduleInputErrorCode, message: impl Into<String>) -> ScheduleInputError {
    ScheduleInputError::new(code, message)
}

pub fn schedule_title(title: &str) -> Result<String, ScheduleInputError> {
    let normalized = js_trim(title);
    if normalized.is_empty() {
        return Err(input(
            ScheduleInputErrorCode::InvalidPrompt,
            REQUIRED_TITLE_MESSAGE,
        ));
    }
    if normalized.encode_utf16().count() > MAX_TITLE_LENGTH {
        return Err(input(
            ScheduleInputErrorCode::InvalidPrompt,
            format!("title must be at most {MAX_TITLE_LENGTH} characters."),
        ));
    }
    Ok(normalized.to_owned())
}

pub fn decode_stored_title(value: &Value) -> Result<String, ScheduleLogError> {
    let Some(value) = value.as_str() else {
        return Err(ScheduleLogError::new(REQUIRED_TITLE_MESSAGE));
    };
    if js_trim(value).is_empty() {
        return Err(ScheduleLogError::new(REQUIRED_TITLE_MESSAGE));
    }
    if value.encode_utf16().count() > MAX_TITLE_LENGTH {
        return Err(ScheduleLogError::new(format!(
            "title must be at most {MAX_TITLE_LENGTH} characters"
        )));
    }
    if js_trim(value) != value {
        return Err(ScheduleLogError::new("title must be a trimmed string"));
    }
    Ok(value.to_owned())
}

pub fn has_exact_keys(value: &Map<String, Value>, expected: &[&str]) -> bool {
    value.len() == expected.len() && expected.iter().all(|key| value.contains_key(*key))
}

pub fn decode_id(value: &Value) -> Result<ScheduleId, ScheduleLogError> {
    match value.as_str() {
        Some(text) if !text.is_empty() && js_trim(text) == text => Ok(ScheduleId::from(text)),
        _ => Err(ScheduleLogError::new(
            "schedule id must be a non-empty string without surrounding whitespace",
        )),
    }
}

pub fn decode_instant(value: &Value) -> Result<String, ScheduleLogError> {
    let Some(value) = value.as_str() else {
        return Err(ScheduleLogError::new(NON_CANONICAL_INSTANT_MESSAGE));
    };
    match four_digit_year_epoch(value) {
        InstantResult::Accepted { .. } => Ok(value.to_owned()),
        InstantResult::Rejected {
            reason: InstantRejection::NotACalendarDate,
        } => Err(ScheduleLogError::new(
            "scheduledAt is not a real UTC calendar instant",
        )),
        InstantResult::Rejected { .. } => Err(ScheduleLogError::new(NON_CANONICAL_INSTANT_MESSAGE)),
    }
}

pub fn canonicalize_time_zone(value: &str) -> Result<String, ScheduleInputError> {
    match canonicalize_shared_time_zone(value) {
        TimeZoneResult::Accepted { time_zone } => Ok(time_zone),
        TimeZoneResult::Rejected {
            reason: TimeZoneRejection::NotCanonicalShape,
            ..
        } => Err(input(
            ScheduleInputErrorCode::InvalidTimeZone,
            "time_zone must resolve to UTC or an IANA Area/Location name.",
        )),
        TimeZoneResult::Rejected { .. } => Err(input(
            ScheduleInputErrorCode::InvalidTimeZone,
            "time_zone must be UTC or a valid IANA Area/Location name.",
        )),
    }
}

pub fn future_instant(epoch: i64, now: i64) -> Result<String, ScheduleInputError> {
    if now.unsigned_abs() > MAX_SAFE_INTEGER {
        return Err(input(
            ScheduleInputErrorCode::TimeOutOfRange,
            "The scheduled time must be representable as a four-digit-year RFC 3339 UTC instant.",
        ));
    }
    let Some(instant) = format_four_digit_year_instant(epoch) else {
        return Err(input(
            ScheduleInputErrorCode::TimeOutOfRange,
            "The scheduled time must be representable as a four-digit-year RFC 3339 UTC instant.",
        ));
    };
    if epoch <= now {
        return Err(input(
            ScheduleInputErrorCode::NotFuture,
            "The scheduled time must be strictly in the future.",
        ));
    }
    Ok(instant)
}

fn parse_at_offset_instant(value: &str) -> Result<i64, ScheduleInputError> {
    match parse_offset_instant(value) {
        InstantResult::Accepted { epoch } => Ok(epoch),
        InstantResult::Rejected {
            reason: InstantRejection::NotRfc3339,
        } => Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "at must use YYYY-MM-DDTHH:mm:ss with optional 1-3 digit fractional seconds and an explicit Z or numeric offset.",
        )),
        InstantResult::Rejected {
            reason: InstantRejection::InvalidNumericOffset,
        } => Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "The at numeric offset is invalid.",
        )),
        InstantResult::Rejected { .. } => Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "The at value must be a real ISO calendar date and time.",
        )),
    }
}

fn parse_local_at(value: &LocalAtInput) -> Result<LocalCalendarTime, ScheduleInputError> {
    match parse_local_calendar(&value.date, &value.time) {
        LocalCalendarResult::Accepted { time } => Ok(time),
        LocalCalendarResult::Rejected {
            reason: LocalCalendarRejection::Malformed,
        } => Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "Local at requires date YYYY-MM-DD and time HH:mm:ss with optional one-to-three digit milliseconds.",
        )),
        LocalCalendarResult::Rejected {
            reason: LocalCalendarRejection::NotACalendarDate,
        } => Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "The at value must be a real ISO calendar date and time.",
        )),
        LocalCalendarResult::Rejected { .. } => Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "The local at value must be a real ISO calendar date and time.",
        )),
    }
}

pub fn parse_at_input(at: &AtInput) -> Result<i64, ScheduleInputError> {
    match at {
        AtInput::Instant(value) => parse_at_offset_instant(value),
        AtInput::Local(value) => {
            let local = parse_local_at(value)?;
            let time_zone = canonicalize_time_zone(&value.time_zone)?;
            resolve_local_instant(&local, &time_zone)
                .map_err(|_| {
                    input(
                        ScheduleInputErrorCode::InvalidRule,
                        "The local at value must be a real ISO calendar date and time.",
                    )
                })?
                .ok_or_else(|| {
                    input(
                        ScheduleInputErrorCode::InvalidRule,
                        "The local at time does not exist in the selected time zone.",
                    )
                })
        }
    }
}

fn prompt(value: &str) -> Result<String, ScheduleInputError> {
    let value = js_trim(value);
    if value.is_empty() {
        Err(input(
            ScheduleInputErrorCode::InvalidPrompt,
            "prompt must be non-empty after trimming.",
        ))
    } else {
        Ok(value.to_owned())
    }
}

fn normalize_clock(value: &str, selector: &str) -> Result<String, ScheduleInputError> {
    let bytes = value.as_bytes();
    let valid_base = bytes.len() >= 8
        && bytes[2] == b':'
        && bytes[5] == b':'
        && bytes[..2].iter().all(u8::is_ascii_digit)
        && bytes[3..5].iter().all(u8::is_ascii_digit)
        && bytes[6..8].iter().all(u8::is_ascii_digit);
    let fraction_valid = bytes.len() == 8
        || ((10..=12).contains(&bytes.len())
            && bytes[8] == b'.'
            && bytes[9..].iter().all(u8::is_ascii_digit));
    let number =
        |range: std::ops::Range<usize>| -> u32 { value[range].parse().unwrap_or(u32::MAX) };
    if !valid_base || !fraction_valid || number(0..2) > 23 || number(3..5) > 59 || number(6..8) > 59
    {
        return Err(input(
            ScheduleInputErrorCode::InvalidRule,
            format!(
                "{selector}.time must use HH:mm:ss with optional 1-3 fractional digits, without leap seconds or 24:00."
            ),
        ));
    }
    let fraction = if bytes.len() == 8 {
        "000".to_owned()
    } else {
        format!("{:<03}", &value[9..]).replace(' ', "0")
    };
    Ok(format!("{}.{}", &value[..8], fraction))
}

pub fn normalize_weekdays(weekdays: &[u8]) -> Result<Vec<u8>, ScheduleInputError> {
    if weekdays.is_empty() {
        return Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "weekly.weekdays must be a non-empty array of ISO weekday numbers.",
        ));
    }
    let mut unique = BTreeSet::new();
    for &weekday in weekdays {
        if !(1..=7).contains(&weekday) {
            return Err(input(
                ScheduleInputErrorCode::InvalidRule,
                "Each weekly.weekdays entry must be an integer from 1 (Monday) through 7 (Sunday).",
            ));
        }
        if !unique.insert(weekday) {
            return Err(input(
                ScheduleInputErrorCode::InvalidRule,
                format!("weekly.weekdays must not repeat weekday {weekday}."),
            ));
        }
    }
    Ok(unique.into_iter().collect())
}

pub fn parse_daily_input(daily: &DailyInput) -> Result<DailyInput, ScheduleInputError> {
    Ok(DailyInput {
        time: normalize_clock(&daily.time, "daily")?,
        time_zone: canonicalize_time_zone(&daily.time_zone)?,
    })
}

pub fn parse_weekly_input(weekly: &WeeklyInput) -> Result<WeeklyInput, ScheduleInputError> {
    Ok(WeeklyInput {
        time: normalize_clock(&weekly.time, "weekly")?,
        time_zone: canonicalize_time_zone(&weekly.time_zone)?,
        weekdays: normalize_weekdays(&weekly.weekdays)?,
    })
}

fn recurrence<T>(value: RecurrenceResult<T>) -> Result<T, ScheduleInputError> {
    match value {
        RecurrenceResult::Accepted { rule } => Ok(rule),
        RecurrenceResult::Rejected { reason, detail, .. } => Err(input(
            match reason {
                RecurrenceRejection::Syntax => ScheduleInputErrorCode::InvalidRule,
                RecurrenceRejection::TimeZone => ScheduleInputErrorCode::InvalidTimeZone,
                RecurrenceRejection::Density => ScheduleInputErrorCode::FrequencyTooHigh,
            },
            detail,
        )),
    }
}

pub fn canonicalize_cron_expression(expression: &str) -> Result<String, ScheduleInputError> {
    Ok(recurrence(Cron::parse(expression, "UTC"))?
        .expression()
        .to_owned())
}

pub fn parse_cron_input(cron: &CronInput) -> Result<CronInput, ScheduleInputError> {
    Ok(CronInput {
        expression: canonicalize_cron_expression(&cron.expression)?,
        time_zone: canonicalize_time_zone(&cron.time_zone)?,
    })
}

pub fn normalize_rrule(value: &str) -> Result<String, ScheduleInputError> {
    Ok(
        recurrence(RRule::parse(value, RRuleParseOptions::default()))?
            .expression()
            .to_owned(),
    )
}

fn epoch(value: &str) -> Result<i64, ScheduleLogError> {
    match four_digit_year_epoch(value) {
        InstantResult::Accepted { epoch } => Ok(epoch),
        InstantResult::Rejected { .. } => Err(ScheduleLogError::new(NON_CANONICAL_INSTANT_MESSAGE)),
    }
}

fn local_date(epoch: i64, time_zone: &str) -> Result<Date, ScheduleLogError> {
    let zone =
        TimeZone::get(time_zone).map_err(|error| ScheduleLogError::new(error.to_string()))?;
    Ok(Timestamp::from_millisecond(epoch)
        .map_err(|error| ScheduleLogError::new(error.to_string()))?
        .to_zoned_datetime(zone)
        .date())
}

fn clock_parts(value: &str) -> (u32, u32, u32, u32) {
    let millis = value.get(9..).unwrap_or("000").parse().unwrap_or(0);
    (
        value[0..2].parse().unwrap_or(0),
        value[3..5].parse().unwrap_or(0),
        value[6..8].parse().unwrap_or(0),
        millis,
    )
}

fn local_fields(date: Date, time: &str) -> LocalCalendarTime {
    let (hour, minute, second, millisecond) = clock_parts(time);
    LocalCalendarTime {
        year: i32::from(date.year()),
        month: u32::try_from(date.month()).expect("civil month is positive"),
        day: u32::try_from(date.day()).expect("civil day is positive"),
        hour,
        minute,
        second,
        millisecond,
    }
}

fn next_matching_target(
    time: &str,
    time_zone: &str,
    now: i64,
    weekdays: Option<&[u8]>,
    after_date: Option<Date>,
) -> Result<Option<String>, ScheduleInputError> {
    let mut date = local_date(now, time_zone)
        .map_err(|error| input(ScheduleInputErrorCode::TimeOutOfRange, error.to_string()))?;
    if after_date.is_some_and(|after| date <= after) {
        date = after_date.expect("checked").tomorrow().map_err(|_| {
            input(
                ScheduleInputErrorCode::TimeOutOfRange,
                "No future occurrence is representable as a four-digit-year UTC instant.",
            )
        })?;
    }
    loop {
        let day = u8::try_from(date.weekday().to_monday_one_offset())
            .expect("weekday is in one through seven");
        if weekdays.is_none_or(|days| days.contains(&day)) {
            let target =
                resolve_local_instant(&local_fields(date, time), time_zone).map_err(|error| {
                    input(ScheduleInputErrorCode::TimeOutOfRange, error.to_string())
                })?;
            if let Some(target) = target
                && target >= hoh_util_time::MIN_FOUR_DIGIT_YEAR_MS
                && target > now
            {
                return Ok(format_four_digit_year_instant(target));
            }
        }
        let Ok(next) = date.tomorrow() else {
            return Ok(None);
        };
        date = next;
    }
}

fn latest_due_occurrence(
    time_zone: &str,
    weekdays: Option<&[u8]>,
    time: &str,
    accepted_at: i64,
    saved_target: i64,
) -> Result<i64, ScheduleLogError> {
    let mut date = local_date(accepted_at, "UTC")?
        .tomorrow()
        .map_err(|error| ScheduleLogError::new(error.to_string()))?;
    loop {
        let day = u8::try_from(date.weekday().to_monday_one_offset())
            .expect("weekday is in one through seven");
        if weekdays.is_none_or(|days| days.contains(&day))
            && let Some(candidate) = resolve_local_instant(&local_fields(date, time), time_zone)
                .map_err(|error| ScheduleLogError::new(error.to_string()))?
            && candidate <= accepted_at
        {
            return Ok(candidate.max(saved_target));
        }
        date = date
            .yesterday()
            .map_err(|error| ScheduleLogError::new(error.to_string()))?;
    }
}

pub fn accepted_decision(selector: &str, accepted_at: i64) -> Result<i64, ScheduleLogError> {
    if is_four_digit_year_instant(accepted_at) {
        Ok(accepted_at)
    } else {
        Err(ScheduleLogError::new(format!(
            "{selector} acceptedAt must be a representable four-digit-year instant"
        )))
    }
}

pub fn create_after_schedule_record(
    id: ScheduleId,
    prompt_text: &str,
    after_seconds: u64,
    now: i64,
    title: &str,
) -> Result<ScheduleRecord, ScheduleInputError> {
    let normalized_prompt = prompt(prompt_text)?;
    if after_seconds == 0 || after_seconds > MAX_SAFE_INTEGER {
        return Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "after_seconds must be a positive safe integer.",
        ));
    }
    let target = now.checked_add((after_seconds * 1_000) as i64).ok_or_else(|| input(ScheduleInputErrorCode::TimeOutOfRange, "The scheduled time must be representable as a four-digit-year RFC 3339 UTC instant."))?;
    Ok(ScheduleRecord::After {
        id,
        title: schedule_title(title)?,
        prompt: normalized_prompt,
        after_seconds,
        scheduled_at: future_instant(target, now)?,
    })
}

pub fn create_at_schedule_record(
    id: ScheduleId,
    prompt_text: &str,
    at: &AtInput,
    now: i64,
    title: &str,
) -> Result<ScheduleRecord, ScheduleInputError> {
    let normalized_prompt = prompt(prompt_text)?;
    Ok(ScheduleRecord::At {
        id,
        title: schedule_title(title)?,
        prompt: normalized_prompt,
        scheduled_at: future_instant(parse_at_input(at)?, now)?,
    })
}

pub fn create_every_schedule_record(
    id: ScheduleId,
    prompt_text: &str,
    every_seconds: u64,
    now: i64,
    title: &str,
) -> Result<ScheduleRecord, ScheduleInputError> {
    let normalized_prompt = prompt(prompt_text)?;
    if every_seconds > MAX_SAFE_INTEGER {
        return Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "every_seconds must be a safe integer.",
        ));
    }
    if every_seconds < MIN_EVERY_INTERVAL_SECONDS {
        return Err(input(
            ScheduleInputErrorCode::FrequencyTooHigh,
            format!("every_seconds must be at least {MIN_EVERY_INTERVAL_SECONDS}."),
        ));
    }
    let interval = every_seconds
        .checked_mul(1_000)
        .and_then(|v| i64::try_from(v).ok())
        .ok_or_else(|| {
            input(
                ScheduleInputErrorCode::InvalidRule,
                "every_seconds must be a safe integer.",
            )
        })?;
    let target = now.checked_add(interval).ok_or_else(|| {
        input(
            ScheduleInputErrorCode::TimeOutOfRange,
            "The scheduled time must be representable as a four-digit-year RFC 3339 UTC instant.",
        )
    })?;
    Ok(ScheduleRecord::Every {
        id,
        title: schedule_title(title)?,
        prompt: normalized_prompt,
        every_seconds,
        scheduled_at: future_instant(target, now)?,
    })
}

pub fn create_daily_schedule_record(
    id: ScheduleId,
    prompt_text: &str,
    daily: &DailyInput,
    now: i64,
    title: &str,
) -> Result<ScheduleRecord, ScheduleInputError> {
    let normalized_prompt = prompt(prompt_text)?;
    let daily = parse_daily_input(daily)?;
    if !is_four_digit_year_instant(now) {
        return Err(input(
            ScheduleInputErrorCode::TimeOutOfRange,
            "Daily creation time must be a representable four-digit-year UTC instant.",
        ));
    }
    let scheduled_at = next_matching_target(&daily.time, &daily.time_zone, now, None, None)?
        .ok_or_else(|| {
            input(
                ScheduleInputErrorCode::TimeOutOfRange,
                "No future daily occurrence is representable as a four-digit-year UTC instant.",
            )
        })?;
    Ok(ScheduleRecord::Daily {
        id,
        title: schedule_title(title)?,
        prompt: normalized_prompt,
        time: daily.time,
        time_zone: daily.time_zone,
        scheduled_at,
    })
}

pub fn create_weekly_schedule_record(
    id: ScheduleId,
    prompt_text: &str,
    weekly: &WeeklyInput,
    now: i64,
    title: &str,
) -> Result<ScheduleRecord, ScheduleInputError> {
    let normalized_prompt = prompt(prompt_text)?;
    let weekly = parse_weekly_input(weekly)?;
    if !is_four_digit_year_instant(now) {
        return Err(input(
            ScheduleInputErrorCode::TimeOutOfRange,
            "Weekly creation time must be a representable four-digit-year UTC instant.",
        ));
    }
    let scheduled_at = next_matching_target(
        &weekly.time,
        &weekly.time_zone,
        now,
        Some(&weekly.weekdays),
        None,
    )?
    .ok_or_else(|| {
        input(
            ScheduleInputErrorCode::TimeOutOfRange,
            "No future weekly occurrence is representable as a four-digit-year UTC instant.",
        )
    })?;
    Ok(ScheduleRecord::Weekly {
        id,
        title: schedule_title(title)?,
        prompt: normalized_prompt,
        time: weekly.time,
        time_zone: weekly.time_zone,
        weekdays: weekly.weekdays,
        scheduled_at,
    })
}

pub fn create_cron_schedule_record(
    id: ScheduleId,
    prompt_text: &str,
    cron: &CronInput,
    now: i64,
    title: &str,
) -> Result<ScheduleRecord, ScheduleInputError> {
    let normalized_prompt = prompt(prompt_text)?;
    let cron = parse_cron_input(cron)?;
    if !is_four_digit_year_instant(now) {
        return Err(input(
            ScheduleInputErrorCode::TimeOutOfRange,
            "Cron creation time must be a representable four-digit-year UTC instant.",
        ));
    }
    let rule = recurrence(Cron::parse(&cron.expression, &cron.time_zone))?;
    let scheduled_at = rule
        .next(now)
        .map_err(|error| input(ScheduleInputErrorCode::TimeOutOfRange, error.to_string()))?
        .ok_or_else(|| {
            input(
                ScheduleInputErrorCode::TimeOutOfRange,
                "No future cron occurrence is representable as a four-digit-year UTC instant.",
            )
        })?;
    Ok(ScheduleRecord::Cron {
        id,
        title: schedule_title(title)?,
        prompt: normalized_prompt,
        expression: cron.expression,
        time_zone: cron.time_zone,
        scheduled_at,
    })
}

pub fn create_rrule_schedule_record(
    id: ScheduleId,
    title: &str,
    prompt_text: &str,
    rrule: &str,
    now: i64,
) -> Result<ScheduleRecord, ScheduleInputError> {
    let normalized_title = schedule_title(title)?;
    let normalized_prompt = prompt(prompt_text)?;
    let rule = recurrence(RRule::parse(
        rrule,
        RRuleParseOptions {
            reject_ineffective_set_positions: true,
        },
    ))?;
    if !is_four_digit_year_instant(now) {
        return Err(input(
            ScheduleInputErrorCode::TimeOutOfRange,
            "The recurrence reference time is outside the supported range.",
        ));
    }
    let scheduled_at = rule
        .next(now)
        .map_err(|error| input(ScheduleInputErrorCode::TimeOutOfRange, error.to_string()))?
        .ok_or_else(|| {
            input(
                ScheduleInputErrorCode::NotFuture,
                "rrule must contain an occurrence strictly in the future.",
            )
        })?;
    Ok(ScheduleRecord::Rrule {
        id,
        title: normalized_title,
        prompt: normalized_prompt,
        rrule: rule.expression().to_owned(),
        scheduled_at,
    })
}

pub fn update_rrule_schedule_record(
    current: &ScheduleRecord,
    title: &str,
    prompt_text: &str,
    changed_rrule: Option<&str>,
    now: i64,
    recompute: bool,
) -> Result<ScheduleRecord, ScheduleInputError> {
    let ScheduleRecord::Rrule {
        id,
        rrule,
        scheduled_at,
        ..
    } = current
    else {
        return Err(input(
            ScheduleInputErrorCode::InvalidRule,
            "record must be rrule",
        ));
    };
    let title = schedule_title(title)?;
    let prompt = prompt(prompt_text)?;
    let rule = recurrence(RRule::parse(
        changed_rrule.unwrap_or(rrule),
        RRuleParseOptions {
            reject_ineffective_set_positions: changed_rrule.is_some(),
        },
    ))?;
    let next = if recompute {
        if !is_four_digit_year_instant(now) {
            return Err(input(
                ScheduleInputErrorCode::TimeOutOfRange,
                "The recurrence reference time is outside the supported range.",
            ));
        }
        rule.next(now)
            .map_err(|error| input(ScheduleInputErrorCode::TimeOutOfRange, error.to_string()))?
            .ok_or_else(|| {
                input(
                    ScheduleInputErrorCode::NotFuture,
                    "rrule must contain an occurrence strictly in the future.",
                )
            })?
    } else {
        scheduled_at.clone()
    };
    Ok(ScheduleRecord::Rrule {
        id: id.clone(),
        title,
        prompt,
        rrule: rule.expression().to_owned(),
        scheduled_at: next,
    })
}

pub fn resolve_every_occurrence(
    record: &ScheduleRecord,
    accepted_at: i64,
) -> Result<RecurringOccurrence, ScheduleLogError> {
    let ScheduleRecord::Every {
        every_seconds,
        scheduled_at,
        ..
    } = record
    else {
        return Err(ScheduleLogError::new("record must be every"));
    };
    let target = epoch(scheduled_at)?;
    let interval = i64::try_from(*every_seconds)
        .ok()
        .and_then(|v| v.checked_mul(1_000))
        .ok_or_else(|| {
            ScheduleLogError::new("every interval milliseconds must be a positive safe integer")
        })?;
    if !is_four_digit_year_instant(accepted_at) {
        return Err(ScheduleLogError::new(
            "every acceptedAt must be a representable four-digit-year instant",
        ));
    }
    if accepted_at < target {
        return Err(ScheduleLogError::new(
            "every dispatch cannot precede the active scheduledAt",
        ));
    }
    let occurrence = target + ((accepted_at - target) / interval) * interval;
    let next = occurrence
        .checked_add(interval)
        .filter(|v| is_four_digit_year_instant(*v))
        .and_then(format_four_digit_year_instant);
    Ok(RecurringOccurrence {
        occurrence_at: format_four_digit_year_instant(occurrence).expect("bounded occurrence"),
        next_scheduled_at: next,
    })
}

pub fn resolve_daily_occurrence(
    record: &ScheduleRecord,
    accepted_at: i64,
) -> Result<RecurringOccurrence, ScheduleLogError> {
    let ScheduleRecord::Daily {
        time,
        time_zone,
        scheduled_at,
        ..
    } = record
    else {
        return Err(ScheduleLogError::new("record must be daily"));
    };
    accepted_decision("daily", accepted_at)?;
    let saved = epoch(scheduled_at)?;
    if accepted_at < saved {
        return Err(ScheduleLogError::new(
            "daily dispatch cannot precede the active scheduledAt",
        ));
    }
    let occurrence = latest_due_occurrence(time_zone, None, time, accepted_at, saved)?;
    let next = next_matching_target(
        time,
        time_zone,
        accepted_at,
        None,
        Some(local_date(occurrence, time_zone)?),
    )
    .map_err(|error| ScheduleLogError::new(error.to_string()))?;
    Ok(RecurringOccurrence {
        occurrence_at: format_four_digit_year_instant(occurrence).expect("bounded occurrence"),
        next_scheduled_at: next,
    })
}

pub fn resolve_weekly_occurrence(
    record: &ScheduleRecord,
    accepted_at: i64,
) -> Result<RecurringOccurrence, ScheduleLogError> {
    let ScheduleRecord::Weekly {
        time,
        time_zone,
        weekdays,
        scheduled_at,
        ..
    } = record
    else {
        return Err(ScheduleLogError::new("record must be weekly"));
    };
    accepted_decision("weekly", accepted_at)?;
    let saved = epoch(scheduled_at)?;
    if accepted_at < saved {
        return Err(ScheduleLogError::new(
            "weekly dispatch cannot precede the active scheduledAt",
        ));
    }
    let occurrence = latest_due_occurrence(time_zone, Some(weekdays), time, accepted_at, saved)?;
    let next = next_matching_target(
        time,
        time_zone,
        accepted_at,
        Some(weekdays),
        Some(local_date(occurrence, time_zone)?),
    )
    .map_err(|error| ScheduleLogError::new(error.to_string()))?;
    Ok(RecurringOccurrence {
        occurrence_at: format_four_digit_year_instant(occurrence).expect("bounded occurrence"),
        next_scheduled_at: next,
    })
}

fn recurrence_occurrence(value: RecurrenceOccurrence) -> RecurringOccurrence {
    RecurringOccurrence {
        occurrence_at: value.occurrence_at,
        next_scheduled_at: value.next_scheduled_at,
    }
}

pub fn resolve_cron_occurrence(
    record: &ScheduleRecord,
    accepted_at: i64,
) -> Result<RecurringOccurrence, ScheduleLogError> {
    let ScheduleRecord::Cron {
        expression,
        time_zone,
        scheduled_at,
        ..
    } = record
    else {
        return Err(ScheduleLogError::new("record must be cron"));
    };
    accepted_decision("cron", accepted_at)?;
    if accepted_at < epoch(scheduled_at)? {
        return Err(ScheduleLogError::new(
            "cron dispatch cannot precede the active scheduledAt",
        ));
    }
    let rule = recurrence(Cron::parse(expression, time_zone))
        .map_err(|error| ScheduleLogError::new(error.to_string()))?;
    rule.resolve(scheduled_at, accepted_at)
        .map(recurrence_occurrence)
        .map_err(|error| ScheduleLogError::new(error.to_string()))
}

pub fn resolve_rrule_occurrence(
    record: &ScheduleRecord,
    accepted_at: i64,
) -> Result<RecurringOccurrence, ScheduleLogError> {
    let ScheduleRecord::Rrule {
        rrule,
        scheduled_at,
        ..
    } = record
    else {
        return Err(ScheduleLogError::new("record must be rrule"));
    };
    accepted_decision("rrule", accepted_at)?;
    if accepted_at < epoch(scheduled_at)? {
        return Err(ScheduleLogError::new(
            "rrule dispatch cannot precede the active scheduledAt",
        ));
    }
    let rule = recurrence(RRule::parse(rrule, RRuleParseOptions::default()))
        .map_err(|error| ScheduleLogError::new(error.to_string()))?;
    rule.resolve(scheduled_at, accepted_at)
        .map(recurrence_occurrence)
        .map_err(|error| ScheduleLogError::new(error.to_string()))
}

pub fn is_recurring_schedule_record(record: &ScheduleRecord) -> bool {
    matches!(
        record,
        ScheduleRecord::Every { .. }
            | ScheduleRecord::Daily { .. }
            | ScheduleRecord::Weekly { .. }
            | ScheduleRecord::Cron { .. }
            | ScheduleRecord::Rrule { .. }
    )
}

pub fn resolve_recurring_occurrence(
    record: &ScheduleRecord,
    accepted_at: i64,
) -> Result<RecurringOccurrence, ScheduleLogError> {
    match record {
        ScheduleRecord::Every { .. } => resolve_every_occurrence(record, accepted_at),
        ScheduleRecord::Daily { .. } => resolve_daily_occurrence(record, accepted_at),
        ScheduleRecord::Weekly { .. } => resolve_weekly_occurrence(record, accepted_at),
        ScheduleRecord::Cron { .. } => resolve_cron_occurrence(record, accepted_at),
        ScheduleRecord::Rrule { .. } => resolve_rrule_occurrence(record, accepted_at),
        _ => Err(ScheduleLogError::new("one-shot schedule is not recurring")),
    }
}

pub fn render_reminder_framing(record: &ScheduleRecord) -> String {
    format!(
        "[SCHEDULE REMINDER]\nThis is a scheduled message from the user\nschedule_id_json: {}\noccurrence_at: {}\nreminder_prompt_json: {}",
        serde_json::to_string(record.id().as_str()).expect("string JSON"),
        record.scheduled_at(),
        serde_json::to_string(record.prompt()).expect("string JSON")
    )
}

pub fn render_recurring_reminder_batch_framing(reminders: &[(&ScheduleRecord, String)]) -> String {
    let payload: Vec<Value> = reminders.iter().map(|(record, occurrence)| json!({
        "schedule_id": record.id(), "occurrence_at": occurrence, "reminder_prompt": record.prompt()
    })).collect();
    format!(
        "[SCHEDULE REMINDER BATCH]\nThis is a scheduled message from the user\nreminders_json: {}",
        serde_json::to_string(&payload).expect("JSON values")
    )
}

pub fn format_schedule_local_time(
    record: &ScheduleRecord,
    instant: &str,
) -> Result<String, ScheduleInputError> {
    let zone_name = match record {
        ScheduleRecord::Daily { time_zone, .. }
        | ScheduleRecord::Weekly { time_zone, .. }
        | ScheduleRecord::Cron { time_zone, .. } => time_zone.clone(),
        ScheduleRecord::Rrule { rrule, .. } => {
            recurrence(RRule::parse(rrule, RRuleParseOptions::default()))?
                .time_zone()
                .to_owned()
        }
        _ => "UTC".to_owned(),
    };
    let at = epoch(instant)
        .map_err(|error| input(ScheduleInputErrorCode::InvalidRule, error.to_string()))?;
    let zone = TimeZone::get(&zone_name)
        .map_err(|error| input(ScheduleInputErrorCode::InvalidTimeZone, error.to_string()))?;
    let local = Timestamp::from_millisecond(at)
        .map_err(|error| input(ScheduleInputErrorCode::TimeOutOfRange, error.to_string()))?
        .to_zoned_datetime(zone);
    let offset_seconds = local.offset().seconds();
    let sign = if offset_seconds < 0 { '-' } else { '+' };
    let offset_minutes = offset_seconds.unsigned_abs().saturating_add(30) / 60;
    let offset = format!(
        "{sign}{:02}:{:02}",
        offset_minutes / 60,
        offset_minutes % 60
    );
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{offset}",
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second()
    ))
}

fn required<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a Value, ScheduleLogError> {
    map.get(key)
        .ok_or_else(|| ScheduleLogError::new(format!("missing {key}")))
}

fn stored_prompt(map: &Map<String, Value>, selector: &str) -> Result<String, ScheduleLogError> {
    match required(map, "prompt")?.as_str() {
        Some(value) if !value.is_empty() && js_trim(value) == value => Ok(value.to_owned()),
        _ => Err(ScheduleLogError::new(format!(
            "{selector} prompt must be non-empty and already trimmed"
        ))),
    }
}

pub fn decode_schedule_record(value: &Value) -> Result<ScheduleRecord, ScheduleLogError> {
    let map = value
        .as_object()
        .ok_or_else(|| ScheduleLogError::new("schedule record must be an object"))?;
    let kind = map.get("kind").and_then(Value::as_str).unwrap_or("");
    let expected: &[&str] = match kind {
        "after" => &[
            "id",
            "kind",
            "title",
            "prompt",
            "afterSeconds",
            "scheduledAt",
        ],
        "at" => &["id", "kind", "title", "prompt", "scheduledAt"],
        "every" => &[
            "id",
            "kind",
            "title",
            "prompt",
            "everySeconds",
            "scheduledAt",
        ],
        "daily" => &[
            "id",
            "kind",
            "title",
            "prompt",
            "time",
            "timeZone",
            "scheduledAt",
        ],
        "weekly" => &[
            "id",
            "kind",
            "title",
            "prompt",
            "time",
            "timeZone",
            "weekdays",
            "scheduledAt",
        ],
        "cron" => &[
            "id",
            "kind",
            "title",
            "prompt",
            "expression",
            "timeZone",
            "scheduledAt",
        ],
        "rrule" => &["id", "kind", "title", "prompt", "rrule", "scheduledAt"],
        _ => {
            return Err(ScheduleLogError::new(
                "schedule kind must be \"after\", \"at\", \"every\", \"daily\", \"weekly\", \"cron\", or \"rrule\"",
            ));
        }
    };
    if kind == "rrule" && !has_exact_keys(map, expected) {
        return Err(ScheduleLogError::new(
            "rrule schedule must contain exactly id, kind, title, prompt, rrule, and scheduledAt",
        ));
    }
    let title = decode_stored_title(map.get("title").unwrap_or(&Value::Null))?;
    if !has_exact_keys(map, expected) {
        let message = match kind {
            "after" => {
                "after schedule must contain exactly id, kind, title, prompt, afterSeconds, and scheduledAt"
            }
            "at" => "at schedule must contain exactly id, kind, title, prompt, and scheduledAt",
            "every" => {
                "every schedule must contain exactly id, kind, title, prompt, everySeconds, and scheduledAt"
            }
            "daily" => {
                "daily schedule must contain exactly id, kind, title, prompt, time, timeZone, and scheduledAt"
            }
            "weekly" => {
                "weekly schedule must contain exactly id, kind, title, prompt, time, timeZone, weekdays, and scheduledAt"
            }
            "cron" => {
                "cron schedule must contain exactly id, kind, title, prompt, expression, timeZone, and scheduledAt"
            }
            "rrule" => unreachable!("rrule exact keys were checked before title decoding"),
            _ => unreachable!("schedule kind was matched above"),
        };
        return Err(ScheduleLogError::new(message));
    }
    match kind {
        "after" => {
            let prompt = stored_prompt(map, "after")?;
            let seconds = required(map, "afterSeconds")?
                .as_u64()
                .filter(|v| *v > 0 && *v <= MAX_SAFE_INTEGER)
                .ok_or_else(|| {
                    ScheduleLogError::new("afterSeconds must be a positive safe integer")
                })?;
            Ok(ScheduleRecord::After {
                id: decode_id(required(map, "id")?)?,
                title,
                prompt,
                after_seconds: seconds,
                scheduled_at: decode_instant(required(map, "scheduledAt")?)?,
            })
        }
        "at" => {
            let prompt = stored_prompt(map, "at")?;
            Ok(ScheduleRecord::At {
                id: decode_id(required(map, "id")?)?,
                title,
                prompt,
                scheduled_at: decode_instant(required(map, "scheduledAt")?)?,
            })
        }
        "every" => {
            let prompt = stored_prompt(map, "every")?;
            let seconds = required(map, "everySeconds")?.as_u64().filter(|v| {
                *v >= MIN_EVERY_INTERVAL_SECONDS
                    && *v <= MAX_SAFE_INTEGER
                    && v.checked_mul(1_000).is_some_and(|ms| ms <= MAX_SAFE_INTEGER)
            }).ok_or_else(|| ScheduleLogError::new(format!("everySeconds must be a safe integer of at least {MIN_EVERY_INTERVAL_SECONDS}")))?;
            Ok(ScheduleRecord::Every {
                id: decode_id(required(map, "id")?)?,
                title,
                prompt,
                every_seconds: seconds,
                scheduled_at: decode_instant(required(map, "scheduledAt")?)?,
            })
        }
        "daily" => {
            let prompt = stored_prompt(map, "daily")?;
            let time = required(map, "time")?
                .as_str()
                .ok_or_else(|| ScheduleLogError::new("daily time and timeZone must be strings"))?;
            let zone = required(map, "timeZone")?
                .as_str()
                .ok_or_else(|| ScheduleLogError::new("daily time and timeZone must be strings"))?;
            let parsed = parse_daily_input(&DailyInput {
                time: time.to_owned(),
                time_zone: zone.to_owned(),
            })
            .map_err(|error| ScheduleLogError::new(format!("ScheduleInputError: {error}")))?;
            if parsed.time != time {
                return Err(ScheduleLogError::new(
                    "daily time must be normalized to HH:mm:ss.SSS",
                ));
            }
            Ok(ScheduleRecord::Daily {
                id: decode_id(required(map, "id")?)?,
                title,
                prompt,
                time: parsed.time,
                time_zone: zone.to_owned(),
                scheduled_at: decode_instant(required(map, "scheduledAt")?)?,
            })
        }
        "weekly" => {
            let prompt = stored_prompt(map, "weekly")?;
            let time = required(map, "time")?
                .as_str()
                .ok_or_else(|| ScheduleLogError::new("weekly time and timeZone must be strings"))?;
            let zone = required(map, "timeZone")?
                .as_str()
                .ok_or_else(|| ScheduleLogError::new("weekly time and timeZone must be strings"))?;
            canonicalize_time_zone(zone)
                .map_err(|error| ScheduleLogError::new(format!("ScheduleInputError: {error}")))?;
            let normalized_time = normalize_clock(time, "weekly")
                .map_err(|error| ScheduleLogError::new(format!("ScheduleInputError: {error}")))?;
            if normalized_time != time {
                return Err(ScheduleLogError::new(
                    "weekly time must be normalized to HH:mm:ss.SSS",
                ));
            }
            let weekday_values = required(map, "weekdays")?
                .as_array()
                .filter(|values| !values.is_empty())
                .ok_or_else(|| ScheduleLogError::new("ScheduleInputError: weekly.weekdays must be a non-empty array of ISO weekday numbers."))?;
            let weekdays: Vec<u8> = weekday_values
                .iter()
                .map(|value| {
                    value
                        .as_u64()
                        .filter(|weekday| (1..=7).contains(weekday))
                        .map(|weekday| weekday as u8)
                        .ok_or_else(|| ScheduleLogError::new("ScheduleInputError: Each weekly.weekdays entry must be an integer from 1 (Monday) through 7 (Sunday)."))
                })
                .collect::<Result<_, _>>()?;
            let normalized_weekdays = normalize_weekdays(&weekdays)
                .map_err(|error| ScheduleLogError::new(format!("ScheduleInputError: {error}")))?;
            if normalized_weekdays != weekdays {
                return Err(ScheduleLogError::new(
                    "weekly weekdays must be normalized to unique ascending ISO weekday numbers",
                ));
            }
            Ok(ScheduleRecord::Weekly {
                id: decode_id(required(map, "id")?)?,
                title,
                prompt,
                time: normalized_time,
                time_zone: zone.to_owned(),
                weekdays: normalized_weekdays,
                scheduled_at: decode_instant(required(map, "scheduledAt")?)?,
            })
        }
        "cron" => {
            let prompt = stored_prompt(map, "cron")?;
            let expression = required(map, "expression")?.as_str().ok_or_else(|| {
                ScheduleLogError::new("cron expression and timeZone must be strings")
            })?;
            let zone = required(map, "timeZone")?.as_str().ok_or_else(|| {
                ScheduleLogError::new("cron expression and timeZone must be strings")
            })?;
            canonicalize_time_zone(zone)
                .map_err(|error| ScheduleLogError::new(format!("ScheduleInputError: {error}")))?;
            let canonical = canonicalize_cron_expression(expression)
                .map_err(|error| ScheduleLogError::new(format!("ScheduleInputError: {error}")))?;
            if canonical != expression {
                return Err(ScheduleLogError::new("cron expression must be canonical"));
            }
            Ok(ScheduleRecord::Cron {
                id: decode_id(required(map, "id")?)?,
                title,
                prompt,
                expression: expression.to_owned(),
                time_zone: zone.to_owned(),
                scheduled_at: decode_instant(required(map, "scheduledAt")?)?,
            })
        }
        "rrule" => {
            let prompt = stored_prompt(map, "schedule")?;
            let rrule = required(map, "rrule")?
                .as_str()
                .ok_or_else(|| ScheduleLogError::new("schedule rrule must be a string"))?;
            let normalized = normalize_rrule(rrule).map_err(|error| {
                ScheduleLogError::new(format!(
                    "invalid schedule rrule: ScheduleInputError: {error}"
                ))
            })?;
            if normalized != rrule {
                return Err(ScheduleLogError::new("schedule rrule must be normalized"));
            }
            Ok(ScheduleRecord::Rrule {
                id: decode_id(required(map, "id")?)?,
                title,
                prompt,
                rrule: rrule.to_owned(),
                scheduled_at: decode_instant(required(map, "scheduledAt")?)?,
            })
        }
        _ => unreachable!(),
    }
}

pub fn max_four_digit_year_ms() -> i64 {
    MAX_FOUR_DIGIT_YEAR_MS
}
