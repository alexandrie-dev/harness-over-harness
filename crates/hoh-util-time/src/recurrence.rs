//! Bounded calendar recurrence shared by consumers of Cron and RRULE.

use crate::error::TimeError;
use crate::instant::{
    InstantResult, TimeZoneResult, four_digit_year_epoch, is_four_digit_year_instant,
};

/// Latest admitted occurrence and the first future occurrence, if one exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceOccurrence {
    /// Latest due UTC occurrence, never before the committed lower bound.
    pub occurrence_at: String,
    /// First strictly future UTC occurrence, omitted at exhaustion.
    pub next_scheduled_at: Option<String>,
}

/// Why recurrence content cannot be evaluated by the bounded engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceRejection {
    /// `syntax`
    Syntax,
    /// `time_zone`
    TimeZone,
    /// `density`
    Density,
}

impl RecurrenceRejection {
    /// The stable reason text of the source.
    pub fn as_str(self) -> &'static str {
        match self {
            RecurrenceRejection::Syntax => "syntax",
            RecurrenceRejection::TimeZone => "time_zone",
            RecurrenceRejection::Density => "density",
        }
    }
}

/// The failure behind a recurrence refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecurrenceCause {
    /// The rejected zone result, as the source carries it.
    TimeZone(TimeZoneResult),
    /// A calendar failure of the platform, with its description.
    Detail(String),
}

/// A parsed recurrence or its validation reason; no consumer-specific error codes.
#[derive(Debug, Clone, PartialEq)]
pub enum RecurrenceResult<T> {
    /// The input is usable.
    Accepted {
        /// The parsed rule.
        rule: T,
    },
    /// The input is unusable.
    Rejected {
        /// The stable reason.
        reason: RecurrenceRejection,
        /// Human-readable detail, byte for byte as the source words it.
        detail: String,
        /// The underlying failure, when there is one.
        cause: Option<RecurrenceCause>,
    },
}

impl<T> RecurrenceResult<T> {
    /// The parsed rule, or the refusal detail as an error.
    pub fn into_rule(self) -> Result<T, String> {
        match self {
            RecurrenceResult::Accepted { rule } => Ok(rule),
            RecurrenceResult::Rejected { detail, .. } => Err(detail),
        }
    }
}

/// Internal validation failure, contained by the public parsers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceInputError {
    /// Why the content was refused.
    pub reason: RecurrenceRejection,
    /// The refusal text.
    pub message: String,
    /// The underlying failure, when there is one.
    pub cause: Option<RecurrenceCause>,
}

impl RecurrenceInputError {
    /// A refusal without an underlying failure.
    pub fn new(reason: RecurrenceRejection, message: impl Into<String>) -> RecurrenceInputError {
        RecurrenceInputError {
            reason,
            message: message.into(),
            cause: None,
        }
    }

    /// A refusal that carries the failure behind it.
    pub fn with_cause(
        reason: RecurrenceRejection,
        message: impl Into<String>,
        cause: RecurrenceCause,
    ) -> RecurrenceInputError {
        RecurrenceInputError {
            reason,
            message: message.into(),
            cause: Some(cause),
        }
    }
}

/// Contain recurrence validation failures: the parsed value or its validation refusal.
pub fn parse_recurrence<T>(
    parse: impl FnOnce() -> Result<T, RecurrenceInputError>,
) -> RecurrenceResult<T> {
    match parse() {
        Ok(rule) => RecurrenceResult::Accepted { rule },
        Err(error) => RecurrenceResult::Rejected {
            reason: error.reason,
            detail: error.message,
            cause: error.cause,
        },
    }
}

/// Validate the explicit decision and its committed lower bound before calendar arithmetic.
///
/// # Errors
/// [`TimeError::Range`] for an invalid decision.
pub fn recurrence_decision(scheduled_at: &str, accepted_at: i64) -> Result<i64, TimeError> {
    match four_digit_year_epoch(scheduled_at) {
        InstantResult::Accepted { epoch }
            if is_four_digit_year_instant(accepted_at) && accepted_at >= epoch =>
        {
            Ok(epoch)
        }
        _ => Err(TimeError::Range(
            "Recurrence decision must be representable and at or after its committed target"
                .to_owned(),
        )),
    }
}
