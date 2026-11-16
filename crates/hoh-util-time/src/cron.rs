//! Strict five-field Cron parsing and bounded, zone-aware occurrence arithmetic.

use std::collections::BTreeSet;

use crate::calendar::{
    MS_PER_DAY, civil_from_days, days_from_civil, digits_value, is_js_whitespace, iso_string,
    js_trim, weekday_sunday0,
};
use crate::error::TimeError;
use crate::instant::{
    MAX_FOUR_DIGIT_YEAR_MS, MIN_FOUR_DIGIT_YEAR_MS, canonicalize_zone, is_four_digit_year_instant,
};
use crate::recurrence::{
    RecurrenceInputError, RecurrenceOccurrence, RecurrenceRejection, RecurrenceResult,
    parse_recurrence, recurrence_decision,
};
use crate::zone::Zone;

/// Gregorian calendar repetition bounds both forward and backward searches.
pub const CRON_SEARCH_HORIZON_YEARS: i64 = 400;

/// Largest integer JavaScript represents exactly (`Number.MAX_SAFE_INTEGER`).
const MAX_SAFE_INTEGER: i128 = 9_007_199_254_740_991;

/// Field bounds of the supported five-field cron dialect, in evaluation order.
struct CronFieldSpec {
    /// Field name used in diagnostics.
    name: &'static str,
    /// Lowest value accepted in the input dialect.
    min: i64,
    /// Highest value accepted in the input dialect; Sunday accepts both 0 and 7.
    max: i64,
    /// Highest value retained after folding Sunday 7 onto 0.
    canonical_max: i64,
}

const CRON_FIELDS: [CronFieldSpec; 5] = [
    CronFieldSpec {
        name: "minute",
        min: 0,
        max: 59,
        canonical_max: 59,
    },
    CronFieldSpec {
        name: "hour",
        min: 0,
        max: 23,
        canonical_max: 23,
    },
    CronFieldSpec {
        name: "day-of-month",
        min: 1,
        max: 31,
        canonical_max: 31,
    },
    CronFieldSpec {
        name: "month",
        min: 1,
        max: 12,
        canonical_max: 12,
    },
    CronFieldSpec {
        name: "day-of-week",
        min: 0,
        max: 7,
        canonical_max: 6,
    },
];

/// One parsed cron field: its matched values, canonical spelling, and Vixie star flag.
struct ParsedCronField {
    values: Vec<i64>,
    canonical: String,
    star: bool,
}

/// `JSON.stringify` of a string.
fn json_quote(text: &str) -> String {
    let mut quoted = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\u{8}' => quoted.push_str("\\b"),
            '\u{c}' => quoted.push_str("\\f"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            c if (c as u32) < 0x20 => quoted.push_str(&format!("\\u{:04x}", c as u32)),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

fn syntax(message: String) -> RecurrenceInputError {
    RecurrenceInputError::new(RecurrenceRejection::Syntax, message)
}

/// Build the stable diagnostic for one malformed cron field element.
fn invalid_cron_field(spec: &CronFieldSpec, element: &str) -> RecurrenceInputError {
    syntax(format!(
        "cron.expression {} field element {} must be *, a value, a-b, */n, a-b/n, or a comma-separated list of those.",
        spec.name,
        json_quote(element)
    ))
}

/// Read one in-range cron field value.
fn cron_field_value(text: &str, spec: &CronFieldSpec) -> Result<i64, RecurrenceInputError> {
    let value = digits_value(text).unwrap_or(i128::MAX);
    if value > MAX_SAFE_INTEGER || value < i128::from(spec.min) || value > i128::from(spec.max) {
        return Err(syntax(format!(
            "cron.expression {} field value {} is outside {}-{}.",
            spec.name, text, spec.min, spec.max
        )));
    }
    Ok(value as i64)
}

/// Read one positive cron field step.
fn cron_field_step(text: &str, spec: &CronFieldSpec) -> Result<i64, RecurrenceInputError> {
    match digits_value(text) {
        Some(step) if (1..=MAX_SAFE_INTEGER).contains(&step) => Ok(step as i64),
        _ => Err(syntax(format!(
            "cron.expression {} field step must be a positive integer.",
            spec.name
        ))),
    }
}

/// Add `start`, `start + step`, ... through `last`.
fn add_walk(matched: &mut Vec<i64>, start: i64, last: i64, step: i64) {
    let mut value = start;
    while value <= last {
        matched.push(value);
        value += step;
    }
}

/// Expand one comma-separated cron field into its matched value set.
fn cron_field_values(raw: &str, spec: &CronFieldSpec) -> Result<Vec<i64>, RecurrenceInputError> {
    let mut matched: Vec<i64> = Vec::new();
    for element in raw.split(',') {
        if element.is_empty() {
            return Err(syntax(format!(
                "cron.expression {} field must not contain an empty list element.",
                spec.name
            )));
        }
        if element == "*" {
            add_walk(&mut matched, spec.min, spec.max, 1);
            continue;
        }
        if element.starts_with('*') {
            let step_text = element
                .strip_prefix("*/")
                .filter(|text| digits_value(text).is_some())
                .ok_or_else(|| invalid_cron_field(spec, element))?;
            let step = cron_field_step(step_text, spec)?;
            add_walk(&mut matched, spec.min, spec.max, step);
            continue;
        }
        let (main, step_text) = match element.split_once('/') {
            Some((main, step)) => (main, Some(step)),
            None => (element, None),
        };
        let (start_text, end_text) = match main.split_once('-') {
            Some((start, end)) => (start, Some(end)),
            None => (main, None),
        };
        let shaped = digits_value(start_text).is_some()
            && end_text.is_none_or(|text| digits_value(text).is_some())
            && step_text.is_none_or(|text| digits_value(text).is_some());
        if !shaped {
            return Err(invalid_cron_field(spec, element));
        }
        let start = cron_field_value(start_text, spec)?;
        let Some(end_text) = end_text else {
            if step_text.is_some() {
                return Err(invalid_cron_field(spec, element));
            }
            matched.push(start);
            continue;
        };
        let last = cron_field_value(end_text, spec)?;
        if start > last {
            return Err(syntax(format!(
                "cron.expression {} field range {}-{} is inverted.",
                spec.name, start, last
            )));
        }
        let step = match step_text {
            Some(text) => cron_field_step(text, spec)?,
            None => 1,
        };
        add_walk(&mut matched, start, last, step);
    }
    let folded: BTreeSet<i64> = matched
        .into_iter()
        .map(|value| {
            if spec.canonical_max != spec.max && value == spec.max {
                spec.min
            } else {
                value
            }
        })
        .collect();
    Ok(folded.into_iter().collect())
}

/// Encode one matched value set as the shortest equivalent comma-separated cron field, never led by `*`.
fn encode_cron_field(values: &[i64]) -> String {
    let first = values[0];
    if values.len() == 1 {
        return first.to_string();
    }
    let last = values[values.len() - 1];
    let step = values[1] - first;
    let uniform = values
        .windows(2)
        .skip(1)
        .all(|pair| pair[1] - pair[0] == step);
    if uniform {
        return if step == 1 {
            format!("{first}-{last}")
        } else {
            format!("{first}-{last}/{step}")
        };
    }
    let run = |from: i64, to: i64| {
        if from == to {
            from.to_string()
        } else {
            format!("{from}-{to}")
        }
    };
    let mut parts: Vec<String> = Vec::new();
    let mut run_start = first;
    for index in 1..values.len() {
        let (current, previous) = (values[index], values[index - 1]);
        if current == previous + 1 {
            continue;
        }
        parts.push(run(run_start, previous));
        run_start = current;
    }
    parts.push(run(run_start, last));
    parts.join(",")
}

/// One uniform walk from a field's minimum, folded exactly as a parsed field folds.
fn star_walk(spec: &CronFieldSpec, step: i64) -> Vec<i64> {
    let mut walked = BTreeSet::new();
    let mut value = spec.min;
    while value <= spec.max {
        walked.insert(if spec.canonical_max != spec.max && value == spec.max {
            spec.min
        } else {
            value
        });
        value += step;
    }
    walked.into_iter().collect()
}

/// Canonical spelling of a field whose text started with `*`: the bare star for every value, otherwise the widest
/// star-step walk plus any remaining values as a list.
fn encode_star_cron_field(
    values: &[i64],
    spec: &CronFieldSpec,
) -> Result<String, RecurrenceInputError> {
    if values.len() as i64 == spec.canonical_max - spec.min + 1 {
        return Ok("*".to_owned());
    }
    let present: BTreeSet<i64> = values.iter().copied().collect();
    let mut best_step: Option<i64> = None;
    let mut best_walk: Vec<i64> = Vec::new();
    for step in 1..=spec.max - spec.min + 1 {
        let walk = star_walk(spec, step);
        if walk.len() > best_walk.len() && walk.iter().all(|value| present.contains(value)) {
            best_step = Some(step);
            best_walk = walk;
        }
    }
    let Some(best_step) = best_step else {
        return Err(syntax(format!("{} cannot keep a leading `*`.", spec.name)));
    };
    let walked: BTreeSet<i64> = best_walk.into_iter().collect();
    let remaining: Vec<i64> = values
        .iter()
        .copied()
        .filter(|value| !walked.contains(value))
        .collect();
    let walk_text = format!("*/{best_step}");
    Ok(if remaining.is_empty() {
        walk_text
    } else {
        format!("{walk_text},{}", encode_cron_field(&remaining))
    })
}

/// Parse one cron field into its matched values, canonical spelling, and star flag.
fn parse_cron_field(
    raw: &str,
    spec: &CronFieldSpec,
) -> Result<ParsedCronField, RecurrenceInputError> {
    let values = cron_field_values(raw, spec)?;
    let star = raw.starts_with('*');
    let canonical = if star {
        encode_star_cron_field(&values, spec)?
    } else {
        encode_cron_field(&values)
    };
    Ok(ParsedCronField {
        values,
        canonical,
        star,
    })
}

/// Parsed five-field cron rule: canonical text plus every field's matched values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronFields {
    /// Canonical expression stored in the durable record.
    pub expression: String,
    /// Unique ascending matched minutes.
    pub minutes: Vec<u32>,
    /// Unique ascending matched hours.
    pub hours: Vec<u32>,
    /// Unique ascending matched days of the month.
    pub days_of_month: Vec<u32>,
    /// Unique ascending matched months.
    pub months: Vec<u32>,
    /// Unique ascending matched cron weekdays, Sunday 0 through Saturday 6.
    pub days_of_week: Vec<u32>,
    /// Whether the day-of-month field text starts with `*` (Vixie `DOM_STAR`).
    pub day_of_month_star: bool,
    /// Whether the day-of-week field text starts with `*` (Vixie `DOW_STAR`).
    pub day_of_week_star: bool,
}

fn narrow(values: &[i64]) -> Vec<u32> {
    values.iter().map(|value| *value as u32).collect()
}

/// Parse and canonicalize one strict five-field cron expression.
fn parse_cron_expression(expression: &str) -> Result<CronFields, RecurrenceInputError> {
    if expression.is_empty() || js_trim(expression) != expression {
        return Err(syntax(
            "cron.expression must be a non-empty trimmed string.".to_owned(),
        ));
    }
    let fields: Vec<&str> = expression
        .split(is_js_whitespace)
        .filter(|field| !field.is_empty())
        .collect();
    if fields.len() != 5 {
        return Err(syntax(
            "cron.expression must contain exactly five whitespace-separated fields: minute hour day-of-month month day-of-week."
                .to_owned(),
        ));
    }
    let mut parsed = Vec::with_capacity(5);
    for (field, spec) in fields.iter().zip(CRON_FIELDS.iter()) {
        parsed.push(parse_cron_field(field, spec)?);
    }
    let canonical: Vec<&str> = parsed
        .iter()
        .map(|field| field.canonical.as_str())
        .collect();
    Ok(CronFields {
        expression: canonical.join(" "),
        minutes: narrow(&parsed[0].values),
        hours: narrow(&parsed[1].values),
        days_of_month: narrow(&parsed[2].values),
        months: narrow(&parsed[3].values),
        days_of_week: narrow(&parsed[4].values),
        day_of_month_star: parsed[2].star,
        day_of_week_star: parsed[4].star,
    })
}

/// The local-date predicate of one parsed cron rule under Vixie day-of-month/day-of-week semantics: when either day
/// field text starts with `*`, a date must satisfy both fields; otherwise either field suffices.
struct DateMatch {
    days_of_month: BTreeSet<u32>,
    days_of_week: BTreeSet<u32>,
    day_of_month_restricted: bool,
    day_of_week_restricted: bool,
}

impl DateMatch {
    fn new(parsed: &CronFields) -> DateMatch {
        DateMatch {
            days_of_month: parsed.days_of_month.iter().copied().collect(),
            days_of_week: parsed.days_of_week.iter().copied().collect(),
            day_of_month_restricted: !parsed.day_of_month_star,
            day_of_week_restricted: !parsed.day_of_week_star,
        }
    }

    /// Whether a local date, given as a day count and its day of the month, matches the rule's day fields.
    fn matches(&self, days: i64, day_of_month: i64) -> bool {
        let dom = self.days_of_month.contains(&(day_of_month as u32));
        let dow = self.days_of_week.contains(&(weekday_sunday0(days) as u32));
        if self.day_of_month_restricted && self.day_of_week_restricted {
            dom || dow
        } else {
            dom && dow
        }
    }
}

/// Local minutes of day in matching order.
fn cron_times(parsed: &CronFields, reverse: bool) -> Vec<i64> {
    let mut times = Vec::with_capacity(parsed.hours.len() * parsed.minutes.len());
    for hour in &parsed.hours {
        for minute in &parsed.minutes {
            times.push(i64::from(hour * 60 + minute));
        }
    }
    if reverse {
        times.reverse();
    }
    times
}

/// Resolve one local minute of a date; a date `jiff` cannot hold lies beyond the four-digit-year ceiling.
fn resolve_minute(zone: &Zone, days: i64, minute: i64) -> Option<Option<i64>> {
    let (year, month, day) = civil_from_days(days);
    zone.resolve((year, month, day, minute / 60, minute % 60, 0, 0))
        .ok()
}

/// Whole days since the epoch of the first of next month.
fn first_of_next_month(days: i64) -> i64 {
    let (year, month, _) = civil_from_days(days);
    days_from_civil(year, month + 1, 1)
}

/// Find the first strictly future cron occurrence without leaving the four-digit UTC year range.
///
/// The walk stops at [`CRON_SEARCH_HORIZON_YEARS`] past the floor date when that horizon precedes the four-digit
/// ceiling, so a rule with no match inside the horizon resolves to exhaustion.
fn next_cron_target(parsed: &CronFields, zone: &Zone, floor: i64) -> Option<String> {
    let matches = DateMatch::new(parsed);
    let times = cron_times(parsed, false);
    let local_floor = floor + zone.offset_ms_at(floor);
    let first_date = local_floor.div_euclid(MS_PER_DAY);
    let floor_minute = local_floor.rem_euclid(MS_PER_DAY) / 60_000;
    let ceiling = days_from_civil(10_000, 1, 1);
    let (first_year, first_month, first_day) = civil_from_days(first_date);
    let horizon = days_from_civil(
        first_year + CRON_SEARCH_HORIZON_YEARS,
        first_month,
        first_day,
    );
    let last_date = horizon.min(ceiling);
    let mut date = first_date;
    while date <= last_date {
        let (_, month, day) = civil_from_days(date);
        if !parsed.months.contains(&(month as u32)) {
            date = first_of_next_month(date);
            continue;
        }
        if matches.matches(date, day) {
            for &minute in &times {
                if date == first_date && minute < floor_minute {
                    continue;
                }
                // A date beyond what the zone data holds is past the ceiling.
                let target = resolve_minute(zone, date, minute)?;
                let Some(target) = target else { continue };
                if target > MAX_FOUR_DIGIT_YEAR_MS {
                    return None;
                }
                if target >= MIN_FOUR_DIGIT_YEAR_MS && target > floor {
                    return Some(iso_string(target));
                }
            }
        }
        date += 1;
    }
    None
}

/// Find the latest matching cron occurrence at or before a decision without leaving the committed floor.
///
/// The walk stops at [`CRON_SEARCH_HORIZON_YEARS`] before the decision date when that horizon follows the
/// four-digit floor: a rule with no match inside the horizon keeps the committed target.
fn latest_cron_occurrence(
    parsed: &CronFields,
    zone: &Zone,
    decision: i64,
    saved_target: i64,
) -> i64 {
    let matches = DateMatch::new(parsed);
    let forward = cron_times(parsed, false);
    let backward = cron_times(parsed, true);
    // Offsets are less than 24 hours; a date-line rollback can make a due local date later than the UTC decision
    // date, which is why the walk starts one day after it.
    let mut date = decision.div_euclid(MS_PER_DAY) + 1;
    // The floor is an instant: the earliest local date the walk may reach is the one holding it in the rule's own
    // zone, and candidates below it are skipped.
    let floor_date =
        (MIN_FOUR_DIGIT_YEAR_MS + zone.offset_ms_at(MIN_FOUR_DIGIT_YEAR_MS)).div_euclid(MS_PER_DAY);
    let (year, month, day) = civil_from_days(date);
    let horizon = days_from_civil(year - CRON_SEARCH_HORIZON_YEARS, month, day);
    let first_date = horizon.max(floor_date);
    while date >= first_date {
        let (year, month, day) = civil_from_days(date);
        if !parsed.months.contains(&(month as u32)) {
            date = days_from_civil(year, month, 1) - 1;
            continue;
        }
        if matches.matches(date, day) {
            // One conversion of the date's earliest wall clock decides whether the whole date lies beyond the
            // decision. An earliest time that a DST gap removes leaves the bound to the first time that does exist.
            let mut earliest: Option<i64> = None;
            for &minute in &forward {
                earliest = match resolve_minute(zone, date, minute) {
                    Some(resolved) => resolved,
                    None => Some(i64::MAX),
                };
                if earliest.is_some() {
                    break;
                }
            }
            if earliest.is_some_and(|earliest| earliest > decision) {
                date -= 1;
                continue;
            }
            for &minute in &backward {
                let Some(Some(candidate)) = resolve_minute(zone, date, minute) else {
                    continue;
                };
                if candidate > decision || candidate < MIN_FOUR_DIGIT_YEAR_MS {
                    continue;
                }
                // A committed target remains due even if current tzdata places the rule before it.
                return candidate.max(saved_target);
            }
        }
        date -= 1;
    }
    saved_target
}

/// Parsed Cron value; expression and zone stay authoritative independently of any task.
#[derive(Debug, Clone)]
pub struct Cron {
    parsed: CronFields,
    time_zone: String,
    zone: Zone,
}

impl Cron {
    /// Immutable matched fields and canonical text for consumers such as rule descriptions.
    pub fn fields(&self) -> &CronFields {
        &self.parsed
    }

    /// Canonical five-field expression, preserving Vixie star semantics.
    pub fn expression(&self) -> &str {
        &self.parsed.expression
    }

    /// Canonical zone of the rule.
    pub fn time_zone(&self) -> &str {
        &self.time_zone
    }

    /// Parse one five-field expression and its explicit IANA zone.
    pub fn parse(expression: &str, time_zone: &str) -> RecurrenceResult<Cron> {
        parse_recurrence(|| {
            let parsed = parse_cron_expression(expression)?;
            let (time_zone, zone) = canonicalize_zone(time_zone).map_err(|(reason, _)| {
                RecurrenceInputError::with_cause(
                    RecurrenceRejection::TimeZone,
                    "Cron requires a valid UTC or IANA Area/Location zone.",
                    reason.as_str().to_owned(),
                )
            })?;
            Ok(Cron {
                parsed,
                time_zone,
                zone,
            })
        })
    }

    /// First strictly future occurrence, skipping gaps and choosing the earlier overlap; `None` at exhaustion.
    ///
    /// # Errors
    /// [`TimeError::Range`] when `after` is not a four-digit-year instant.
    pub fn next(&self, after: i64) -> Result<Option<String>, TimeError> {
        if !is_four_digit_year_instant(after) {
            return Err(TimeError::Range(
                "Cron lower bound must be a four-digit-year instant".to_owned(),
            ));
        }
        Ok(next_cron_target(&self.parsed, &self.zone, after))
    }

    /// Latest due occurrence and first future occurrence, without replaying a backlog.
    ///
    /// # Errors
    /// [`TimeError::Range`] when either instant is invalid or the decision precedes the committed occurrence.
    pub fn resolve(
        &self,
        scheduled_at: &str,
        accepted_at: i64,
    ) -> Result<RecurrenceOccurrence, TimeError> {
        let saved_target = recurrence_decision(scheduled_at, accepted_at)?;
        let occurrence_at = iso_string(latest_cron_occurrence(
            &self.parsed,
            &self.zone,
            accepted_at,
            saved_target,
        ));
        let next_scheduled_at = self.next(accepted_at)?;
        Ok(RecurrenceOccurrence {
            occurrence_at,
            next_scheduled_at,
        })
    }
}
