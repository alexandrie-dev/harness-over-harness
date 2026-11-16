//! One span of time, held as a count of milliseconds and split into the whole parts a reading is written in.
//!
//! The count is a binary floating-point number because the source keeps fractional milliseconds.

use crate::error::TimeError;

const MS_PER_SECOND: f64 = 1_000.0;
const SECONDS_PER_DAY: f64 = 86_400.0;
const SECONDS_PER_HOUR: f64 = 3_600.0;
const SECONDS_PER_MINUTE: f64 = 60.0;

/// Largest millisecond count a span holds (`Number.MAX_SAFE_INTEGER`): every integer up to it is exactly
/// representable, which is what makes the whole-part split exact.
const MAX_SPAN_MS: f64 = 9_007_199_254_740_991.0;

/// Whole parts of one non-negative span, in the units a reading is written in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DurationParts {
    /// Whole days.
    pub days: f64,
    /// Whole hours left after the days, from 0 through 23.
    pub hours: f64,
    /// Whole minutes left after the hours, from 0 through 59.
    pub minutes: f64,
    /// Whole seconds left after the minutes, from 0 through 59.
    pub seconds: f64,
    /// Fractional milliseconds left after the seconds, from 0 up to (but excluding) 1000.
    pub milliseconds: f64,
}

/// One span of time in milliseconds.
///
/// A span is signed: a negative count builds, but [`Duration::decompose`] refuses it because a negative interval has
/// no single whole-part split. A span is bounded by the exactly representable millisecond range and carries no
/// calendar meaning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Duration {
    span: f64,
}

/// `String(number)` of JavaScript for the values this module reports.
fn js_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    if value.abs() >= 1e21 {
        let text = format!("{value:e}");
        return match text.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => {
                format!("{mantissa}e+{exponent}")
            }
            _ => text,
        };
    }
    format!("{value}")
}

impl Duration {
    /// Build one span from a millisecond count. A negative count is accepted; such a span cannot be decomposed.
    ///
    /// # Errors
    /// [`TimeError::Type`] when the count is not finite, or exceeds the exactly representable millisecond range.
    pub fn from_milliseconds(milliseconds: f64) -> Result<Duration, TimeError> {
        if !milliseconds.is_finite() {
            return Err(TimeError::Type(format!(
                "duration milliseconds must be finite, got {}",
                js_number(milliseconds)
            )));
        }
        if milliseconds.abs() > MAX_SPAN_MS {
            return Err(TimeError::Type(format!(
                "duration milliseconds must not exceed {} so its parts stay exact, got {}",
                js_number(MAX_SPAN_MS),
                js_number(milliseconds)
            )));
        }
        Ok(Duration { span: milliseconds })
    }

    /// Split the span into whole days, hours, minutes, seconds, and the remaining fraction of a second.
    ///
    /// # Errors
    /// [`TimeError::Range`] when the span is negative.
    pub fn decompose(&self) -> Result<DurationParts, TimeError> {
        if self.span < 0.0 {
            return Err(TimeError::Range(
                "a negative duration has no whole-part decomposition".to_owned(),
            ));
        }
        let mut seconds = (self.span / MS_PER_SECOND).floor();
        let days = (seconds / SECONDS_PER_DAY).floor();
        seconds %= SECONDS_PER_DAY;
        let hours = (seconds / SECONDS_PER_HOUR).floor();
        seconds %= SECONDS_PER_HOUR;
        let minutes = (seconds / SECONDS_PER_MINUTE).floor();
        seconds %= SECONDS_PER_MINUTE;
        Ok(DurationParts {
            days,
            hours,
            minutes,
            seconds,
            milliseconds: self.span - (self.span / MS_PER_SECOND).floor() * MS_PER_SECOND,
        })
    }
}
