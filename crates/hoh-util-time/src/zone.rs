//! IANA zone lookup and wall-clock resolution on top of `jiff`, with the `Temporal` choices of the source.

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::{AmbiguousOffset, TimeZone};

use crate::calendar::{Fields, epoch_ms, fields_of};

/// A resolved zone.
#[derive(Debug, Clone)]
pub(crate) struct Zone {
    tz: TimeZone,
}

impl Zone {
    /// Look a canonical zone name up in the platform's zone data.
    pub(crate) fn get(name: &str) -> Result<Zone, String> {
        TimeZone::get(name)
            .map(|tz| Zone { tz })
            .map_err(|error| error.to_string())
    }

    /// Offset from UTC in milliseconds at an instant; instants beyond the range `jiff` holds use its nearest bound.
    pub(crate) fn offset_ms_at(&self, epoch: i64) -> i64 {
        let timestamp = Timestamp::from_millisecond(epoch).unwrap_or(if epoch < 0 {
            Timestamp::MIN
        } else {
            Timestamp::MAX
        });
        i64::from(self.tz.to_offset(timestamp).seconds()) * 1_000
    }

    /// Resolve civil fields read in this zone to an instant: the earlier instant of an overlap, `None` inside a gap.
    /// `Err` when the fields are not a date `jiff` can hold.
    pub(crate) fn resolve(&self, fields: Fields) -> Result<Option<i64>, String> {
        let (year, month, day, hour, minute, second, milli) = fields;
        let narrow = |value: i64| i8::try_from(value).map_err(|error| error.to_string());
        let civil = DateTime::new(
            i16::try_from(year).map_err(|error| error.to_string())?,
            narrow(month)?,
            narrow(day)?,
            narrow(hour)?,
            narrow(minute)?,
            narrow(second)?,
            i32::try_from(milli * 1_000_000).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let local = epoch_ms(year, month, day, hour, minute, second, milli);
        Ok(match self.tz.to_ambiguous_timestamp(civil).offset() {
            AmbiguousOffset::Unambiguous { offset } => {
                Some(local - i64::from(offset.seconds()) * 1_000)
            }
            AmbiguousOffset::Fold { before, .. } => {
                Some(local - i64::from(before.seconds()) * 1_000)
            }
            AmbiguousOffset::Gap { .. } => None,
        })
    }

    /// Resolve the UTC-shaped wall time `wall` (milliseconds) as local civil fields.
    pub(crate) fn resolve_wall(&self, wall: i64) -> Result<Option<i64>, String> {
        self.resolve(fields_of(wall))
    }
}
