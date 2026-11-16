//! Port of `tests/instant.spec.ts`.

use hoh_util_time::{
    InstantRejection, InstantResult, LocalCalendarRejection, LocalCalendarResult,
    LocalCalendarTime, MAX_FOUR_DIGIT_YEAR_MS, MIN_FOUR_DIGIT_YEAR_MS, TimeError, TimeZoneCause,
    TimeZoneRejection, TimeZoneResult, canonicalize_time_zone, format_four_digit_year_instant,
    four_digit_year_epoch, is_four_digit_year_instant, parse_local_calendar, parse_offset_instant,
    resolve_local_instant,
};

/// `Date.parse` of an ISO instant with an explicit `Z`.
fn date_parse(text: &str) -> i64 {
    match parse_offset_instant(text) {
        InstantResult::Accepted { epoch } => epoch,
        InstantResult::Rejected { reason } => panic!("{text}: {}", reason.as_str()),
    }
}

fn time(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millisecond: u32,
) -> LocalCalendarTime {
    LocalCalendarTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        millisecond,
    }
}

fn accepted_local(date: &str, clock: &str) -> LocalCalendarResult {
    parse_local_calendar(date, clock)
}

fn rejected_local(reason: LocalCalendarRejection) -> LocalCalendarResult {
    LocalCalendarResult::Rejected { reason }
}

fn rejected(reason: InstantRejection) -> InstantResult {
    InstantResult::Rejected { reason }
}

fn epoch(value: i64) -> InstantResult {
    InstantResult::Accepted { epoch: value }
}

// parseLocalCalendar

#[test]
fn parse_local_calendar_reads_exact_calendar_fields_and_normalizes_an_absent_fraction_to_zero() {
    assert_eq!(
        accepted_local("2026-03-08", "02:30:00"),
        LocalCalendarResult::Accepted {
            time: time(2026, 3, 8, 2, 30, 0, 0)
        }
    );
}

#[test]
fn parse_local_calendar_pads_a_one_or_two_digit_fraction_to_milliseconds() {
    assert_eq!(
        accepted_local("2026-03-08", "02:30:00.5"),
        LocalCalendarResult::Accepted {
            time: time(2026, 3, 8, 2, 30, 0, 500)
        }
    );
    assert_eq!(
        accepted_local("2026-03-08", "02:30:00.25"),
        LocalCalendarResult::Accepted {
            time: time(2026, 3, 8, 2, 30, 0, 250)
        }
    );
}

#[test]
fn parse_local_calendar_distinguishes_malformed_text_from_an_out_of_range_field() {
    assert_eq!(
        accepted_local("2026-3-08", "02:30:00"),
        rejected_local(LocalCalendarRejection::Malformed)
    );
    assert_eq!(
        accepted_local("2026-03-08", "2:30:00"),
        rejected_local(LocalCalendarRejection::Malformed)
    );
    assert_eq!(
        accepted_local("2026-03-08", "02:30:00.1234"),
        rejected_local(LocalCalendarRejection::Malformed)
    );
    assert_eq!(
        accepted_local("0000-03-08", "02:30:00"),
        rejected_local(LocalCalendarRejection::OutOfRange)
    );
    assert_eq!(
        accepted_local("2026-03-08", "24:00:00"),
        rejected_local(LocalCalendarRejection::OutOfRange)
    );
    assert_eq!(
        accepted_local("2026-03-08", "02:60:00"),
        rejected_local(LocalCalendarRejection::OutOfRange)
    );
    assert_eq!(
        accepted_local("2026-03-08", "02:30:60"),
        rejected_local(LocalCalendarRejection::OutOfRange)
    );
}

#[test]
fn parse_local_calendar_rejects_a_date_the_calendar_does_not_have() {
    assert_eq!(
        accepted_local("2026-02-30", "02:30:00"),
        rejected_local(LocalCalendarRejection::NotACalendarDate)
    );
    assert_eq!(
        accepted_local("2025-02-29", "02:30:00"),
        rejected_local(LocalCalendarRejection::NotACalendarDate)
    );
    assert!(matches!(
        accepted_local("2024-02-29", "02:30:00"),
        LocalCalendarResult::Accepted { .. }
    ));
}

// parseOffsetInstant

#[test]
fn parse_offset_instant_reads_a_z_instant_as_its_utc_epoch() {
    assert_eq!(
        parse_offset_instant("2026-03-08T02:30:00Z"),
        epoch(date_parse("2026-03-08T02:30:00.000Z"))
    );
}

#[test]
fn parse_offset_instant_subtracts_a_numeric_offset_from_the_written_wall_clock_value() {
    let wall_clock = date_parse("2026-03-08T02:30:00.000Z");
    assert_eq!(
        parse_offset_instant("2026-03-08T02:30:00+08:00"),
        epoch(wall_clock - 8 * 3_600_000)
    );
    assert_eq!(
        parse_offset_instant("2026-03-08T02:30:00-05:30"),
        epoch(wall_clock + 5 * 3_600_000 + 1_800_000)
    );
}

#[test]
fn parse_offset_instant_normalizes_an_optional_one_to_three_digit_fraction_to_milliseconds() {
    assert_eq!(
        parse_offset_instant("2026-03-08T02:30:00.5Z"),
        epoch(date_parse("2026-03-08T02:30:00.500Z"))
    );
    assert_eq!(
        parse_offset_instant("2026-03-08T02:30:00.25Z"),
        epoch(date_parse("2026-03-08T02:30:00.250Z"))
    );
}

#[test]
fn parse_offset_instant_rejects_text_that_is_not_the_accepted_rfc3339_profile() {
    for value in [
        "2026-03-08T02:30:00",
        "2026-03-08 02:30:00Z",
        "2026-03-08T02:30Z",
        "2026-03-08T02:30:00.1234Z",
        "2026-3-08T02:30:00Z",
    ] {
        assert_eq!(
            parse_offset_instant(value),
            rejected(InstantRejection::NotRfc3339),
            "{value}"
        );
    }
}

#[test]
fn parse_offset_instant_rejects_a_clock_field_a_zero_year_and_a_non_existent_calendar_date() {
    for value in [
        "0000-03-08T02:30:00Z",
        "2026-03-08T24:30:00Z",
        "2026-03-08T02:60:00Z",
        "2026-03-08T02:30:60Z",
    ] {
        assert_eq!(
            parse_offset_instant(value),
            rejected(InstantRejection::OutOfRange),
            "{value}"
        );
    }
    assert_eq!(
        parse_offset_instant("2026-02-30T02:30:00Z"),
        rejected(InstantRejection::NotACalendarDate)
    );
}

#[test]
fn parse_offset_instant_rejects_an_unusable_numeric_offset() {
    for value in [
        "2026-03-08T02:30:00+24:00",
        "2026-03-08T02:30:00+08:60",
        "2026-03-08T02:30:00-00:00",
    ] {
        assert_eq!(
            parse_offset_instant(value),
            rejected(InstantRejection::InvalidNumericOffset),
            "{value}"
        );
    }
    assert_eq!(
        parse_offset_instant("2026-03-08T02:30:00+00:00"),
        epoch(date_parse("2026-03-08T02:30:00.000Z"))
    );
}

// fourDigitYearEpoch

#[test]
fn four_digit_year_epoch_accepts_the_canonical_profile_and_answers_its_epoch() {
    assert_eq!(
        four_digit_year_epoch("2026-03-08T02:30:00.000Z"),
        epoch(date_parse("2026-03-08T02:30:00.000Z"))
    );
    assert_eq!(
        four_digit_year_epoch("0001-01-01T00:00:00.000Z"),
        epoch(MIN_FOUR_DIGIT_YEAR_MS)
    );
    assert_eq!(
        four_digit_year_epoch("9999-12-31T23:59:59.999Z"),
        epoch(MAX_FOUR_DIGIT_YEAR_MS)
    );
}

#[test]
fn four_digit_year_epoch_rejects_everything_outside_the_canonical_profile() {
    for value in [
        "2026-03-08T02:30:00Z",
        "2026-03-08T02:30:00.000+00:00",
        "2026-03-08T02:30:00.00Z",
        "0000-01-01T00:00:00.000Z",
        "2026-3-08T02:30:00.000Z",
    ] {
        assert_eq!(
            four_digit_year_epoch(value),
            rejected(InstantRejection::NotRfc3339),
            "{value}"
        );
    }
}

#[test]
fn four_digit_year_epoch_rejects_a_profile_shaped_text_whose_date_does_not_exist() {
    assert_eq!(
        four_digit_year_epoch("2026-02-30T02:30:00.000Z"),
        rejected(InstantRejection::NotACalendarDate)
    );
}

// isFourDigitYearInstant

#[test]
fn is_four_digit_year_instant_answers_the_range_including_both_bounds_and_excludes_everything_else()
{
    assert!(is_four_digit_year_instant(MIN_FOUR_DIGIT_YEAR_MS));
    assert!(is_four_digit_year_instant(MAX_FOUR_DIGIT_YEAR_MS));
    assert!(is_four_digit_year_instant(date_parse(
        "2026-03-08T02:30:00.000Z"
    )));
    // The non-integral and NaN epochs of the source cannot be written as `i64`.
    for value in [MIN_FOUR_DIGIT_YEAR_MS - 1, MAX_FOUR_DIGIT_YEAR_MS + 1] {
        assert!(!is_four_digit_year_instant(value));
    }
}

#[test]
fn bounds_are_the_first_and_last_millisecond_of_the_four_digit_years() {
    assert_eq!(
        MIN_FOUR_DIGIT_YEAR_MS,
        date_parse("0001-01-01T00:00:00.000Z")
    );
    assert_eq!(
        MAX_FOUR_DIGIT_YEAR_MS,
        date_parse("9999-12-31T23:59:59.999Z")
    );
}

// formatFourDigitYearInstant

#[test]
fn format_four_digit_year_instant_formats_both_inclusive_bounds_in_the_canonical_profile() {
    assert_eq!(
        format_four_digit_year_instant(MIN_FOUR_DIGIT_YEAR_MS).as_deref(),
        Some("0001-01-01T00:00:00.000Z")
    );
    assert_eq!(
        format_four_digit_year_instant(MAX_FOUR_DIGIT_YEAR_MS).as_deref(),
        Some("9999-12-31T23:59:59.999Z")
    );
}

#[test]
fn format_four_digit_year_instant_refuses_an_out_of_range_epoch() {
    // The non-integral and NaN epochs of the source cannot be written as `i64`.
    for value in [MIN_FOUR_DIGIT_YEAR_MS - 1, MAX_FOUR_DIGIT_YEAR_MS + 1] {
        assert_eq!(format_four_digit_year_instant(value), None);
    }
}

// canonicalizeTimeZone

fn zone_accepted(name: &str) -> TimeZoneResult {
    TimeZoneResult::Accepted {
        time_zone: name.to_owned(),
    }
}

fn zone_rejected(reason: TimeZoneRejection) -> TimeZoneResult {
    TimeZoneResult::Rejected {
        reason,
        cause: None,
    }
}

#[test]
fn canonicalize_time_zone_answers_the_runtime_canonical_name() {
    assert_eq!(canonicalize_time_zone("UTC"), zone_accepted("UTC"));
    assert_eq!(
        canonicalize_time_zone("US/Eastern"),
        zone_accepted("America/New_York")
    );
    assert_eq!(
        canonicalize_time_zone("America/New_York"),
        zone_accepted("America/New_York")
    );
}

#[test]
fn canonicalize_time_zone_reports_why_a_name_is_refused() {
    assert_eq!(
        canonicalize_time_zone(""),
        zone_rejected(TimeZoneRejection::Blank)
    );
    assert_eq!(
        canonicalize_time_zone(" UTC"),
        zone_rejected(TimeZoneRejection::Padded)
    );
    assert_eq!(
        canonicalize_time_zone("UTC "),
        zone_rejected(TimeZoneRejection::Padded)
    );
    assert_eq!(
        canonicalize_time_zone("Asia"),
        zone_rejected(TimeZoneRejection::NotAreaLocation)
    );
    assert_eq!(
        canonicalize_time_zone("GMT+8"),
        zone_rejected(TimeZoneRejection::NotAreaLocation)
    );
    assert_eq!(
        canonicalize_time_zone("utc"),
        zone_rejected(TimeZoneRejection::NotAreaLocation)
    );
    let TimeZoneResult::Rejected { reason, cause } = canonicalize_time_zone("Not/A_Real_Zone")
    else {
        panic!("expected a rejection");
    };
    assert_eq!(reason, TimeZoneRejection::Unsupported);
    // The platform refusal is the caller's only evidence of why a well-formed name failed, so the rejection carries it.
    assert_eq!(
        cause,
        Some(TimeZoneCause::RangeError {
            message: "Invalid time zone specified: Not/A_Real_Zone".to_owned()
        })
    );
    assert_eq!(
        canonicalize_time_zone("Asia"),
        TimeZoneResult::Rejected {
            reason: TimeZoneRejection::NotAreaLocation,
            cause: None
        }
    );
}

#[test]
fn canonicalize_time_zone_answers_the_canonical_spelling_not_the_callers() {
    for (asked, canonical) in [
        ("america/new_york", "America/New_York"),
        ("EUROPE/MOSCOW", "Europe/Moscow"),
        ("us/eastern", "America/New_York"),
        ("Europe/MOSCOW", "Europe/Moscow"),
    ] {
        assert_eq!(
            canonicalize_time_zone(asked),
            zone_accepted(canonical),
            "{asked}"
        );
    }
    // `W-SU` is an alias of Europe/Moscow, but a name without a slash is refused by shape, as in the source.
    assert_eq!(
        canonicalize_time_zone("w-su"),
        zone_rejected(TimeZoneRejection::NotAreaLocation)
    );
}

#[test]
fn canonicalize_time_zone_answers_utc_for_the_utc_aliases_and_keeps_fixed_offset_names() {
    for asked in [
        "Etc/UTC",
        "Etc/GMT",
        "Etc/UCT",
        "Etc/Zulu",
        "Etc/Greenwich",
        "Etc/Universal",
        "Etc/GMT0",
        "Etc/GMT+0",
        "Etc/GMT-0",
    ] {
        assert_eq!(
            canonicalize_time_zone(asked),
            zone_accepted("UTC"),
            "{asked}"
        );
    }
    for kept in ["Etc/GMT-1", "Etc/GMT+5"] {
        assert_eq!(canonicalize_time_zone(kept), zone_accepted(kept), "{kept}");
    }
}

#[test]
fn canonicalize_time_zone_refuses_the_special_unknown_zone() {
    for asked in ["Etc/Unknown", "etc/unknown"] {
        assert!(
            matches!(
                canonicalize_time_zone(asked),
                TimeZoneResult::Rejected {
                    reason: TimeZoneRejection::Unsupported,
                    cause: Some(_)
                }
            ),
            "{asked}"
        );
    }
}

// resolveLocalInstant

fn local(hour: u32, minute: u32) -> LocalCalendarTime {
    time(2026, 3, 8, hour, minute, 0, 0)
}

#[test]
fn resolve_local_instant_resolves_a_wall_clock_value_that_exists_exactly_once() {
    assert_eq!(
        resolve_local_instant(&local(12, 0), "America/New_York"),
        Ok(Some(date_parse("2026-03-08T16:00:00.000Z")))
    );
}

#[test]
fn resolve_local_instant_answers_none_for_a_forward_transition_gap() {
    // 2026-03-08T02:30 local does not exist in America/New_York.
    assert_eq!(
        resolve_local_instant(&local(2, 30), "America/New_York"),
        Ok(None)
    );
}

#[test]
fn resolve_local_instant_selects_the_earlier_instant_of_a_backward_transition_overlap() {
    // 2026-11-01T01:30 local occurs twice in America/New_York.
    let overlap = LocalCalendarTime {
        month: 11,
        day: 1,
        ..local(1, 30)
    };
    assert_eq!(
        resolve_local_instant(&overlap, "America/New_York"),
        Ok(Some(date_parse("2026-11-01T05:30:00.000Z")))
    );
}

#[test]
fn resolve_local_instant_resolves_utc_without_a_transition() {
    assert_eq!(
        resolve_local_instant(&local(2, 30), "UTC"),
        Ok(Some(date_parse("2026-03-08T02:30:00.000Z")))
    );
}

#[test]
fn resolve_local_instant_accepts_calendar_fields_in_place_of_a_plain_date_time() {
    // The `Temporal.PlainDateTime` overload has no Rust counterpart; `LocalCalendarTime` carries the same fields.
    assert_eq!(
        resolve_local_instant(&local(12, 0), "America/New_York"),
        Ok(Some(date_parse("2026-03-08T16:00:00.000Z")))
    );
    assert_eq!(
        resolve_local_instant(&local(2, 30), "America/New_York"),
        Ok(None)
    );
}

#[test]
fn resolve_local_instant_raises_a_range_error_for_an_unknown_zone_or_a_missing_date() {
    assert!(matches!(
        resolve_local_instant(&local(12, 0), "Not/A_Zone"),
        Err(TimeError::Range(_))
    ));
    assert!(matches!(
        resolve_local_instant(&time(2026, 2, 30, 0, 0, 0, 0), "UTC"),
        Err(TimeError::Range(_))
    ));
}
