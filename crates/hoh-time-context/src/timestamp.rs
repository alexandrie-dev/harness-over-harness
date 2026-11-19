//! ISO-shaped timestamp formatting for durable time-context readings.

use hoh_error::BoxError;
use hoh_util_time::{TimeZoneResult, canonicalize_time_zone};
use jiff::{Timestamp, tz::TimeZone};

use crate::TimeContextError;

/// Resolved formatter used by the plugin and replay validation.
#[derive(Clone, Debug)]
pub struct TimestampFormatter {
    zone: TimeZone,
    time_zone: String,
}

impl TimestampFormatter {
    /// Canonical zone label returned by the platform resolver.
    pub fn time_zone(&self) -> &str {
        &self.time_zone
    }
}

/// Create the exact zoned formatter used by durable readings.
pub fn create_timestamp_formatter(time_zone: Option<&str>) -> Result<TimestampFormatter, BoxError> {
    let (zone, time_zone) = match time_zone {
        Some(value) => {
            let zone = match TimeZone::get(value) {
                Ok(zone) => zone,
                Err(direct_error) => {
                    let TimeZoneResult::Accepted { time_zone } = canonicalize_time_zone(value)
                    else {
                        return Err(Box::new(TimeContextError::RangeError(
                            direct_error.to_string(),
                        )));
                    };
                    TimeZone::get(&time_zone).map_err(|error| {
                        Box::new(TimeContextError::RangeError(error.to_string())) as BoxError
                    })?
                }
            };
            let time_zone = zone
                .primary_iana_name()
                .or_else(|| zone.iana_name())
                .ok_or_else(|| {
                    Box::new(TimeContextError::RangeError(format!(
                        "time zone {value} has no IANA name"
                    ))) as BoxError
                })?
                .to_owned();
            (zone, time_zone)
        }
        None => {
            let zone = TimeZone::try_system().map_err(|error| {
                Box::new(TimeContextError::RangeError(error.to_string())) as BoxError
            })?;
            let time_zone = zone
                .primary_iana_name()
                .or_else(|| zone.iana_name())
                .ok_or_else(|| {
                    Box::new(TimeContextError::RangeError(
                        "system time zone has no IANA name".into(),
                    )) as BoxError
                })?
                .to_owned();
            (zone, time_zone)
        }
    };
    Ok(TimestampFormatter { zone, time_zone })
}

/// Format epoch milliseconds as `YYYY-MM-DDTHH:mm:ss±HH:mm[Zone]`.
pub fn format_timestamp(
    now: i64,
    formatter: &TimestampFormatter,
    time_zone: &str,
) -> Result<String, BoxError> {
    let timestamp = Timestamp::from_millisecond(now)?;
    let local = timestamp.to_zoned_datetime(formatter.zone.clone());
    let offset = formatter.zone.to_offset(timestamp).seconds();
    let sign = if offset < 0 { '-' } else { '+' };
    let absolute = offset.unsigned_abs();
    let hours = absolute / 3_600;
    let minutes = (absolute % 3_600) / 60;
    let seconds = absolute % 60;
    let offset = if seconds == 0 {
        format!("{sign}{hours:02}:{minutes:02}")
    } else {
        format!("{sign}{hours:02}:{minutes:02}:{seconds:02}")
    };
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{offset}[{time_zone}]",
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
    ))
}
