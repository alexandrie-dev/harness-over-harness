//! Proleptic Gregorian arithmetic on integer days and milliseconds, the calendar JavaScript `Date` and `Temporal` use.

/// Milliseconds in one day.
pub(crate) const MS_PER_DAY: i64 = 86_400_000;

/// Whether a year is a Gregorian leap year.
pub(crate) fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Number of days in a month (1 through 12).
pub(crate) fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(year) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 of a civil date; the month may be any integer and is normalized.
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year + (month - 1).div_euclid(12);
    let month = (month - 1).rem_euclid(12) + 1;
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil `(year, month, day)` of a day count since 1970-01-01.
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Weekday of a day count with Sunday as 0.
pub(crate) fn weekday_sunday0(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

/// Weekday of a day count with Monday as 0.
pub(crate) fn weekday_monday0(days: i64) -> i64 {
    (days + 3).rem_euclid(7)
}

/// Epoch milliseconds of civil fields read as UTC.
pub(crate) fn epoch_ms(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    milli: i64,
) -> i64 {
    days_from_civil(year, month, day) * MS_PER_DAY
        + ((hour * 60 + minute) * 60 + second) * 1_000
        + milli
}

/// Civil fields `(year, month, day, hour, minute, second, millisecond)`.
pub(crate) type Fields = (i64, i64, i64, i64, i64, i64, i64);

/// Civil fields of epoch milliseconds read as UTC.
pub(crate) fn fields_of(ms: i64) -> Fields {
    let days = ms.div_euclid(MS_PER_DAY);
    let rest = ms.rem_euclid(MS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    (
        year,
        month,
        day,
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000,
    )
}

/// `Date.prototype.toISOString` for years 0000 through 9999.
pub(crate) fn iso_string(ms: i64) -> String {
    let (year, month, day, hour, minute, second, milli) = fields_of(ms);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z")
}

/// JavaScript `trim` whitespace test (differs from `char::is_whitespace`).
pub(crate) fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}'
            | '\u{a}'
            | '\u{b}'
            | '\u{c}'
            | '\u{d}'
            | '\u{20}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// JavaScript `String.prototype.trim`.
pub(crate) fn js_trim(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

/// Digits of an ASCII-only decimal string as a saturating number; `None` when it is not all ASCII digits.
pub(crate) fn digits_value(text: &str) -> Option<i128> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut value: i128 = 0;
    for byte in text.bytes() {
        value = value
            .saturating_mul(10)
            .saturating_add(i128::from(byte - b'0'))
            .min(1_000_000_000_000_000_000_000_000);
    }
    Some(value)
}
