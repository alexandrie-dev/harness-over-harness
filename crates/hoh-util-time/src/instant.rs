//! Strict instant and local-calendar vocabulary for the boundaries that accept a wall-clock selector.
//!
//! Every entry that reads untrusted text answers a discriminated result instead of failing: the library carries no
//! failure vocabulary, and each caller maps a reason onto its own domain code and diagnostic.
//! [`format_four_digit_year_instant`] and [`is_four_digit_year_instant`] never fail, while [`resolve_local_instant`]
//! answers a [`TimeError::Range`] when a caller violates its precondition: an unknown zone, or calendar fields naming
//! a value the calendar does not have.

use crate::calendar::{days_in_month, epoch_ms, is_js_whitespace, iso_string};
use crate::error::TimeError;
use crate::zone::Zone;
use crate::zone_links::ZONE_LINKS;

/// Inclusive lower epoch-millisecond bound of a four-digit-year RFC 3339 UTC instant (`0001-01-01T00:00:00.000Z`).
pub const MIN_FOUR_DIGIT_YEAR_MS: i64 = -62_135_596_800_000;

/// Inclusive upper epoch-millisecond bound of a four-digit-year RFC 3339 UTC instant (`9999-12-31T23:59:59.999Z`).
pub const MAX_FOUR_DIGIT_YEAR_MS: i64 = 253_402_300_799_999;

/// Exact calendar fields of one local wall-clock value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalCalendarTime {
    /// Year of the calendar date. `parse_local_calendar` rejects year 0 in untrusted text.
    pub year: i32,
    /// Month from 1 through 12.
    pub month: u32,
    /// Day of month from 1 through the month's length.
    pub day: u32,
    /// Hour from 0 through 23.
    pub hour: u32,
    /// Minute from 0 through 59.
    pub minute: u32,
    /// Second from 0 through 59; leap seconds are rejected.
    pub second: u32,
    /// Millisecond from 0 through 999.
    pub millisecond: u32,
}

/// Why an RFC 3339 instant string is unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstantRejection {
    /// `not_rfc3339`
    NotRfc3339,
    /// `out_of_range`: year 0, or an hour, minute, or second above its maximum.
    OutOfRange,
    /// `not_a_calendar_date`
    NotACalendarDate,
    /// `invalid_numeric_offset`
    InvalidNumericOffset,
}

impl InstantRejection {
    /// The stable reason text of the source.
    pub fn as_str(self) -> &'static str {
        match self {
            InstantRejection::NotRfc3339 => "not_rfc3339",
            InstantRejection::OutOfRange => "out_of_range",
            InstantRejection::NotACalendarDate => "not_a_calendar_date",
            InstantRejection::InvalidNumericOffset => "invalid_numeric_offset",
        }
    }
}

/// Why a local calendar selector is unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCalendarRejection {
    /// `malformed`
    Malformed,
    /// `out_of_range`
    OutOfRange,
    /// `not_a_calendar_date`
    NotACalendarDate,
}

impl LocalCalendarRejection {
    /// The stable reason text of the source.
    pub fn as_str(self) -> &'static str {
        match self {
            LocalCalendarRejection::Malformed => "malformed",
            LocalCalendarRejection::OutOfRange => "out_of_range",
            LocalCalendarRejection::NotACalendarDate => "not_a_calendar_date",
        }
    }
}

/// Why one IANA zone selector is unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeZoneRejection {
    /// `blank`
    Blank,
    /// `padded`
    Padded,
    /// `not_area_location`
    NotAreaLocation,
    /// `unsupported`
    Unsupported,
    /// `not_canonical_shape`
    NotCanonicalShape,
}

impl TimeZoneRejection {
    /// The stable reason text of the source.
    pub fn as_str(self) -> &'static str {
        match self {
            TimeZoneRejection::Blank => "blank",
            TimeZoneRejection::Padded => "padded",
            TimeZoneRejection::NotAreaLocation => "not_area_location",
            TimeZoneRejection::Unsupported => "unsupported",
            TimeZoneRejection::NotCanonicalShape => "not_canonical_shape",
        }
    }
}

/// Discriminated result of parsing one RFC 3339 instant string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstantResult {
    /// The text is usable.
    Accepted {
        /// Epoch milliseconds.
        epoch: i64,
    },
    /// The text is unusable.
    Rejected {
        /// The stable reason.
        reason: InstantRejection,
    },
}

/// Discriminated result of parsing one local calendar selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCalendarResult {
    /// The selector is usable.
    Accepted {
        /// The parsed calendar fields.
        time: LocalCalendarTime,
    },
    /// The selector is unusable.
    Rejected {
        /// The stable reason.
        reason: LocalCalendarRejection,
    },
}

/// The platform failure behind a refused well-formed zone name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeZoneCause {
    /// The `RangeError` the platform raised, with its message.
    RangeError {
        /// The platform's message.
        message: String,
    },
}

/// Discriminated result of resolving one raw IANA zone selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeZoneResult {
    /// The name is usable.
    Accepted {
        /// The canonical zone name.
        time_zone: String,
    },
    /// The name is unusable.
    Rejected {
        /// The stable reason.
        reason: TimeZoneRejection,
        /// Platform error caught while resolving the name; present when the platform refused a well-formed one.
        cause: Option<TimeZoneCause>,
    },
}

/// Read a run of ASCII digits as a number.
fn number(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        bytes
            .iter()
            .fold(0, |value, byte| value * 10 + i64::from(byte - b'0')),
    )
}

/// Normalize an optional one-to-three digit fractional second to milliseconds.
fn fractional_milliseconds(digits: &[u8]) -> i64 {
    let mut value = 0;
    for index in 0..3 {
        value = value * 10 + digits.get(index).map_or(0, |byte| i64::from(byte - b'0'));
    }
    value
}

/// Whether the clock fields of untrusted text are out of range (year 0 included).
fn out_of_range(year: i64, hour: i64, minute: i64, second: i64) -> bool {
    year == 0 || hour > 23 || minute > 59 || second > 59
}

/// Whether the date fields name a day the calendar has.
fn is_calendar_date(year: i64, month: i64, day: i64) -> bool {
    (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)
}

/// Whether an integral epoch is inside the four-digit-year range.
fn in_four_digit_year_range(epoch: i64) -> bool {
    (MIN_FOUR_DIGIT_YEAR_MS..=MAX_FOUR_DIGIT_YEAR_MS).contains(&epoch)
}

/// Split `YYYY-MM-DD` into its numbers; `None` unless the text is exactly that shape.
fn local_date_parts(text: &str) -> Option<(i64, i64, i64)> {
    let b = text.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    Some((number(&b[0..4])?, number(&b[5..7])?, number(&b[8..10])?))
}

/// Split `HH:MM:SS` with an optional one-to-three digit fraction into its numbers.
fn local_time_parts(text: &str) -> Option<(i64, i64, i64, i64)> {
    let b = text.as_bytes();
    if b.len() < 8 || b[2] != b':' || b[5] != b':' {
        return None;
    }
    let (hour, minute, second) = (number(&b[0..2])?, number(&b[3..5])?, number(&b[6..8])?);
    if b.len() == 8 {
        return Some((hour, minute, second, 0));
    }
    let fraction = &b[9..];
    if b[8] != b'.' || fraction.is_empty() || fraction.len() > 3 || number(fraction).is_none() {
        return None;
    }
    Some((hour, minute, second, fractional_milliseconds(fraction)))
}

/// Read one local calendar selector strictly, without consulting a process zone.
///
/// The date and time texts are matched separately, so a malformed spelling, a non-existent calendar date such as
/// February 30, and an out-of-range clock field answer distinct reasons.
pub fn parse_local_calendar(date: &str, time: &str) -> LocalCalendarResult {
    let (Some((year, month, day)), Some((hour, minute, second, millisecond))) =
        (local_date_parts(date), local_time_parts(time))
    else {
        return LocalCalendarResult::Rejected {
            reason: LocalCalendarRejection::Malformed,
        };
    };
    if out_of_range(year, hour, minute, second) {
        return LocalCalendarResult::Rejected {
            reason: LocalCalendarRejection::OutOfRange,
        };
    }
    if !is_calendar_date(year, month, day) {
        return LocalCalendarResult::Rejected {
            reason: LocalCalendarRejection::NotACalendarDate,
        };
    }
    // Every field is range-checked above, so the narrowing below cannot lose a value.
    LocalCalendarResult::Accepted {
        time: LocalCalendarTime {
            year: year as i32,
            month: month as u32,
            day: day as u32,
            hour: hour as u32,
            minute: minute as u32,
            second: second as u32,
            millisecond: millisecond as u32,
        },
    }
}

/// Parse one RFC 3339 instant whose numeric offset is part of the input.
///
/// A bare `Z` and an explicit numeric offset are both accepted, and the offset is subtracted from the local
/// wall-clock value. A negative zero offset, an offset whose hour or minute is out of range, and a non-existent
/// calendar date are refused.
pub fn parse_offset_instant(value: &str) -> InstantResult {
    let reject = |reason| InstantResult::Rejected { reason };
    let b = value.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return reject(InstantRejection::NotRfc3339);
    }
    let fields = (
        number(&b[0..4]),
        number(&b[5..7]),
        number(&b[8..10]),
        number(&b[11..13]),
        number(&b[14..16]),
        number(&b[17..19]),
    );
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = fields
    else {
        return reject(InstantRejection::NotRfc3339);
    };
    let mut rest = &b[19..];
    let mut millisecond = 0;
    if rest.first() == Some(&b'.') {
        let digits = rest[1..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if !(1..=3).contains(&digits) {
            return reject(InstantRejection::NotRfc3339);
        }
        millisecond = fractional_milliseconds(&rest[1..=digits]);
        rest = &rest[1 + digits..];
    }
    let offset = if rest == b"Z" {
        None
    } else if rest.len() == 6 && (rest[0] == b'+' || rest[0] == b'-') && rest[3] == b':' {
        match (number(&rest[1..3]), number(&rest[4..6])) {
            (Some(hours), Some(minutes)) => Some((rest[0], hours, minutes)),
            _ => return reject(InstantRejection::NotRfc3339),
        }
    } else {
        return reject(InstantRejection::NotRfc3339);
    };
    if out_of_range(year, hour, minute, second) {
        return reject(InstantRejection::OutOfRange);
    }
    if !is_calendar_date(year, month, day) {
        return reject(InstantRejection::NotACalendarDate);
    }
    let local = epoch_ms(year, month, day, hour, minute, second, millisecond);
    let Some((sign, offset_hour, offset_minute)) = offset else {
        return InstantResult::Accepted { epoch: local };
    };
    if offset_hour > 23
        || offset_minute > 59
        || (sign == b'-' && offset_hour == 0 && offset_minute == 0)
    {
        return reject(InstantRejection::InvalidNumericOffset);
    }
    let direction = if sign == b'+' { 1 } else { -1 };
    InstantResult::Accepted {
        epoch: local - direction * (offset_hour * 60 + offset_minute) * 60_000,
    }
}

/// Validate one canonical four-digit-year RFC 3339 UTC instant.
///
/// The accepted profile carries exactly three fractional digits and a `Z`, and every year from 0001 through 9999
/// whose calendar date exists is representable.
pub fn four_digit_year_epoch(value: &str) -> InstantResult {
    let reject = |reason| InstantResult::Rejected { reason };
    let b = value.as_bytes();
    let shape = b.len() == 24
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'.'
        && b[23] == b'Z';
    let fields = if shape {
        (
            number(&b[0..4]),
            number(&b[5..7]),
            number(&b[8..10]),
            number(&b[11..13]),
            number(&b[14..16]),
            number(&b[17..19]),
            number(&b[20..23]),
        )
    } else {
        (None, None, None, None, None, None, None)
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second), Some(milli)) =
        fields
    else {
        return reject(InstantRejection::NotRfc3339);
    };
    if year == 0
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return reject(InstantRejection::NotRfc3339);
    }
    if !is_calendar_date(year, month, day) {
        return reject(InstantRejection::NotACalendarDate);
    }
    InstantResult::Accepted {
        epoch: epoch_ms(year, month, day, hour, minute, second, milli),
    }
}

/// Whether an epoch is representable as a canonical four-digit-year RFC 3339 UTC instant.
pub fn is_four_digit_year_instant(epoch: i64) -> bool {
    in_four_digit_year_range(epoch)
}

/// Format one epoch as the canonical four-digit-year RFC 3339 UTC instant; `None` when it is unrepresentable.
pub fn format_four_digit_year_instant(epoch: i64) -> Option<String> {
    in_four_digit_year_range(epoch).then(|| iso_string(epoch))
}

/// Strict browser-zone shape: an IANA `Area/Location`-style identifier (`UTC` is admitted separately).
fn is_area_location(value: &str) -> bool {
    let mut segments = value.split('/');
    let (Some(first), Some(second)) = (segments.next(), segments.next()) else {
        return false;
    };
    let tail_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '.' | '-');
    let mut head = first.chars();
    head.next().is_some_and(|c| c.is_ascii_alphabetic())
        && head.all(tail_char)
        && [second]
            .into_iter()
            .chain(segments)
            .all(|segment| !segment.is_empty() && segment.chars().all(tail_char))
}

/// Names the platform reports as the zone `UTC`.
fn is_utc_alias(name: &str) -> bool {
    matches!(
        name,
        "Etc/UTC"
            | "Etc/UCT"
            | "Etc/Universal"
            | "Etc/Zulu"
            | "Etc/GMT"
            | "Etc/Greenwich"
            | "Etc/GMT0"
            | "Etc/GMT+0"
            | "Etc/GMT-0"
    )
}

/// The link target of an alias in the zone data, when it is one.
fn link_target(alias: &str) -> Option<&'static str> {
    ZONE_LINKS
        .binary_search_by(|(link, _)| (*link).cmp(alias))
        .ok()
        .map(|index| ZONE_LINKS[index].1)
}

/// Find the name the zone data knows for a requested name: exact first, then ignoring ASCII case.
///
/// The answer is the spelling the zone data uses, never the caller's. Only names the zone data lists are known:
/// `jiff` itself also admits special identifiers such as `Etc/Unknown`, which the platform of the source refuses.
fn known_name(value: &str) -> Option<String> {
    if value == "UTC" {
        return Some(value.to_owned());
    }
    if let Some((link, _)) = ZONE_LINKS.iter().find(|(link, _)| *link == value) {
        return Some((*link).to_owned());
    }
    let listed = || {
        jiff::tz::db()
            .available()
            .map(|name| name.as_str().to_owned())
    };
    if let Some(name) = listed().find(|name| name == value) {
        return Some(name);
    }
    if let Some((link, _)) = ZONE_LINKS
        .iter()
        .find(|(link, _)| link.eq_ignore_ascii_case(value))
    {
        return Some((*link).to_owned());
    }
    listed().find(|name| name.eq_ignore_ascii_case(value))
}

/// Resolve a requested name to its canonical spelling and zone, or the platform's refusal text.
fn canonical_zone(value: &str) -> Result<(String, Zone), String> {
    let refused = || format!("Invalid time zone specified: {value}");
    let known = known_name(value).ok_or_else(refused)?;
    let target = link_target(&known).map_or(known.clone(), str::to_owned);
    let canonical = if is_utc_alias(&target) {
        "UTC".to_owned()
    } else {
        target
    };
    let zone = Zone::get(&canonical).map_err(|_| refused())?;
    Ok((canonical, zone))
}

/// Validate and canonicalize a zone selector, keeping the resolved zone for callers inside this crate.
pub(crate) fn canonicalize_zone(value: &str) -> Result<(String, Zone), TimeZoneResult> {
    let rejected = |reason| TimeZoneResult::Rejected {
        reason,
        cause: None,
    };
    if value.is_empty() {
        return Err(rejected(TimeZoneRejection::Blank));
    }
    if value.trim_matches(is_js_whitespace) != value {
        return Err(rejected(TimeZoneRejection::Padded));
    }
    if value != "UTC" && !is_area_location(value) {
        return Err(rejected(TimeZoneRejection::NotAreaLocation));
    }
    // The platform refuses a well-formed name its zone data does not know; its text is the caller's only evidence.
    let (canonical, zone) = canonical_zone(value).map_err(|message| TimeZoneResult::Rejected {
        reason: TimeZoneRejection::Unsupported,
        cause: Some(TimeZoneCause::RangeError { message }),
    })?;
    if canonical != "UTC" && !is_area_location(&canonical) {
        return Err(rejected(TimeZoneRejection::NotCanonicalShape));
    }
    Ok((canonical, zone))
}

/// Validate and canonicalize one raw IANA zone selector.
///
/// A zone identity is stored on durable records and resolved again by another process, so an alias is answered by
/// its canonical name. Abbreviations, numeric offsets, single-segment names, and unknown names are refused.
pub fn canonicalize_time_zone(value: &str) -> TimeZoneResult {
    match canonicalize_zone(value) {
        Ok((time_zone, _)) => TimeZoneResult::Accepted { time_zone },
        Err(rejected) => rejected,
    }
}

/// Resolve one local wall-clock value in an explicit zone.
///
/// A value inside a forward transition's gap does not exist, so it answers `None`; a value inside an overlap
/// resolves to the earlier instant.
///
/// # Errors
/// [`TimeError::Range`] when `time_zone` is not a known zone or the calendar value does not exist.
pub fn resolve_local_instant(
    time: &LocalCalendarTime,
    time_zone: &str,
) -> Result<Option<i64>, TimeError> {
    let range = |message: String| TimeError::Range(message);
    let (year, month, day) = (
        i64::from(time.year),
        i64::from(time.month),
        i64::from(time.day),
    );
    let valid = is_calendar_date(year, month, day)
        && time.hour < 24
        && time.minute < 60
        && time.second < 60
        && time.millisecond < 1_000;
    if !valid {
        return Err(range(format!("invalid calendar fields {time:?}")));
    }
    let (_, zone) = canonical_zone(time_zone).map_err(range)?;
    zone.resolve((
        year,
        month,
        day,
        i64::from(time.hour),
        i64::from(time.minute),
        i64::from(time.second),
        i64::from(time.millisecond),
    ))
    .map_err(range)
}
