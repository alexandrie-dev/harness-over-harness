//! Port of `tests/rrule.spec.ts`: RRULE syntax and bounded calendar arithmetic, independent of Schedule task policy.

use hoh_util_time::{
    InstantResult, RRule, RRuleParseOptions, RecurrenceOccurrence, RecurrenceRejection,
    RecurrenceResult, parse_offset_instant,
};

const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

fn date_parse(text: &str) -> i64 {
    match parse_offset_instant(text) {
        InstantResult::Accepted { epoch } => epoch,
        InstantResult::Rejected { reason } => panic!("{text}: {}", reason.as_str()),
    }
}

const DEFAULT_START: &str = "DTSTART:20260916T010000Z";

/// Join a DTSTART line and an RRULE body.
fn rule(body: &str) -> String {
    rule_from(body, DEFAULT_START)
}

fn rule_from(body: &str, start: &str) -> String {
    format!("{start}\nRRULE:{body}")
}

/// List the integers from `from` up to but excluding `to`, separated by commas.
fn numbers(from: i64, to: i64) -> String {
    (from..to)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn now() -> i64 {
    date_parse("2026-09-16T00:00:00.000Z")
}

fn every_minute() -> String {
    format!("BYHOUR={};BYMINUTE={}", numbers(0, 24), numbers(0, 60))
}

const LINES: &str = "rrule must contain one DTSTART line followed by one RRULE line.";
const SPRING_GAP: &str = "DTSTART;TZID=America/New_York:20260308T023000";

/// Parse a valid test rule, preserving the shared utility's own failures.
fn parsed(value: &str) -> RRule {
    match RRule::parse(value, RRuleParseOptions::default()) {
        RecurrenceResult::Accepted { rule } => rule,
        RecurrenceResult::Rejected { detail, .. } => panic!("{detail}"),
    }
}

fn normalized(value: &str) -> String {
    parsed(value).expression().to_owned()
}

fn first_at(value: &str, at: i64) -> Option<String> {
    parsed(value).next(at).unwrap()
}

fn first(value: &str) -> Option<String> {
    first_at(value, now())
}

fn after(value: &str, instant: &str) -> Option<String> {
    first_at(value, date_parse(instant))
}

fn resolve(value: &str, scheduled_at: Option<String>, accepted_at: &str) -> RecurrenceOccurrence {
    let scheduled_at = scheduled_at.expect("RRULE fixture has no committed occurrence");
    parsed(value)
        .resolve(&scheduled_at, date_parse(accepted_at))
        .unwrap()
}

fn due(at: &str, next: &str) -> RecurrenceOccurrence {
    RecurrenceOccurrence {
        occurrence_at: at.to_owned(),
        next_scheduled_at: Some(next.to_owned()),
    }
}

fn last(at: &str) -> RecurrenceOccurrence {
    RecurrenceOccurrence {
        occurrence_at: at.to_owned(),
        next_scheduled_at: None,
    }
}

/// Up to `limit` consecutive occurrences after an instant.
fn occurrences(rrule: &str, instant: &str, limit: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut next = after(rrule, instant);
    while let Some(current) = next {
        if result.len() >= limit {
            break;
        }
        next = after(rrule, &current);
        result.push(current);
    }
    result
}

fn rejection(result: RecurrenceResult<RRule>) -> (RecurrenceRejection, String) {
    match result {
        RecurrenceResult::Rejected { reason, detail, .. } => (reason, detail),
        RecurrenceResult::Accepted { .. } => panic!("expected a rejection"),
    }
}

fn parse_text(text: &str) -> RecurrenceResult<RRule> {
    RRule::parse(text, RRuleParseOptions::default())
}

// RRULE validation

#[test]
fn keeps_the_trimmed_dtstart_and_rrule_lines_and_the_tzid_as_written() {
    let daily = rule("FREQ=DAILY");
    assert_eq!(
        normalized(" DTSTART:20260916T010000Z\r\n\n RRULE:FREQ=DAILY \n"),
        daily
    );
    let eastern = rule_from("FREQ=DAILY", "DTSTART;TZID=US/Eastern:20260916T010000");
    assert_eq!(normalized(&eastern), eastern);
    assert_eq!(first(&eastern).as_deref(), Some("2026-09-16T05:00:00.000Z"));
}

#[test]
fn accepts_the_bounds_of_each_numeric_part() {
    for text in [
        rule("FREQ=DAILY;COUNT=1000"),
        rule("FREQ=DAILY;UNTIL=99991231T235959Z"),
        rule_from("FREQ=DAILY", "DTSTART:01000101T000000Z"),
        rule(&format!("FREQ=DAILY;INTERVAL={MAX_SAFE_INTEGER}")),
        rule("FREQ=YEARLY;BYMONTH=1,12;BYMONTHDAY=-31,31;BYHOUR=0,23;BYMINUTE=0,59;BYSECOND=59"),
        rule("FREQ=YEARLY;BYYEARDAY=-366,366;BYWEEKNO=-53,53;BYDAY=MO"),
        rule("FREQ=YEARLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYSETPOS=-366,366"),
    ] {
        assert_eq!(normalized(&text), text);
    }
    assert_eq!(
        first(&rule("FREQ=YEARLY;BYDAY=53MO")).as_deref(),
        Some("2029-12-31T01:00:00.000Z")
    );
    assert_eq!(
        first(&rule("FREQ=MONTHLY;BYMONTHDAY=+5")).as_deref(),
        Some("2026-10-05T01:00:00.000Z")
    );
    assert_eq!(
        first(&rule("FREQ=MONTHLY;BYMONTHDAY=05")).as_deref(),
        Some("2026-10-05T01:00:00.000Z")
    );
}

#[test]
fn rejects_an_invalid_rule_with_a_syntax_detail() {
    let daily = rule("FREQ=DAILY");
    let dtstart_date = "DTSTART must be a real calendar date and time in years 0100 through 9999.";
    let dtstart_form = "DTSTART must use UTC with Z or a local date-time with an explicit TZID.";
    let fields = "RRULE fields must be non-empty and unique.";
    let supported_parts = "RRULE supports only FREQ, INTERVAL, COUNT, UNTIL, WKST, BYSECOND, BYMINUTE, BYHOUR, BYDAY, \
                           BYMONTHDAY, BYYEARDAY, BYWEEKNO, BYMONTH, BYSETPOS.";
    let freq = "FREQ must be MINUTELY, HOURLY, DAILY, WEEKLY, MONTHLY, or YEARLY.";
    let interval = "INTERVAL must be a positive safe integer.";
    let count =
        "COUNT must be an integer from 1 to 1000; use UNTIL for a longer finite recurrence.";
    let until_date = "UNTIL must be a real calendar date and time in years 0100 through 9999.";
    let by_day = "BYDAY contains an invalid weekday selector.";
    let by_part_hint =
        "does not support BY rule parts; use FREQ=DAILY or FREQ=WEEKLY with BYHOUR and BYMINUTE.";
    let cases: Vec<(String, String)> = vec![
        (String::new(), LINES.to_owned()),
        ("RRULE:FREQ=DAILY".to_owned(), LINES.to_owned()),
        (
            "RRULE:FREQ=DAILY\nDTSTART:20260916T010000Z".to_owned(),
            LINES.to_owned(),
        ),
        (
            "DTSTART:20260916T010000Z\nEXDATE:20260917T010000Z".to_owned(),
            LINES.to_owned(),
        ),
        (format!("{daily}\n{daily}"), LINES.to_owned()),
        (
            rule_from("FREQ=DAILY", "DTSTART:20260916T010000"),
            dtstart_form.to_owned(),
        ),
        (
            "DTSTART;VALUE=DATE:20260916\nRRULE:FREQ=DAILY".to_owned(),
            dtstart_form.to_owned(),
        ),
        (
            rule_from("FREQ=DAILY", "DTSTART:00991231T000000Z"),
            dtstart_date.to_owned(),
        ),
        (
            rule_from("FREQ=DAILY", "DTSTART:20260230T010000Z"),
            dtstart_date.to_owned(),
        ),
        (
            rule_from("FREQ=DAILY", "DTSTART;TZID=UTC:20260916T240000"),
            dtstart_date.to_owned(),
        ),
        (
            rule_from("FREQ=DAILY", SPRING_GAP),
            "DTSTART must be a real local date and time in its time zone.".to_owned(),
        ),
        (
            rule_from("FREQ=DAILY;COUNT=1", SPRING_GAP),
            "DTSTART must be a real local date and time in its time zone.".to_owned(),
        ),
        (rule("NOT_A_RULE"), fields.to_owned()),
        (rule("=DAILY"), fields.to_owned()),
        (rule("FREQ="), fields.to_owned()),
        (rule("FREQ=DAILY;"), fields.to_owned()),
        (rule("FREQ=DAILY;FREQ=WEEKLY"), fields.to_owned()),
        (rule("freq=daily"), supported_parts.to_owned()),
        (rule("FREQ=DAILY;BYEASTER=0"), supported_parts.to_owned()),
        (rule("INTERVAL=2"), "RRULE must contain FREQ.".to_owned()),
        (rule("FREQ=NOTREAL"), freq.to_owned()),
        (rule("FREQ=constructor"), freq.to_owned()),
        (rule("FREQ=DAILY;INTERVAL=0"), interval.to_owned()),
        (rule("FREQ=DAILY;INTERVAL=-1"), interval.to_owned()),
        (
            rule("FREQ=DAILY;INTERVAL=9007199254740992"),
            interval.to_owned(),
        ),
        (rule("FREQ=DAILY;COUNT=0"), count.to_owned()),
        (rule("FREQ=DAILY;COUNT=1001"), count.to_owned()),
        (
            rule("FREQ=DAILY;UNTIL=20260917T010000"),
            "UNTIL must be an absolute UTC date-time ending in Z.".to_owned(),
        ),
        (
            rule("FREQ=DAILY;UNTIL=20260230T010000Z"),
            until_date.to_owned(),
        ),
        (
            rule("FREQ=DAILY;UNTIL=00991231T000000Z"),
            until_date.to_owned(),
        ),
        (
            rule("FREQ=WEEKLY;WKST=XX"),
            "WKST must be MO, TU, WE, TH, FR, SA, or SU.".to_owned(),
        ),
        (
            rule("FREQ=DAILY;BYSECOND=-1"),
            "BYSECOND values must be integers from 0 to 59.".to_owned(),
        ),
        (
            rule("FREQ=DAILY;BYMINUTE=60"),
            "BYMINUTE values must be integers from 0 to 59.".to_owned(),
        ),
        (
            rule("FREQ=DAILY;BYHOUR=9,"),
            "BYHOUR values must be integers from 0 to 23.".to_owned(),
        ),
        (
            rule("FREQ=MONTHLY;BYMONTHDAY=0"),
            "BYMONTHDAY values must be integers from -31 to 31, excluding 0.".to_owned(),
        ),
        (
            rule("FREQ=YEARLY;BYYEARDAY=367"),
            "BYYEARDAY values must be integers from -366 to 366, excluding 0.".to_owned(),
        ),
        (
            rule("FREQ=YEARLY;BYWEEKNO=+54"),
            "BYWEEKNO values must be integers from -53 to 53, excluding 0.".to_owned(),
        ),
        (
            rule("FREQ=YEARLY;BYMONTH=13"),
            "BYMONTH values must be integers from 1 to 12.".to_owned(),
        ),
        (
            rule("FREQ=MONTHLY;BYDAY=MO;BYSETPOS=0"),
            "BYSETPOS values must be integers from -366 to 366, excluding 0.".to_owned(),
        ),
        (
            rule("FREQ=DAILY;BYHOUR=12,12"),
            "BYHOUR values must be unique.".to_owned(),
        ),
        (
            rule("FREQ=MONTHLY;BYMONTHDAY=1,+1"),
            "BYMONTHDAY values must be unique.".to_owned(),
        ),
        (rule("FREQ=DAILY;BYDAY=XX"), by_day.to_owned()),
        (rule("FREQ=MONTHLY;BYDAY=0MO"), by_day.to_owned()),
        (rule("FREQ=YEARLY;BYDAY=54MO"), by_day.to_owned()),
        (
            rule("FREQ=MONTHLY;BYDAY=2TU,+2TU"),
            "BYDAY values must be unique.".to_owned(),
        ),
        (
            rule("FREQ=DAILY;COUNT=2;UNTIL=20261001T000000Z"),
            "RRULE must not contain both COUNT and UNTIL.".to_owned(),
        ),
        (
            rule("FREQ=MINUTELY;BYSECOND=0"),
            format!("FREQ=MINUTELY {by_part_hint}"),
        ),
        (
            rule("FREQ=HOURLY;BYDAY=MO"),
            format!("FREQ=HOURLY {by_part_hint}"),
        ),
        (
            rule("FREQ=MONTHLY;BYWEEKNO=1"),
            "BYWEEKNO requires FREQ=YEARLY.".to_owned(),
        ),
        (
            rule("FREQ=DAILY;BYYEARDAY=1"),
            "BYYEARDAY requires FREQ=YEARLY.".to_owned(),
        ),
        (
            rule("FREQ=WEEKLY;BYMONTHDAY=1"),
            "BYMONTHDAY requires FREQ=MONTHLY or FREQ=YEARLY.".to_owned(),
        ),
        (
            rule("FREQ=WEEKLY;BYDAY=1MO"),
            "BYDAY ordinals require FREQ=MONTHLY or FREQ=YEARLY.".to_owned(),
        ),
        (
            rule("FREQ=YEARLY;BYWEEKNO=1;BYDAY=1MO"),
            "BYDAY ordinals cannot be combined with BYWEEKNO.".to_owned(),
        ),
        (
            rule("FREQ=MONTHLY;BYSETPOS=1"),
            "BYSETPOS requires another BY rule part.".to_owned(),
        ),
        (
            rule(&format!("FREQ=YEARLY;BYDAY=MO;BYSETPOS={}", numbers(1, 33))),
            "BYSETPOS must contain at most 31 values.".to_owned(),
        ),
    ];
    for (text, message) in cases {
        assert_eq!(
            rejection(parse_text(&text)),
            (RecurrenceRejection::Syntax, message),
            "{text:?}"
        );
    }
}

#[test]
fn rejects_a_rule_as_too_frequent() {
    let frequent = "occurrences must be at least 60 seconds apart.";
    let cases: Vec<(String, String)> = vec![
        (rule("FREQ=SECONDLY;INTERVAL=300"), format!("FREQ=SECONDLY is not supported; {frequent}")),
        (rule("FREQ=SECONDLY;BYSECOND=0"), format!("FREQ=SECONDLY is not supported; {frequent}")),
        (rule("FREQ=DAILY;BYSECOND=0,30"), format!("BYSECOND must contain one value; {frequent}")),
        (
            rule(&format!("FREQ=MONTHLY;BYMONTHDAY={};{}", numbers(1, 9), every_minute())),
            "One FREQ period of this RRULE can contain up to 11520 occurrences; the limit is 10080.".to_owned(),
        ),
        (
            rule(&format!("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;{}", every_minute())),
            "One FREQ period of this RRULE can contain up to 44640 occurrences; the limit is 10080.".to_owned(),
        ),
    ];
    for (text, message) in cases {
        assert_eq!(
            rejection(parse_text(&text)),
            (RecurrenceRejection::Density, message),
            "{text:?}"
        );
    }
}

#[test]
fn rejects_the_zone_in_the_dtstart() {
    for start in [
        "DTSTART;TZID=Not/A_Zone:20260916T010000",
        "DTSTART;TZID=EST:20260916T010000",
    ] {
        assert_eq!(
            rejection(parse_text(&rule_from("FREQ=DAILY", start))),
            (
                RecurrenceRejection::TimeZone,
                "TZID must be UTC or a valid IANA Area/Location name.".to_owned()
            ),
            "{start}"
        );
    }
}

#[test]
fn limits_bysetpos_to_the_positions_one_freq_period_can_hold() {
    let cases = [
        ("FREQ=DAILY;BYHOUR=9,17", 2),
        ("FREQ=WEEKLY;BYDAY=MO,WE", 2),
        ("FREQ=MONTHLY;BYDAY=MO", 5),
        ("FREQ=MONTHLY;BYMONTHDAY=1,15;BYDAY=MO", 2),
        ("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=29,30", 2),
        ("FREQ=YEARLY;BYWEEKNO=1", 9),
        ("FREQ=YEARLY;BYWEEKNO=1;BYDAY=MO", 9),
        ("FREQ=YEARLY;BYYEARDAY=1,2", 2),
        ("FREQ=YEARLY;BYMONTHDAY=1", 12),
        ("FREQ=YEARLY;BYDAY=MO", 53),
        ("FREQ=YEARLY;BYMONTH=1,2;BYDAY=MO", 10),
        ("FREQ=YEARLY;BYMONTH=1;BYYEARDAY=1", 1),
        ("FREQ=YEARLY;BYMONTH=1;BYWEEKNO=1", 9),
        ("FREQ=YEARLY;BYMONTH=1", 1),
        ("FREQ=YEARLY;BYHOUR=1", 1),
    ];
    for (body, bound) in cases {
        for position in [bound, -bound] {
            let text = rule(&format!("{body};BYSETPOS={position}"));
            assert!(
                matches!(parse_text(&text), RecurrenceResult::Accepted { .. }),
                "{text:?}"
            );
        }
        assert_eq!(
            rejection(parse_text(&rule(&format!("{body};BYSETPOS={}", bound + 1)))),
            (
                RecurrenceRejection::Syntax,
                format!(
                    "BYSETPOS values must be from 1 to {bound} or from -{bound} to -1; \
                     one FREQ period of this RRULE contains at most {bound} occurrences."
                )
            ),
            "{body}"
        );
    }
}

#[test]
fn accepts_a_freq_period_with_at_most_one_week_of_minutes() {
    let every_minute_week = rule(&format!(
        "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;{}",
        every_minute()
    ));
    assert_eq!(
        after(&every_minute_week, "2026-09-20T12:34:56.000Z").as_deref(),
        Some("2026-09-20T12:35:00.000Z")
    );
    assert_eq!(
        first(&rule(&format!(
            "FREQ=MONTHLY;BYMONTHDAY={};{}",
            numbers(1, 8),
            every_minute()
        )))
        .as_deref(),
        Some("2026-10-01T00:00:00.000Z")
    );
    assert_eq!(
        after(
            &rule(&format!("FREQ=DAILY;{}", every_minute())),
            "2026-09-16T05:06:07.000Z"
        )
        .as_deref(),
        Some("2026-09-16T05:07:00.000Z")
    );
}

// RRULE occurrences

#[test]
fn fills_the_time_and_date_parts_that_a_daily_or_longer_rule_omits_from_dtstart() {
    assert_eq!(
        after(
            &rule_from("FREQ=WEEKLY", "DTSTART:20260916T013000Z"),
            "2027-03-01T00:00:00.000Z"
        )
        .as_deref(),
        Some("2027-03-03T01:30:00.000Z")
    );
    let weekdays = rule("FREQ=WEEKLY;BYDAY=MO,FR");
    assert_eq!(
        after(&weekdays, "2026-09-16T01:00:00.000Z").as_deref(),
        Some("2026-09-18T01:00:00.000Z")
    );
    assert_eq!(
        after(&weekdays, "2026-09-18T01:00:00.000Z").as_deref(),
        Some("2026-09-21T01:00:00.000Z")
    );
    // A month or year without the DTSTART day has no occurrence.
    let month_end = rule_from("FREQ=MONTHLY", "DTSTART:20260131T010000Z");
    assert_eq!(
        first(&month_end).as_deref(),
        Some("2026-10-31T01:00:00.000Z")
    );
    assert_eq!(
        after(&month_end, "2026-01-31T01:00:00.000Z").as_deref(),
        Some("2026-03-31T01:00:00.000Z")
    );
    assert_eq!(
        first(&rule_from("FREQ=YEARLY", "DTSTART:20240229T010000Z")).as_deref(),
        Some("2028-02-29T01:00:00.000Z")
    );
    assert_eq!(
        first(&rule("FREQ=YEARLY;BYMONTH=1,7")).as_deref(),
        Some("2027-01-16T01:00:00.000Z")
    );
}

#[test]
fn enumerates_the_times_of_one_day_in_ascending_order_whatever_the_list_order() {
    let start = "DTSTART:20260101T000000Z";
    assert_eq!(
        after(
            &rule_from("FREQ=DAILY;BYHOUR=18,9;BYMINUTE=0;BYSECOND=0", start),
            "2026-01-01T08:00:00.000Z"
        )
        .as_deref(),
        Some("2026-01-01T09:00:00.000Z")
    );
    assert_eq!(
        after(
            &rule_from("FREQ=DAILY;BYHOUR=9;BYMINUTE=30,10;BYSECOND=0", start),
            "2026-01-01T09:05:00.000Z"
        )
        .as_deref(),
        Some("2026-01-01T09:10:00.000Z")
    );
}

#[test]
fn aligns_interval_periods_to_dtstart_and_wkst_as_in_the_rfc_5545_example() {
    let start = "DTSTART;TZID=America/New_York:19970805T090000";
    let from_monday = [
        "1997-08-05T13:00:00.000Z",
        "1997-08-10T13:00:00.000Z",
        "1997-08-19T13:00:00.000Z",
        "1997-08-24T13:00:00.000Z",
    ];
    let from_sunday = [
        "1997-08-05T13:00:00.000Z",
        "1997-08-17T13:00:00.000Z",
        "1997-08-19T13:00:00.000Z",
        "1997-08-31T13:00:00.000Z",
    ];
    for (week_start, expected) in [("MO", from_monday), ("SU", from_sunday)] {
        let body = format!("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,SU;WKST={week_start}");
        assert_eq!(
            occurrences(
                &rule_from(&format!("{body};COUNT=4"), start),
                "1997-08-05T12:00:00.000Z",
                5
            ),
            expected
        );
        assert_eq!(
            occurrences(&rule_from(&body, start), "1997-08-05T12:00:00.000Z", 4),
            expected
        );
    }
    assert_eq!(
        after(
            &rule_from(
                "FREQ=HOURLY;INTERVAL=5",
                "DTSTART;TZID=Asia/Singapore:20260916T090000"
            ),
            "2026-09-17T00:00:00.000Z"
        )
        .as_deref(),
        Some("2026-09-17T02:00:00.000Z")
    );
    assert_eq!(
        after(&rule("FREQ=DAILY;INTERVAL=3"), "2026-10-01T00:00:00.000Z").as_deref(),
        Some("2026-10-01T01:00:00.000Z")
    );
    let long_cycle = rule("FREQ=YEARLY;INTERVAL=401");
    assert_eq!(
        first(&long_cycle).as_deref(),
        Some("2026-09-16T01:00:00.000Z")
    );
    assert_eq!(
        after(&long_cycle, "2026-09-16T01:00:00.000Z").as_deref(),
        Some("2427-09-16T01:00:00.000Z")
    );
}

#[test]
fn ends_each_freq_after_dtstart_when_interval_is_the_largest_safe_integer() {
    for frequency in ["MINUTELY", "HOURLY", "DAILY", "WEEKLY", "MONTHLY", "YEARLY"] {
        let once = rule(&format!("FREQ={frequency};INTERVAL={MAX_SAFE_INTEGER}"));
        assert_eq!(
            first(&once).as_deref(),
            Some("2026-09-16T01:00:00.000Z"),
            "{frequency}"
        );
        assert_eq!(
            after(&once, "2026-09-16T01:00:00.000Z"),
            None,
            "{frequency}"
        );
    }
}

#[test]
fn enumerates_a_rule_without_count_from_the_freq_period_that_contains_the_reference_time() {
    let minutely = rule_from("FREQ=MINUTELY", "DTSTART:01000101T000000Z");
    assert_eq!(
        first_at(&minutely, date_parse("2026-10-09T00:00:30.000Z")).as_deref(),
        Some("2026-10-09T00:01:00.000Z")
    );
    assert_eq!(
        resolve(
            &minutely,
            Some("2026-10-09T00:01:00.000Z".to_owned()),
            "2026-10-10T00:00:30.000Z"
        ),
        due("2026-10-10T00:00:00.000Z", "2026-10-10T00:01:00.000Z")
    );
    assert_eq!(
        first(&rule_from("FREQ=WEEKLY", "DTSTART:01000101T010000Z")).as_deref(),
        Some("2026-09-18T01:00:00.000Z")
    );
}

#[test]
fn counts_a_sparse_count_rule_from_dtstart() {
    let leap_mondays = rule_from(
        "FREQ=YEARLY;COUNT=1000;BYMONTH=2;BYMONTHDAY=29;BYDAY=MO",
        "DTSTART:01000101T000000Z",
    );
    assert_eq!(
        first(&leap_mondays).as_deref(),
        Some("2044-02-29T00:00:00.000Z")
    );
    assert_eq!(
        resolve(
            &leap_mondays,
            Some("2044-02-29T00:00:00.000Z".to_owned()),
            "2044-03-01T00:00:00.000Z"
        ),
        due("2044-02-29T00:00:00.000Z", "2072-02-29T00:00:00.000Z")
    );
}

#[test]
fn counts_an_overlapping_bysetpos_occurrence_only_once() {
    for positions in ["2,-3", "-3,2"] {
        let counted = rule_from(
            &format!("FREQ=WEEKLY;BYHOUR=2,3;BYDAY=WE,SA;BYSETPOS={positions};COUNT=4"),
            "DTSTART:20260107T020000Z",
        );
        let expected = [
            "2026-01-07T03:00:00.000Z",
            "2026-01-14T03:00:00.000Z",
            "2026-01-21T03:00:00.000Z",
            "2026-01-28T03:00:00.000Z",
        ];
        assert_eq!(
            occurrences(&counted, "2026-01-07T00:00:00.000Z", 5),
            expected,
            "{positions}"
        );
        assert_eq!(
            resolve(
                &counted,
                Some(expected[0].to_owned()),
                "2026-01-21T12:00:00.000Z"
            ),
            due(expected[2], expected[3]),
            "{positions}"
        );
    }
}

#[test]
fn ends_at_an_inclusive_absolute_until_instant() {
    let start = "DTSTART;TZID=Asia/Singapore:20260916T090000";
    let until = rule_from("FREQ=DAILY;UNTIL=20260917T010000Z", start);
    assert_eq!(
        after(&until, "2026-09-16T01:00:00.000Z").as_deref(),
        Some("2026-09-17T01:00:00.000Z")
    );
    assert_eq!(after(&until, "2026-09-17T01:00:00.000Z"), None);
    assert_eq!(
        resolve(&until, first(&until), "2026-09-18T01:00:00.000Z"),
        last("2026-09-17T01:00:00.000Z")
    );
    assert_eq!(
        after(
            &rule_from("FREQ=DAILY;UNTIL=20260917T005959Z", start),
            "2026-09-16T01:00:00.000Z"
        ),
        None
    );
}

#[test]
fn reports_no_future_occurrence() {
    // The second Sunday of March at 02:30 always falls inside the New York forward transition.
    let new_york = "DTSTART;TZID=America/New_York:20260307T023000";
    for text in [
        rule("FREQ=MONTHLY;BYMONTH=2;BYMONTHDAY=30"),
        rule_from(
            "FREQ=MONTHLY;BYMONTH=2;BYMONTHDAY=30",
            "DTSTART:01000101T000000Z",
        ),
        rule("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30"),
        rule_from(
            "FREQ=YEARLY;BYMONTH=3;BYDAY=2SU;BYHOUR=2;BYMINUTE=30",
            new_york,
        ),
        rule_from(
            "FREQ=YEARLY;COUNT=5;BYMONTH=3;BYDAY=2SU;BYHOUR=2;BYMINUTE=30",
            new_york,
        ),
        rule_from("FREQ=DAILY;COUNT=1", "DTSTART:20250916T000100Z"),
    ] {
        assert_eq!(first(&text), None, "{text:?}");
    }
}

#[test]
fn resolves_a_repeated_wall_time_to_its_earlier_instant() {
    let fallback = rule_from(
        "FREQ=DAILY",
        "DTSTART;TZID=America/New_York:20261101T013000",
    );
    assert_eq!(
        first_at(&fallback, date_parse("2026-11-01T00:00:00.000Z")).as_deref(),
        Some("2026-11-01T05:30:00.000Z")
    );
    assert_eq!(
        resolve(
            &fallback,
            Some("2026-11-01T05:30:00.000Z".to_owned()),
            "2026-11-01T06:15:00.000Z"
        ),
        due("2026-11-01T05:30:00.000Z", "2026-11-02T06:30:00.000Z")
    );
    // Wall times from 01:00 through 01:45 occur once, before the clocks move back.
    let quarter_hours = rule_from(
        "FREQ=MINUTELY;INTERVAL=15",
        "DTSTART;TZID=America/New_York:20261101T000000",
    );
    assert_eq!(
        first_at(&quarter_hours, date_parse("2026-11-01T06:15:00.000Z")).as_deref(),
        Some("2026-11-01T07:00:00.000Z")
    );
    for saved in [
        "2026-11-01T05:00:00.000Z",
        "2026-11-01T05:30:00.000Z",
        "2026-11-01T05:45:00.000Z",
    ] {
        assert_eq!(
            resolve(
                &quarter_hours,
                Some(saved.to_owned()),
                "2026-11-01T06:15:00.000Z"
            ),
            due("2026-11-01T05:45:00.000Z", "2026-11-01T07:00:00.000Z"),
            "{saved}"
        );
    }
}

#[test]
fn skips_a_wall_time_inside_a_forward_transition_and_does_not_count_it() {
    let counted = rule_from(
        "FREQ=DAILY;COUNT=3",
        "DTSTART;TZID=America/New_York:20260307T023000",
    );
    let created = first_at(&counted, date_parse("2026-03-07T00:00:00.000Z"));
    assert_eq!(created.as_deref(), Some("2026-03-07T07:30:00.000Z"));
    assert_eq!(
        resolve(&counted, created.clone(), "2026-03-08T12:00:00.000Z"),
        due("2026-03-07T07:30:00.000Z", "2026-03-09T06:30:00.000Z")
    );
    assert_eq!(
        resolve(&counted, created.clone(), "2026-03-10T12:00:00.000Z"),
        last("2026-03-10T06:30:00.000Z")
    );
    assert_eq!(after(&counted, "2026-03-10T06:30:00.000Z"), None);
    let endless = rule_from(
        "FREQ=DAILY",
        "DTSTART;TZID=America/New_York:20260307T023000",
    );
    for accepted_at in ["2026-03-08T05:00:00.000Z", "2026-03-08T12:00:00.000Z"] {
        assert_eq!(
            resolve(&endless, created.clone(), accepted_at),
            due("2026-03-07T07:30:00.000Z", "2026-03-09T06:30:00.000Z"),
            "{accepted_at}"
        );
    }
}

#[test]
fn recurs_only_through_the_last_four_digit_year_instant() {
    for body in ["FREQ=DAILY", "FREQ=DAILY;COUNT=3"] {
        let last_day = rule_from(body, "DTSTART;TZID=America/Los_Angeles:99991230T200000");
        assert_eq!(
            first(&last_day).as_deref(),
            Some("9999-12-31T04:00:00.000Z"),
            "{body}"
        );
        assert_eq!(
            resolve(
                &last_day,
                Some("9999-12-31T04:00:00.000Z".to_owned()),
                "9999-12-31T05:00:00.000Z"
            ),
            last("9999-12-31T04:00:00.000Z"),
            "{body}"
        );
        let beyond = rule_from(body, "DTSTART;TZID=America/Los_Angeles:99991231T200000");
        assert_eq!(first(&beyond), None, "{body}");
    }
    // At this instant, the Kiritimati wall time is already in year 10000.
    let ahead = rule_from(
        "FREQ=DAILY",
        "DTSTART;TZID=Pacific/Kiritimati:20260916T090000",
    );
    assert_eq!(after(&ahead, "9999-12-31T12:00:00.000Z"), None);
}

// RRULE committed occurrence and zone

#[test]
fn finds_the_latest_occurrence_of_a_sparse_rule_across_empty_search_windows() {
    assert_eq!(
        resolve(
            &rule("FREQ=YEARLY"),
            Some("2026-09-16T01:00:00.000Z".to_owned()),
            "2026-12-01T00:00:00.000Z"
        ),
        due("2026-09-16T01:00:00.000Z", "2027-09-16T01:00:00.000Z")
    );
}

#[test]
fn exposes_the_authoritative_dtstart_time_zone() {
    assert_eq!(parsed(&rule("FREQ=DAILY")).time_zone(), "UTC");
    assert_eq!(
        parsed(&rule_from(
            "FREQ=DAILY",
            "DTSTART;TZID=US/Eastern:20260916T010000"
        ))
        .time_zone(),
        "America/New_York"
    );
}

#[test]
fn keeps_a_committed_target_due_when_the_rule_does_not_generate_it() {
    assert_eq!(
        resolve(
            &rule_from("FREQ=DAILY", "DTSTART:20260916T090000Z"),
            Some("2026-09-16T08:00:00.000Z".to_owned()),
            "2026-09-16T08:30:00.000Z"
        ),
        due("2026-09-16T08:00:00.000Z", "2026-09-16T09:00:00.000Z")
    );
    let quarter_hours = rule_from(
        "FREQ=MINUTELY;INTERVAL=15",
        "DTSTART;TZID=America/New_York:20261101T000000",
    );
    assert_eq!(
        resolve(
            &quarter_hours,
            Some("2026-11-01T06:15:00.000Z".to_owned()),
            "2026-11-01T06:20:00.000Z"
        ),
        due("2026-11-01T06:15:00.000Z", "2026-11-01T07:00:00.000Z")
    );
    let later = rule_from("FREQ=DAILY", "DTSTART;TZID=Asia/Singapore:20300916T090000");
    assert_eq!(
        resolve(
            &later,
            Some("2026-09-16T01:00:00.000Z".to_owned()),
            "2026-09-16T01:00:00.000Z"
        ),
        due("2026-09-16T01:00:00.000Z", "2030-09-16T01:00:00.000Z")
    );
}
