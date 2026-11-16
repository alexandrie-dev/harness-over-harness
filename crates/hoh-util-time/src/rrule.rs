//! Bounded RFC 5545 recurrence over UTC or explicit local wall time.
//!
//! The source delegates occurrence enumeration to the `rrule` npm library, fed with UTC-shaped wall times. The
//! enumeration here is a port of that library's iteration (dateutil's algorithm) for the profile this module
//! admits, so the bounded search logic around it stays line for line the source's.

use crate::calendar::{
    MS_PER_DAY, civil_from_days, days_from_civil, days_in_month, epoch_ms, fields_of, is_leap,
    iso_string, js_trim, weekday_monday0,
};
use crate::error::TimeError;
use crate::instant::{MAX_FOUR_DIGIT_YEAR_MS, canonicalize_zone, is_four_digit_year_instant};
use crate::recurrence::{
    RecurrenceInputError, RecurrenceOccurrence, RecurrenceRejection, RecurrenceResult,
    parse_recurrence, recurrence_decision,
};
use crate::zone::Zone;

/// Largest accepted COUNT; a longer finite recurrence uses UNTIL.
const MAX_RRULE_COUNT: i128 = 1_000;

/// Largest number of occurrences that one FREQ period may generate: every minute of one week.
const MAX_PERIOD_CANDIDATES: i64 = 10_080;

/// Largest number of BYSETPOS values.
const MAX_SET_POSITIONS: usize = 31;

/// Earliest DTSTART and UNTIL year; the recurrence library reads years 0000 through 0099 as 1900 through 1999.
const MIN_RRULE_YEAR: i64 = 100;

/// Largest integer JavaScript represents exactly.
const MAX_SAFE_INTEGER: i128 = 9_007_199_254_740_991;

/// Stands in for JavaScript's `Infinity` in the cost bounds.
const INFINITY: i64 = i64::MAX;

/// Latest wall time that the recurrence library can generate or read as a DTSTART (9999-12-31T23:59:59).
const LAST_RRULE_WALL_MS: i64 = 253_402_300_799_000;

/// FREQ periods in one 400-year Gregorian cycle; after a cycle, weekdays and leap days repeat.
const CYCLE_PERIODS_MINUTELY: i128 = 210_379_680;
const CYCLE_PERIODS_HOURLY: i128 = 3_506_328;
const CYCLE_PERIODS_DAILY: i128 = 146_097;
const CYCLE_PERIODS_WEEKLY: i128 = 20_871;
const CYCLE_PERIODS_MONTHLY: i128 = 4_800;
const CYCLE_PERIODS_YEARLY: i128 = 400;

/// Length of one 400-year Gregorian cycle in milliseconds.
const CYCLE_MS: i128 = CYCLE_PERIODS_DAILY * MS_PER_DAY as i128;

/// RFC 5545 weekday codes; index 0 is Monday, as in the recurrence library.
const WEEKDAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

const SUPPORTED_PARTS: [&str; 14] = [
    "FREQ",
    "INTERVAL",
    "COUNT",
    "UNTIL",
    "WKST",
    "BYSECOND",
    "BYMINUTE",
    "BYHOUR",
    "BYDAY",
    "BYMONTHDAY",
    "BYYEARDAY",
    "BYWEEKNO",
    "BYMONTH",
    "BYSETPOS",
];

/// Inclusive range of one numeric BY list; a range with a negative minimum excludes 0.
const LIST_RANGES: [(&str, i64, i64); 8] = [
    ("BYSECOND", 0, 59),
    ("BYMINUTE", 0, 59),
    ("BYHOUR", 0, 23),
    ("BYMONTHDAY", -31, 31),
    ("BYYEARDAY", -366, 366),
    ("BYWEEKNO", -53, 53),
    ("BYMONTH", 1, 12),
    ("BYSETPOS", -366, 366),
];

/// BY lists whose values the recurrence library combines into the times of one day.
const TIME_PARTS: [&str; 3] = ["BYHOUR", "BYMINUTE", "BYSECOND"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frequency {
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Frequency {
    fn name(self) -> &'static str {
        match self {
            Frequency::Minutely => "MINUTELY",
            Frequency::Hourly => "HOURLY",
            Frequency::Daily => "DAILY",
            Frequency::Weekly => "WEEKLY",
            Frequency::Monthly => "MONTHLY",
            Frequency::Yearly => "YEARLY",
        }
    }

    fn cycle_periods(self) -> i128 {
        match self {
            Frequency::Minutely => CYCLE_PERIODS_MINUTELY,
            Frequency::Hourly => CYCLE_PERIODS_HOURLY,
            Frequency::Daily => CYCLE_PERIODS_DAILY,
            Frequency::Weekly => CYCLE_PERIODS_WEEKLY,
            Frequency::Monthly => CYCLE_PERIODS_MONTHLY,
            Frequency::Yearly => CYCLE_PERIODS_YEARLY,
        }
    }

    fn parse(text: &str) -> Option<Frequency> {
        [
            Frequency::Minutely,
            Frequency::Hourly,
            Frequency::Daily,
            Frequency::Weekly,
            Frequency::Monthly,
            Frequency::Yearly,
        ]
        .into_iter()
        .find(|frequency| frequency.name() == text)
    }
}

/// BYDAY entries: the count of entries and of ordinal ones (an ordinal entry such as `2TU` selects at most one day of
/// its month or year), plus the decoded selectors.
struct ByDay {
    entries: usize,
    ordinals: usize,
    /// `(ordinal, weekday index with Monday 0)`.
    items: Vec<(Option<i64>, i64)>,
}

/// Decoded RRULE values used by the combination and cost checks.
struct RuleValues {
    frequency: Frequency,
    interval: i64,
    count: Option<i64>,
    /// UNTIL instant in epoch milliseconds.
    until: Option<i64>,
    /// WKST index; 0 is Monday.
    week_start: i64,
    lists: Vec<(&'static str, Vec<i64>)>,
    by_day: Option<ByDay>,
}

impl RuleValues {
    fn list(&self, key: &str) -> Option<&Vec<i64>> {
        self.lists
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, values)| values)
    }
}

/// The enumeration input for one rule, in the vocabulary of the recurrence library.
#[derive(Debug, Clone)]
struct LibRule {
    frequency: Frequency,
    interval: i64,
    week_start: i64,
    by_month: Vec<i64>,
    by_month_day: Vec<i64>,
    by_negative_month_day: Vec<i64>,
    by_year_day: Vec<i64>,
    by_week_no: Vec<i64>,
    by_weekday: Vec<i64>,
    by_nth_weekday: Vec<(i64, i64)>,
    by_set_pos: Vec<i64>,
    /// Milliseconds of the day, ascending; used from DAILY upwards.
    timeset: Vec<i64>,
}

/// One validated rule and the recurrence-library input for its UTC-shaped local wall time.
#[derive(Debug, Clone)]
struct PreparedRRule {
    /// Trimmed two-line input; stored records keep this text.
    normalized: String,
    /// Canonical zone; `UTC` when DTSTART ends in Z.
    time_zone: String,
    zone: Zone,
    /// DTSTART wall time as UTC-shaped epoch milliseconds.
    start: i64,
    frequency: Frequency,
    interval: i64,
    /// WKST index; 0 is Monday.
    week_start: i64,
    count: Option<i64>,
    /// UNTIL instant, or the latest four-digit-year instant.
    limit: i64,
    lib: LibRule,
    /// Wall-time length of one complete calendar and INTERVAL cycle; a longer run of gap wall times never ends.
    invalid_run_limit: i128,
}

/// Latest occurrence at or before a decision time and the first occurrence after it.
#[derive(Debug, Default, Clone, Copy)]
struct OccurrenceWindow {
    latest: Option<i64>,
    next: Option<i64>,
}

fn syntax(message: impl Into<String>) -> RecurrenceInputError {
    RecurrenceInputError::new(RecurrenceRejection::Syntax, message)
}

fn density(message: impl Into<String>) -> RecurrenceInputError {
    RecurrenceInputError::new(RecurrenceRejection::Density, message)
}

fn is_digits(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit)
}

/// Whether text is `YYYYMMDDTHHMMSS`, followed by `Z` when `with_z`.
fn is_basic(text: &str, with_z: bool) -> bool {
    let b = text.as_bytes();
    b.len() == if with_z { 16 } else { 15 }
        && is_digits(&b[0..8])
        && b[8] == b'T'
        && is_digits(&b[9..15])
        && (!with_z || b[15] == b'Z')
}

/// Read a matched basic date-time `YYYYMMDDTHHMMSS` as UTC-shaped epoch milliseconds, rejecting normalization.
fn basic_time(field: &str, text: &str) -> Result<i64, RecurrenceInputError> {
    let part = |from: usize, to: usize| text[from..to].parse::<i64>().unwrap_or(0);
    let (year, month, day) = (part(0, 4), part(4, 6), part(6, 8));
    let (hour, minute, second) = (part(9, 11), part(11, 13), part(13, 15));
    let message =
        format!("{field} must be a real calendar date and time in years 0100 through 9999.");
    if year < MIN_RRULE_YEAR {
        return Err(syntax(message));
    }
    let real = (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
        && hour <= 23
        && minute <= 59
        && second <= 59;
    if !real {
        return Err(RecurrenceInputError::with_cause(
            RecurrenceRejection::Syntax,
            message,
            "RangeError: date-time field out of range".to_owned(),
        ));
    }
    Ok(epoch_ms(year, month, day, hour, minute, second, 0))
}

/// Split the input into its trimmed DTSTART and RRULE lines.
fn recurrence_lines(value: &str) -> Result<(String, String), RecurrenceInputError> {
    let normalized = value.replace("\r\n", "\n");
    let lines: Vec<&str> = js_trim(&normalized)
        .split('\n')
        .map(js_trim)
        .filter(|line| !line.is_empty())
        .collect();
    match lines.as_slice() {
        [start, rule] if start.starts_with("DTSTART") && rule.starts_with("RRULE:") => {
            Ok(((*start).to_owned(), (*rule).to_owned()))
        }
        _ => Err(syntax(
            "rrule must contain one DTSTART line followed by one RRULE line.",
        )),
    }
}

/// Require a canonical zone while retaining the parser's validation result.
fn validated_zone(value: &str) -> Result<(String, Zone), RecurrenceInputError> {
    canonicalize_zone(value).map_err(|(reason, cause)| {
        RecurrenceInputError::with_cause(
            RecurrenceRejection::TimeZone,
            "TZID must be UTC or a valid IANA Area/Location name.",
            cause.unwrap_or_else(|| reason.as_str().to_owned()),
        )
    })
}

/// Read DTSTART as its canonical zone and UTC-shaped wall time.
fn parse_start(line: &str) -> Result<(String, Zone, i64), RecurrenceInputError> {
    if let Some(text) = line
        .strip_prefix("DTSTART:")
        .filter(|text| is_basic(text, true))
    {
        let (time_zone, zone) = validated_zone("UTC")?;
        return Ok((time_zone, zone, basic_time("DTSTART", &text[..15])?));
    }
    let zoned = line.strip_prefix("DTSTART;TZID=").and_then(|rest| {
        let (zone, tail) = rest.split_once(':')?;
        (!zone.is_empty() && !zone.contains(';') && is_basic(tail, false)).then_some((zone, tail))
    });
    let Some((zone_text, tail)) = zoned else {
        return Err(syntax(
            "DTSTART must use UTC with Z or a local date-time with an explicit TZID.",
        ));
    };
    let (time_zone, zone) = validated_zone(zone_text)?;
    Ok((time_zone, zone, basic_time("DTSTART", tail)?))
}

/// Split the RRULE line into its parts in input order.
fn rule_parts(line: &str) -> Result<Vec<(String, String)>, RecurrenceInputError> {
    let mut parts: Vec<(String, String)> = Vec::new();
    for component in line["RRULE:".len()..].split(';') {
        let Some((key, text)) = component.split_once('=') else {
            return Err(syntax("RRULE fields must be non-empty and unique."));
        };
        if key.is_empty() || text.is_empty() || parts.iter().any(|(existing, _)| existing == key) {
            return Err(syntax("RRULE fields must be non-empty and unique."));
        }
        if !SUPPORTED_PARTS.contains(&key) {
            return Err(syntax(format!(
                "RRULE supports only {}.",
                SUPPORTED_PARTS.join(", ")
            )));
        }
        parts.push((key.to_owned(), text.to_owned()));
    }
    Ok(parts)
}

fn part<'a>(parts: &'a [(String, String)], key: &str) -> Option<&'a str> {
    parts
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, text)| text.as_str())
}

/// Digits of a matched unsigned integer text as a saturating number.
fn unsigned_value(text: &str) -> Option<i128> {
    crate::calendar::digits_value(text)
}

/// Read FREQ; SECONDLY is valid RFC 5545 but violates the 60-second occurrence spacing.
fn frequency_of(text: Option<&str>) -> Result<Frequency, RecurrenceInputError> {
    let Some(text) = text else {
        return Err(syntax("RRULE must contain FREQ."));
    };
    if text == "SECONDLY" {
        return Err(density(
            "FREQ=SECONDLY is not supported; occurrences must be at least 60 seconds apart.",
        ));
    }
    Frequency::parse(text)
        .ok_or_else(|| syntax("FREQ must be MINUTELY, HOURLY, DAILY, WEEKLY, MONTHLY, or YEARLY."))
}

/// Read an optional unsigned part from 1 through max.
fn positive_value(
    text: Option<&str>,
    max: i128,
    message: &str,
) -> Result<Option<i64>, RecurrenceInputError> {
    let Some(text) = text else { return Ok(None) };
    match unsigned_value(text) {
        Some(value) if value >= 1 && value <= max => Ok(Some(value as i64)),
        _ => Err(syntax(message)),
    }
}

/// Read an optional UNTIL as an actual UTC instant.
fn until_of(text: Option<&str>) -> Result<Option<i64>, RecurrenceInputError> {
    let Some(text) = text else { return Ok(None) };
    if !is_basic(text, true) {
        return Err(syntax(
            "UNTIL must be an absolute UTC date-time ending in Z.",
        ));
    }
    Ok(Some(basic_time("UNTIL", &text[..15])?))
}

/// Read WKST as a weekday index; RFC 5545 defaults it to Monday.
fn week_start_of(text: Option<&str>) -> Result<i64, RecurrenceInputError> {
    WEEKDAYS
        .iter()
        .position(|code| *code == text.unwrap_or("MO"))
        .map(|index| index as i64)
        .ok_or_else(|| syntax("WKST must be MO, TU, WE, TH, FR, SA, or SU."))
}

/// Read one numeric BY list whose values are unique and in range.
fn list_values(
    key: &str,
    text: &str,
    min: i64,
    max: i64,
) -> Result<Vec<i64>, RecurrenceInputError> {
    let signed = min < 0;
    let mut values = Vec::new();
    for item in text.split(',') {
        let (negative, digits) = match item.as_bytes().first() {
            Some(b'+') if signed => (false, &item[1..]),
            Some(b'-') if signed => (true, &item[1..]),
            _ => (false, item),
        };
        let magnitude = unsigned_value(digits);
        let value = magnitude.map(|magnitude| if negative { -magnitude } else { magnitude });
        match value {
            Some(value)
                if value >= i128::from(min)
                    && value <= i128::from(max)
                    && !(signed && value == 0) =>
            {
                values.push(value as i64);
            }
            _ => {
                return Err(syntax(format!(
                    "{key} values must be integers from {min} to {max}{}.",
                    if signed { ", excluding 0" } else { "" }
                )));
            }
        }
    }
    let mut unique = values.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != values.len() {
        return Err(syntax(format!("{key} values must be unique.")));
    }
    Ok(values)
}

/// Whether a BYDAY selector's ordinal text is `[+-]?(1-53)` without leading zeros.
fn is_ordinal(text: &str) -> bool {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text).as_bytes();
    match digits {
        [first] => (b'1'..=b'9').contains(first),
        [first, second] => {
            second.is_ascii_digit()
                && ((b'1'..=b'4').contains(first) || (*first == b'5' && *second <= b'3'))
        }
        _ => false,
    }
}

/// Read an optional BYDAY list; `+2TU` and `2TU` select the same day.
fn by_day_of(text: Option<&str>) -> Result<Option<ByDay>, RecurrenceInputError> {
    let Some(text) = text else { return Ok(None) };
    let selectors: Vec<&str> = text.split(',').collect();
    let mut seen: Vec<(i64, usize)> = Vec::new();
    let mut items = Vec::new();
    let mut ordinals = 0;
    for selector in &selectors {
        let weekday = selector
            .len()
            .checked_sub(2)
            .filter(|at| selector.is_char_boundary(*at))
            .and_then(|at| {
                WEEKDAYS
                    .iter()
                    .position(|code| *code == &selector[at..])
                    .map(|day| (at, day))
            });
        let Some((at, weekday)) = weekday else {
            return Err(syntax("BYDAY contains an invalid weekday selector."));
        };
        let ordinal = &selector[..at];
        if !ordinal.is_empty() && !is_ordinal(ordinal) {
            return Err(syntax("BYDAY contains an invalid weekday selector."));
        }
        let number = if ordinal.is_empty() {
            0
        } else {
            ordinal.trim_start_matches('+').parse::<i64>().unwrap_or(0)
        };
        if !seen.contains(&(number, weekday)) {
            seen.push((number, weekday));
        }
        if !ordinal.is_empty() {
            ordinals += 1;
        }
        items.push((
            if ordinal.is_empty() {
                None
            } else {
                Some(number)
            },
            weekday as i64,
        ));
    }
    if seen.len() != selectors.len() {
        return Err(syntax("BYDAY values must be unique."));
    }
    Ok(Some(ByDay {
        entries: selectors.len(),
        ordinals,
        items,
    }))
}

/// Decode every RRULE part value.
fn rule_values(parts: &[(String, String)]) -> Result<RuleValues, RecurrenceInputError> {
    let frequency = frequency_of(part(parts, "FREQ"))?;
    let interval = positive_value(
        part(parts, "INTERVAL"),
        MAX_SAFE_INTEGER,
        "INTERVAL must be a positive safe integer.",
    )?;
    let count = positive_value(
        part(parts, "COUNT"),
        MAX_RRULE_COUNT,
        &format!(
            "COUNT must be an integer from 1 to {MAX_RRULE_COUNT}; use UNTIL for a longer finite recurrence."
        ),
    )?;
    let until = until_of(part(parts, "UNTIL"))?;
    let week_start = week_start_of(part(parts, "WKST"))?;
    let mut lists = Vec::new();
    for (key, min, max) in LIST_RANGES {
        if let Some(text) = part(parts, key) {
            lists.push((key, list_values(key, text, min, max)?));
        }
    }
    Ok(RuleValues {
        frequency,
        interval: interval.unwrap_or(1),
        count,
        until,
        week_start,
        lists,
        by_day: by_day_of(part(parts, "BYDAY"))?,
    })
}

/// Reject a part combination that RFC 5545 forbids or that breaks the 60-second occurrence spacing.
fn check_combination(values: &RuleValues) -> Result<(), RecurrenceInputError> {
    let frequency = values.frequency;
    let ordinals = values.by_day.as_ref().map_or(0, |by_day| by_day.ordinals);
    if values.count.is_some() && values.until.is_some() {
        return Err(syntax("RRULE must not contain both COUNT and UNTIL."));
    }
    if values.list("BYSECOND").map_or(1, Vec::len) > 1 {
        return Err(density(
            "BYSECOND must contain one value; occurrences must be at least 60 seconds apart.",
        ));
    }
    if matches!(frequency, Frequency::Minutely | Frequency::Hourly)
        && (!values.lists.is_empty() || values.by_day.is_some())
    {
        return Err(syntax(format!(
            "FREQ={} does not support BY rule parts; use FREQ=DAILY or FREQ=WEEKLY with BYHOUR and BYMINUTE.",
            frequency.name()
        )));
    }
    for key in ["BYWEEKNO", "BYYEARDAY"] {
        if values.list(key).is_some() && frequency != Frequency::Yearly {
            return Err(syntax(format!("{key} requires FREQ=YEARLY.")));
        }
    }
    if values.list("BYMONTHDAY").is_some()
        && !matches!(frequency, Frequency::Monthly | Frequency::Yearly)
    {
        return Err(syntax("BYMONTHDAY requires FREQ=MONTHLY or FREQ=YEARLY."));
    }
    if ordinals > 0 && !matches!(frequency, Frequency::Monthly | Frequency::Yearly) {
        return Err(syntax(
            "BYDAY ordinals require FREQ=MONTHLY or FREQ=YEARLY.",
        ));
    }
    if ordinals > 0 && values.list("BYWEEKNO").is_some() {
        return Err(syntax("BYDAY ordinals cannot be combined with BYWEEKNO."));
    }
    if values.list("BYSETPOS").is_some() && values.lists.len() == 1 && values.by_day.is_none() {
        return Err(syntax("BYSETPOS requires another BY rule part."));
    }
    Ok(())
}

/// Return an upper bound on BYDAY days in one month or year, or infinity without BYDAY.
fn by_day_days(by_day: &Option<ByDay>, per_weekday: i64) -> i64 {
    by_day.as_ref().map_or(INFINITY, |by_day| {
        by_day.ordinals as i64 + per_weekday * (by_day.entries - by_day.ordinals) as i64
    })
}

/// Return an upper bound on the days that one FREQ period selects; the rule carries its DTSTART defaults explicitly.
fn period_days(values: &RuleValues) -> i64 {
    let size = |key: &str| values.list(key).map_or(INFINITY, |list| list.len() as i64);
    let months = values.list("BYMONTH").map(|list| list.len() as i64);
    let by_day = &values.by_day;
    match values.frequency {
        Frequency::Minutely | Frequency::Hourly | Frequency::Daily => 1,
        Frequency::Weekly => 7.min(by_day_days(by_day, 1)),
        Frequency::Monthly => 31.min(size("BYMONTHDAY")).min(by_day_days(by_day, 5)),
        Frequency::Yearly => match months {
            None => 366
                .min(size("BYYEARDAY"))
                .min(12_i64.saturating_mul(size("BYMONTHDAY")))
                .min(9_i64.saturating_mul(size("BYWEEKNO")))
                .min(by_day_days(by_day, 53)),
            Some(months) => (31 * months)
                .min(size("BYYEARDAY"))
                .min(months.saturating_mul(size("BYMONTHDAY")))
                .min(9_i64.saturating_mul(size("BYWEEKNO")))
                .min(months.saturating_mul(by_day_days(by_day, 5))),
        },
    }
}

/// Reject a rule whose FREQ period can generate more occurrences than one enumeration may visit.
fn check_cost(
    values: &RuleValues,
    reject_ineffective_set_positions: bool,
) -> Result<(), RecurrenceInputError> {
    let times: i64 = TIME_PARTS
        .iter()
        .map(|key| values.list(key).map_or(1, |list| list.len() as i64))
        .product();
    let candidates = period_days(values) * times;
    if candidates > MAX_PERIOD_CANDIDATES {
        return Err(density(format!(
            "One FREQ period of this RRULE can contain up to {candidates} occurrences; the limit is {MAX_PERIOD_CANDIDATES}."
        )));
    }
    let positions: &[i64] = values.list("BYSETPOS").map_or(&[], Vec::as_slice);
    if reject_ineffective_set_positions && candidates == 1 && !positions.is_empty() {
        return Err(syntax(format!(
            "BYSETPOS has no effect because one FREQ={} period of this RRULE contains at most 1 occurrence. \
             BYSETPOS selects among the occurrences of one period; for example, \
             FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1 selects the last weekday of each month.",
            values.frequency.name()
        )));
    }
    if positions.len() > MAX_SET_POSITIONS {
        return Err(syntax(format!(
            "BYSETPOS must contain at most {MAX_SET_POSITIONS} values."
        )));
    }
    if positions.iter().any(|position| position.abs() > candidates) {
        return Err(syntax(format!(
            "BYSETPOS values must be from 1 to {candidates} or from -{candidates} to -1; \
             one FREQ period of this RRULE contains at most {candidates} occurrences."
        )));
    }
    Ok(())
}

/// Return the parts that the recurrence library derives from DTSTART when a DAILY or longer rule omits them.
fn start_defaults(values: &RuleValues, start: i64) -> Vec<(String, String)> {
    let mut defaults: Vec<(String, String)> = Vec::new();
    let frequency = values.frequency;
    if matches!(frequency, Frequency::Minutely | Frequency::Hourly) {
        return defaults;
    }
    let (_, month, day, hour, minute, second, _) = fields_of(start);
    let has = |key: &str| values.list(key).is_some();
    if !has("BYHOUR") {
        defaults.push(("BYHOUR".to_owned(), hour.to_string()));
    }
    if !has("BYMINUTE") {
        defaults.push(("BYMINUTE".to_owned(), minute.to_string()));
    }
    if !has("BYSECOND") {
        defaults.push(("BYSECOND".to_owned(), second.to_string()));
    }
    if values.by_day.is_some() || has("BYMONTHDAY") || has("BYYEARDAY") || has("BYWEEKNO") {
        return defaults;
    }
    if frequency == Frequency::Weekly {
        let weekday = weekday_monday0(start.div_euclid(MS_PER_DAY));
        defaults.push(("BYDAY".to_owned(), WEEKDAYS[weekday as usize].to_owned()));
    }
    if matches!(frequency, Frequency::Monthly | Frequency::Yearly) {
        defaults.push(("BYMONTHDAY".to_owned(), day.to_string()));
    }
    if frequency == Frequency::Yearly && !has("BYMONTH") {
        defaults.push(("BYMONTH".to_owned(), month.to_string()));
    }
    defaults
}

/// Greatest common divisor of two positive integers.
fn gcd(left: i128, right: i128) -> i128 {
    if right == 0 {
        left
    } else {
        gcd(right, left % right)
    }
}

/// Resolve a UTC-shaped wall time to its earlier actual instant, or `None` inside a forward transition.
fn instant_of(zone: &Zone, wall: i64) -> Option<i64> {
    zone.resolve_wall(wall).ok().flatten()
}

/// Build the enumeration input from fully defaulted rule values.
fn lib_rule(values: &RuleValues) -> LibRule {
    let list = |key: &str| values.list(key).cloned().unwrap_or_default();
    let month_days = list("BYMONTHDAY");
    let mut by_weekday = Vec::new();
    let mut by_nth_weekday = Vec::new();
    if let Some(by_day) = &values.by_day {
        for (ordinal, weekday) in &by_day.items {
            match ordinal {
                Some(n) if matches!(values.frequency, Frequency::Yearly | Frequency::Monthly) => {
                    by_nth_weekday.push((*weekday, *n));
                }
                _ => by_weekday.push(*weekday),
            }
        }
    }
    // The library combines the time lists in their given order, so ascending lists enumerate each day in time order.
    let sorted = |key: &str| {
        let mut values = list(key);
        values.sort_unstable();
        values
    };
    let mut timeset = Vec::new();
    for hour in sorted("BYHOUR") {
        for minute in sorted("BYMINUTE") {
            for second in sorted("BYSECOND") {
                timeset.push(((hour * 60 + minute) * 60 + second) * 1_000);
            }
        }
    }
    LibRule {
        frequency: values.frequency,
        interval: values.interval,
        week_start: values.week_start,
        by_month: list("BYMONTH"),
        by_month_day: month_days.iter().copied().filter(|day| *day > 0).collect(),
        by_negative_month_day: month_days.iter().copied().filter(|day| *day < 0).collect(),
        by_year_day: list("BYYEARDAY"),
        by_week_no: list("BYWEEKNO"),
        by_weekday,
        by_nth_weekday,
        by_set_pos: list("BYSETPOS"),
        timeset,
    }
}

/// Validate a rule completely, then derive its enumeration inputs.
fn prepare_rrule(
    value: &str,
    reject_ineffective_set_positions: bool,
) -> Result<PreparedRRule, RecurrenceInputError> {
    let (start_line, rule_line) = recurrence_lines(value)?;
    let (time_zone, zone, start) = parse_start(&start_line)?;
    if instant_of(&zone, start).is_none() {
        return Err(syntax(
            "DTSTART must be a real local date and time in its time zone.",
        ));
    }
    let parts = rule_parts(&rule_line)?;
    let values = rule_values(&parts)?;
    check_combination(&values)?;
    let mut recurrence_parts = parts.clone();
    recurrence_parts.extend(start_defaults(&values, start));
    let recurrence_values = rule_values(&recurrence_parts)?;
    check_cost(&recurrence_values, reject_ineffective_set_positions)?;
    Ok(PreparedRRule {
        normalized: format!("{start_line}\n{rule_line}"),
        time_zone,
        zone,
        start,
        frequency: values.frequency,
        interval: values.interval,
        week_start: values.week_start,
        count: values.count,
        limit: values.until.unwrap_or(MAX_FOUR_DIGIT_YEAR_MS),
        lib: lib_rule(&recurrence_values),
        invalid_run_limit: CYCLE_MS * i128::from(values.interval)
            / gcd(
                i128::from(values.interval),
                values.frequency.cycle_periods(),
            ),
    })
}

/// Express an actual instant as UTC-shaped wall time in a zone.
fn wall_of(epoch: i64, zone: &Zone) -> i64 {
    epoch + zone.offset_ms_at(epoch)
}

/// Return the epoch day of UTC-shaped epoch milliseconds.
fn day_of(epoch: i64) -> i64 {
    epoch.div_euclid(MS_PER_DAY)
}

/// Return the epoch day of the week start at or before an epoch day; epoch day 0 is a Thursday.
fn week_start_day(day: i64, week_start: i64) -> i64 {
    day - (day + 3 - week_start).rem_euclid(7)
}

/// Return the absolute month number of UTC-shaped epoch milliseconds.
fn month_of(epoch: i64) -> i64 {
    let (year, month, ..) = fields_of(epoch);
    year * 12 + month - 1
}

/// Return the year of UTC-shaped epoch milliseconds.
fn year_of(epoch: i64) -> i64 {
    fields_of(epoch).0
}

/// Return the latest point `origin + k * step` at or before value.
fn floor_step(origin: i64, value: i64, step: i128) -> i64 {
    let k = i128::from(value - origin).div_euclid(step);
    i64::try_from(i128::from(origin) + k * step).unwrap_or(i64::MIN)
}

/// Return the start of the INTERVAL-aligned FREQ period that contains a wall time at or after DTSTART.
fn period_start(prepared: &PreparedRRule, wall: i64) -> i64 {
    let (start, interval, week_start) = (
        prepared.start,
        i128::from(prepared.interval),
        prepared.week_start,
    );
    match prepared.frequency {
        Frequency::Minutely => floor_step(start, wall, interval * 60_000),
        Frequency::Hourly => floor_step(start, wall, interval * 3_600_000),
        Frequency::Daily => floor_step(day_of(start), day_of(wall), interval) * MS_PER_DAY,
        Frequency::Weekly => {
            floor_step(
                week_start_day(day_of(start), week_start),
                week_start_day(day_of(wall), week_start),
                7 * interval,
            ) * MS_PER_DAY
        }
        Frequency::Monthly => {
            let month = floor_step(month_of(start), month_of(wall), interval);
            epoch_ms(
                month.div_euclid(12),
                month.rem_euclid(12) + 1,
                1,
                0,
                0,
                0,
                0,
            )
        }
        Frequency::Yearly => epoch_ms(
            floor_step(year_of(start), year_of(wall), interval),
            1,
            1,
            0,
            0,
            0,
            0,
        ),
    }
}

/// Return the enumeration start for the period that contains a wall time.
///
/// The first period keeps the original DTSTART, because the library truncates a first WEEKLY period at DTSTART.
fn period_anchor(prepared: &PreparedRRule, wall: i64) -> i64 {
    if wall > prepared.start {
        period_start(prepared, wall).max(prepared.start)
    } else {
        prepared.start
    }
}

/// Year-level facts the day filters of the recurrence library read.
struct YearInfo {
    year: i64,
    year_len: i64,
    first_day: i64,
    /// Weekday of January 1 with Monday 0.
    year_weekday: i64,
    /// Day-of-year index of the first day of each month, plus the year length.
    month_range: [i64; 13],
    week_no_mask: Option<Vec<bool>>,
    nth_weekday_mask: Option<Vec<bool>>,
}

impl YearInfo {
    fn new(year: i64) -> YearInfo {
        let year_len = if is_leap(year) { 366 } else { 365 };
        let first_day = days_from_civil(year, 1, 1);
        let mut month_range = [0; 13];
        for month in 1..=12 {
            month_range[month] = month_range[month - 1] + days_in_month(year, month as i64);
        }
        YearInfo {
            year,
            year_len,
            first_day,
            year_weekday: weekday_monday0(first_day),
            month_range,
            week_no_mask: None,
            nth_weekday_mask: None,
        }
    }

    fn weekday(&self, index: i64) -> i64 {
        (self.year_weekday + index).rem_euclid(7)
    }

    /// Mark the days of the weeks selected by BYWEEKNO.
    fn build_week_no_mask(&mut self, week_start: i64, by_week_no: &[i64]) {
        let mut mask = vec![false; (self.year_len + 7) as usize];
        let mut set = |index: i64| {
            if let Some(slot) = usize::try_from(index)
                .ok()
                .and_then(|index| mask.get_mut(index))
            {
                *slot = true;
            }
        };
        let (year_len, year_weekday) = (self.year_len, self.year_weekday);
        let weekday = |index: i64| (year_weekday + index).rem_euclid(7);
        let first_week_start = (7 - year_weekday + week_start).rem_euclid(7);
        let mut no1_week_start = first_week_start;
        let week_year_len = if no1_week_start >= 4 {
            no1_week_start = 0;
            year_len + (year_weekday - week_start).rem_euclid(7)
        } else {
            year_len - no1_week_start
        };
        let num_weeks = week_year_len / 7 + week_year_len % 7 / 4;
        for &number in by_week_no {
            let number = if number < 0 {
                number + num_weeks + 1
            } else {
                number
            };
            if !(0 < number && number <= num_weeks) {
                continue;
            }
            let mut index = if number > 1 {
                let mut index = no1_week_start + (number - 1) * 7;
                if no1_week_start != first_week_start {
                    index -= 7 - first_week_start;
                }
                index
            } else {
                no1_week_start
            };
            for _ in 0..7 {
                set(index);
                index += 1;
                if weekday(index) == week_start {
                    break;
                }
            }
        }
        if by_week_no.contains(&1) {
            // Check week number 1 of next year as well.
            let mut index = no1_week_start + num_weeks * 7;
            if no1_week_start != first_week_start {
                index -= 7 - first_week_start;
            }
            if index < year_len {
                for _ in 0..7 {
                    set(index);
                    index += 1;
                    if weekday(index) == week_start {
                        break;
                    }
                }
            }
        }
        if no1_week_start != 0 {
            // Check last week number of last year as well.
            let last_num_weeks = if by_week_no.contains(&-1) {
                -1
            } else {
                let last_year_weekday = weekday_monday0(days_from_civil(self.year - 1, 1, 1));
                let last_year_len = if is_leap(self.year - 1) { 366 } else { 365 };
                let weeks_start = if (7 - last_year_weekday + week_start).rem_euclid(7) >= 4 {
                    last_year_len + (last_year_weekday - week_start).rem_euclid(7)
                } else {
                    year_len - no1_week_start
                };
                52 + weeks_start.rem_euclid(7) / 4
            };
            if by_week_no.contains(&last_num_weeks) {
                for index in 0..no1_week_start {
                    set(index);
                }
            }
        }
        self.week_no_mask = Some(mask);
    }

    /// Mark the days selected by BYDAY entries with an ordinal, inside each range of days `[first, last)`.
    fn build_nth_weekday_mask(&mut self, ranges: &[(i64, i64)], by_nth_weekday: &[(i64, i64)]) {
        let mut mask = vec![false; self.year_len as usize];
        for &(first, end) in ranges {
            let last = end - 1;
            for &(weekday, n) in by_nth_weekday {
                let index = if n < 0 {
                    let index = last + (n + 1) * 7;
                    index - (self.weekday(index) - weekday).rem_euclid(7)
                } else {
                    let index = first + (n - 1) * 7;
                    index + (7 - self.weekday(index) + weekday).rem_euclid(7)
                };
                if first <= index && index <= last {
                    mask[index as usize] = true;
                }
            }
        }
        self.nth_weekday_mask = Some(mask);
    }
}

impl LibRule {
    /// Whether the library's day filters remove a day; `year` carries the year facts when the period is a month or
    /// year, with the day's index in that year.
    fn day_filtered(&self, day: i64, year: Option<(&YearInfo, i64)>) -> bool {
        let (civil_year, month, day_of_month) = civil_from_days(day);
        let weekday = weekday_monday0(day);
        let month_len = days_in_month(civil_year, month);
        if !self.by_month.is_empty() && !self.by_month.contains(&month) {
            return true;
        }
        if let Some((info, index)) = year
            && !self.by_week_no.is_empty()
            && !info
                .week_no_mask
                .as_ref()
                .is_some_and(|mask| mask[index as usize])
        {
            return true;
        }
        if !self.by_weekday.is_empty() && !self.by_weekday.contains(&weekday) {
            return true;
        }
        if let Some((info, index)) = year
            && info
                .nth_weekday_mask
                .as_ref()
                .is_some_and(|mask| !mask[index as usize])
        {
            return true;
        }
        if (!self.by_month_day.is_empty() || !self.by_negative_month_day.is_empty())
            && !self.by_month_day.contains(&day_of_month)
            && !self
                .by_negative_month_day
                .contains(&(day_of_month - month_len - 1))
        {
            return true;
        }
        if let Some((info, index)) = year
            && !self.by_year_day.is_empty()
            && !self.by_year_day.contains(&(index + 1))
            && !self.by_year_day.contains(&(-info.year_len + index))
        {
            return true;
        }
        false
    }

    /// Facts about the days of a year or month period, with the masks this rule needs.
    fn year_info(&self, year: i64, month: Option<i64>) -> YearInfo {
        let mut info = YearInfo::new(year);
        if !self.by_week_no.is_empty() {
            info.build_week_no_mask(self.week_start, &self.by_week_no);
        }
        if !self.by_nth_weekday.is_empty() {
            let ranges: Vec<(i64, i64)> = match (self.frequency, month) {
                (Frequency::Monthly, Some(month)) => {
                    vec![(
                        info.month_range[month as usize - 1],
                        info.month_range[month as usize],
                    )]
                }
                (Frequency::Yearly, _) if !self.by_month.is_empty() => self
                    .by_month
                    .iter()
                    .map(|month| {
                        (
                            info.month_range[*month as usize - 1],
                            info.month_range[*month as usize],
                        )
                    })
                    .collect(),
                (Frequency::Yearly, _) => vec![(0, info.year_len)],
                _ => Vec::new(),
            };
            if !ranges.is_empty() {
                info.build_nth_weekday_mask(&ranges, &self.by_nth_weekday);
            }
        }
        info
    }

    /// The days of the period, ascending, after the day filters.
    fn period_days(&self, period: &Period) -> Vec<i64> {
        match *period {
            Period::Year(year) => {
                let info = self.year_info(year, None);
                (0..info.year_len)
                    .filter(|index| {
                        !self.day_filtered(info.first_day + index, Some((&info, *index)))
                    })
                    .map(|index| info.first_day + index)
                    .collect()
            }
            Period::Month(year, month) => {
                let info = self.year_info(year, Some(month));
                (info.month_range[month as usize - 1]..info.month_range[month as usize])
                    .filter(|index| {
                        !self.day_filtered(info.first_day + index, Some((&info, *index)))
                    })
                    .map(|index| info.first_day + index)
                    .collect()
            }
            Period::Week(day) => {
                let mut days = Vec::new();
                let mut current = day;
                for _ in 0..7 {
                    if !self.day_filtered(current, None) {
                        days.push(current);
                    }
                    current += 1;
                    if weekday_monday0(current) == self.week_start {
                        break;
                    }
                }
                days
            }
            Period::Day(day) => {
                if self.day_filtered(day, None) {
                    Vec::new()
                } else {
                    vec![day]
                }
            }
        }
    }

    /// The BYSETPOS selection of a period, sorted ascending; entries that name no day are dropped.
    fn set_positions(&self, days: &[i64]) -> Vec<i64> {
        let times = self.timeset.len() as i64;
        let mut selected = Vec::new();
        for &position in &self.by_set_pos {
            let (day_position, time_position) = if position < 0 {
                (position.div_euclid(times), position.rem_euclid(times))
            } else {
                (
                    (position - 1).div_euclid(times),
                    (position - 1).rem_euclid(times),
                )
            };
            let index = if day_position < 0 {
                // `slice(-k)[0]` of the library: the whole array when it holds fewer than k days.
                Some((days.len() as i64 + day_position).max(0) as usize)
            } else {
                Some(day_position as usize)
            };
            let Some(day) = index.and_then(|index| days.get(index)) else {
                continue;
            };
            selected.push(day * MS_PER_DAY + self.timeset[time_position as usize]);
        }
        selected.sort_unstable();
        selected
    }
}

/// One FREQ period of the enumeration.
enum Period {
    Year(i64),
    Month(i64, i64),
    Week(i64),
    Day(i64),
}

/// Latest year the recurrence library iterates.
const MAX_YEAR: i64 = 9_999;

/// Enumerate UTC-shaped wall times of a rule from a period start, ascending, until `emit` answers `false` or the
/// year passes 9999.
fn enumerate(rule: &LibRule, anchor: i64, mut emit: impl FnMut(i64) -> bool) {
    let interval = rule.interval;
    let limit = days_from_civil(MAX_YEAR + 1, 1, 1) * MS_PER_DAY;
    if matches!(rule.frequency, Frequency::Minutely | Frequency::Hourly) {
        let step = i128::from(interval)
            * if rule.frequency == Frequency::Minutely {
                60_000
            } else {
                3_600_000
            };
        if !emit(anchor) {
            return;
        }
        let mut count: i128 = 1;
        loop {
            let wall = i128::from(anchor) + count * step;
            if wall >= i128::from(limit) || !emit(wall as i64) {
                return;
            }
            count += 1;
        }
    }
    let anchor_day = day_of(anchor);
    let (anchor_year, anchor_month, _) = civil_from_days(anchor_day);
    let mut period = match rule.frequency {
        Frequency::Yearly => Period::Year(anchor_year),
        Frequency::Monthly => Period::Month(anchor_year, anchor_month),
        Frequency::Weekly => Period::Week(anchor_day),
        _ => Period::Day(anchor_day),
    };
    loop {
        let days = rule.period_days(&period);
        if rule.by_set_pos.is_empty() {
            for day in &days {
                for time in &rule.timeset {
                    let wall = day * MS_PER_DAY + time;
                    if wall >= anchor && !emit(wall) {
                        return;
                    }
                }
            }
        } else {
            for wall in rule.set_positions(&days) {
                if wall >= anchor && !emit(wall) {
                    return;
                }
            }
        }
        period = match period {
            Period::Year(year) => {
                let next = year.saturating_add(interval);
                if next > MAX_YEAR {
                    return;
                }
                Period::Year(next)
            }
            Period::Month(year, month) => {
                let index = (year * 12 + month - 1).saturating_add(interval);
                let next_year = index.div_euclid(12);
                if next_year > MAX_YEAR {
                    return;
                }
                Period::Month(next_year, index.rem_euclid(12) + 1)
            }
            Period::Week(day) => {
                let next = week_start_day(day, rule.week_start).saturating_add(7 * interval);
                if civil_from_days(next).0 > MAX_YEAR {
                    return;
                }
                Period::Week(next)
            }
            Period::Day(day) => {
                let next = day.saturating_add(interval);
                if civil_from_days(next).0 > MAX_YEAR {
                    return;
                }
                Period::Day(next)
            }
        };
    }
}

/// Report whether the rule can generate a wall time.
///
/// Whole 400-year cycles preserve the Gregorian calendar, so the scan starts at the latest equivalent period that
/// leaves one complete calendar and INTERVAL cycle before year 9999.
fn has_candidate(prepared: &PreparedRRule) -> bool {
    if matches!(prepared.frequency, Frequency::Minutely | Frequency::Hourly) {
        return true;
    }
    let first = period_start(prepared, prepared.start);
    let first_year = i128::from(year_of(first));
    let interval = i128::from(prepared.interval);
    let cycle_years =
        CYCLE_PERIODS_YEARLY * interval / gcd(interval, prepared.frequency.cycle_periods());
    let cycles = i128::from(first_year < i128::from(MIN_RRULE_YEAR))
        .max((9_999 - first_year - cycle_years - 1).div_euclid(CYCLE_PERIODS_YEARLY));
    let anchor = i64::try_from(i128::from(first) + cycles * CYCLE_MS).unwrap_or(i64::MAX);
    let mut found = false;
    enumerate(&prepared.lib, anchor, |_| {
        found = true;
        false
    });
    found
}

/// Find the latest occurrence at or before a decision among wall times from one start, and the first later
/// occurrence. Wall times at or before the decision wall time convert to instants only from the latest one backwards.
fn scan_window(prepared: &PreparedRRule, from_wall: i64, decision: i64) -> OccurrenceWindow {
    let mut window = OccurrenceWindow::default();
    if from_wall > LAST_RRULE_WALL_MS {
        return window;
    }
    let bound = decision.min(prepared.limit);
    let bound_wall = wall_of(bound, &prepared.zone);
    let mut below: Vec<i64> = Vec::new();
    let mut last_valid_wall = bound_wall;
    enumerate(&prepared.lib, period_anchor(prepared, from_wall), |wall| {
        if wall < from_wall {
            return true;
        }
        if wall <= bound_wall {
            below.push(wall);
            return true;
        }
        // UTC offsets are shorter than one day.
        if wall >= prepared.limit + MS_PER_DAY {
            return false;
        }
        let Some(instant) = instant_of(&prepared.zone, wall) else {
            return i128::from(wall - last_valid_wall) <= prepared.invalid_run_limit;
        };
        last_valid_wall = wall;
        if instant <= bound {
            window.latest = Some(instant);
            return true;
        }
        if instant <= prepared.limit {
            window.next = Some(instant);
        }
        false
    });
    while window.latest.is_none() {
        let Some(wall) = below.pop() else { break };
        if let Some(instant) = instant_of(&prepared.zone, wall).filter(|instant| *instant <= bound)
        {
            window.latest = Some(instant);
        }
    }
    window
}

/// Find the latest due occurrence; the window starts two days before the decision and doubles while it has none.
fn dispatch_window(prepared: &PreparedRRule, saved: i64, decision: i64) -> OccurrenceWindow {
    let saved_wall = wall_of(saved, &prepared.zone);
    let bound_wall = wall_of(decision.min(prepared.limit), &prepared.zone);
    let mut span = 2 * MS_PER_DAY;
    loop {
        let from_wall = (bound_wall - span).max(saved_wall);
        let window = scan_window(prepared, from_wall, decision);
        if window.latest.is_some() || from_wall == saved_wall {
            return window;
        }
        span = span.saturating_mul(2);
    }
}

/// Enumerate distinct COUNT instances from DTSTART through the first occurrence after a decision.
fn count_window(prepared: &PreparedRRule, decision: i64, count: i64) -> OccurrenceWindow {
    let mut window = OccurrenceWindow::default();
    let mut remaining = count;
    let mut last_valid_wall = prepared.start;
    enumerate(&prepared.lib, prepared.start, |wall| {
        let Some(instant) = instant_of(&prepared.zone, wall) else {
            return i128::from(wall - last_valid_wall) <= prepared.invalid_run_limit;
        };
        if Some(instant) == window.latest {
            return true;
        }
        last_valid_wall = wall;
        if instant > decision {
            if instant <= MAX_FOUR_DIGIT_YEAR_MS {
                window.next = Some(instant);
            }
            return false;
        }
        window.latest = Some(instant);
        remaining -= 1;
        remaining > 0
    });
    window
}

/// Format the first occurrence strictly after a representable reference time.
fn next_occurrence(prepared: &PreparedRRule, after: i64) -> Option<String> {
    if !has_candidate(prepared) {
        return None;
    }
    let next = match prepared.count {
        None => scan_window(prepared, wall_of(after, &prepared.zone), after).next,
        Some(count) => count_window(prepared, after, count).next,
    };
    next.map(iso_string)
}

/// Optional stricter authoring checks of [`RRule::parse`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RRuleParseOptions {
    /// Refuse a BYSETPOS that cannot select among more than one occurrence of a period.
    pub reject_ineffective_set_positions: bool,
}

/// Parsed two-line DTSTART/RRULE value with bounded occurrence arithmetic.
#[derive(Debug, Clone)]
pub struct RRule {
    prepared: PreparedRRule,
}

impl RRule {
    /// Trimmed two-line expression; DTSTART owns the zone and recurrence anchor.
    pub fn expression(&self) -> &str {
        &self.prepared.normalized
    }

    /// Canonical zone from DTSTART, or UTC for a Z start.
    pub fn time_zone(&self) -> &str {
        &self.prepared.time_zone
    }

    /// Validate syntax and enumeration cost before any occurrence search.
    pub fn parse(expression: &str, options: RRuleParseOptions) -> RecurrenceResult<RRule> {
        parse_recurrence(|| {
            Ok(RRule {
                prepared: prepare_rrule(expression, options.reject_ineffective_set_positions)?,
            })
        })
    }

    /// First occurrence strictly after the explicit lower bound, or exhaustion.
    ///
    /// # Errors
    /// [`TimeError::Range`] when `after` is not a four-digit-year instant.
    pub fn next(&self, after: i64) -> Result<Option<String>, TimeError> {
        if !is_four_digit_year_instant(after) {
            return Err(TimeError::Range(
                "RRULE lower bound must be a four-digit-year instant".to_owned(),
            ));
        }
        Ok(next_occurrence(&self.prepared, after))
    }

    /// Latest due occurrence and first future occurrence, retaining the committed floor.
    ///
    /// # Errors
    /// [`TimeError::Range`] when either instant is invalid or the decision precedes the committed occurrence.
    pub fn resolve(
        &self,
        scheduled_at: &str,
        accepted_at: i64,
    ) -> Result<RecurrenceOccurrence, TimeError> {
        let saved_target = recurrence_decision(scheduled_at, accepted_at)?;
        let prepared = &self.prepared;
        let window = match prepared.count {
            None => dispatch_window(prepared, saved_target, accepted_at),
            Some(count) => count_window(prepared, accepted_at, count),
        };
        let occurrence_at = iso_string(window.latest.unwrap_or(saved_target).max(saved_target));
        Ok(RecurrenceOccurrence {
            occurrence_at,
            next_scheduled_at: window.next.map(iso_string),
        })
    }
}
