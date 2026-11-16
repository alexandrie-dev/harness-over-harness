//! Port of `@deepseek-ai/dsh-util-time` (`packages/util/time`).
//!
//! Time vocabulary shared by the boundaries that accept a caller's zone or a wall-clock selector, elapsed span, or
//! bounded Cron/RRULE recurrence. The library validates, canonicalizes, parses, and renders one canonical instant
//! format; it produces no model-facing text and owns no failure vocabulary.

mod calendar;
mod error;
mod zone;
mod zone_links;

pub mod cron;
pub mod duration;
pub mod instant;
pub mod recurrence;
pub mod rrule;

pub use cron::{CRON_SEARCH_HORIZON_YEARS, Cron, CronFields, CronWork};
pub use duration::{Duration, DurationParts};
pub use error::TimeError;
pub use instant::{
    InstantRejection, InstantResult, LocalCalendarRejection, LocalCalendarResult,
    LocalCalendarTime, MAX_FOUR_DIGIT_YEAR_MS, MIN_FOUR_DIGIT_YEAR_MS, TimeZoneCause,
    TimeZoneRejection, TimeZoneResult, canonicalize_time_zone, format_four_digit_year_instant,
    four_digit_year_epoch, is_four_digit_year_instant, parse_local_calendar, parse_offset_instant,
    resolve_local_instant,
};
pub use recurrence::{
    RecurrenceCause, RecurrenceOccurrence, RecurrenceRejection, RecurrenceResult,
};
pub use rrule::{RRule, RRuleParseOptions};

/// Validate and canonicalize one caller-supplied IANA zone at a wire boundary.
///
/// The canonical name is what a later reader needs: a zone identity is stored on durable records and resolved again
/// by another process, so an alias accepted here would not compare equal to the zone a reader derives.
/// Answers the canonical zone, or `None` when the name is unusable.
pub fn canonical_client_time_zone(value: &str) -> Option<String> {
    match canonicalize_time_zone(value) {
        TimeZoneResult::Accepted { time_zone } => Some(time_zone),
        TimeZoneResult::Rejected { .. } => None,
    }
}
