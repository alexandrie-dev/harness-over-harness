//! Port of `tests/recurrence.spec.ts`: consumer-independent Cron and RRULE values share strict parsing and bounded
//! occurrence behavior.

use hoh_util_time::{
    Cron, InstantResult, MAX_FOUR_DIGIT_YEAR_MS, MIN_FOUR_DIGIT_YEAR_MS, RRule, RRuleParseOptions,
    RecurrenceOccurrence, RecurrenceRejection, RecurrenceResult, TimeError, parse_offset_instant,
};

fn date_parse(text: &str) -> i64 {
    match parse_offset_instant(text) {
        InstantResult::Accepted { epoch } => epoch,
        InstantResult::Rejected { reason } => panic!("{text}: {}", reason.as_str()),
    }
}

fn accepted<T>(result: RecurrenceResult<T>) -> T {
    match result {
        RecurrenceResult::Accepted { rule } => rule,
        RecurrenceResult::Rejected { detail, .. } => panic!("{detail}"),
    }
}

fn rrule(text: &str) -> RRule {
    accepted(RRule::parse(text, RRuleParseOptions::default()))
}

/// The two recurrence values answer the same two operations.
enum Rule {
    Cron(Cron),
    RRule(RRule),
}

impl Rule {
    fn next(&self, after: i64) -> Result<Option<String>, TimeError> {
        match self {
            Rule::Cron(rule) => rule.next(after),
            Rule::RRule(rule) => rule.next(after),
        }
    }

    fn resolve(
        &self,
        scheduled_at: &str,
        accepted_at: i64,
    ) -> Result<RecurrenceOccurrence, TimeError> {
        match self {
            Rule::Cron(rule) => rule.resolve(scheduled_at, accepted_at),
            Rule::RRule(rule) => rule.resolve(scheduled_at, accepted_at),
        }
    }
}

fn occurrence(at: &str, next: &str) -> RecurrenceOccurrence {
    RecurrenceOccurrence {
        occurrence_at: at.to_owned(),
        next_scheduled_at: Some(next.to_owned()),
    }
}

// The source's `preserves unexpected parser failures instead of reporting invalid user input` has no counterpart:
// a Rust parser closure returns its refusal as a value and has no unexpected exception to preserve.

#[test]
fn keeps_cron_expression_and_zone_distinct_from_rrule_content() {
    let cron = accepted(Cron::parse("0 9 1 * 1", "UTC"));
    let rrule = rrule("DTSTART:20261001T090000Z\nRRULE:FREQ=MONTHLY");
    assert_eq!(cron.expression(), "0 9 1 * 1");
    assert_eq!(cron.time_zone(), "UTC");
    assert_eq!(cron.fields().days_of_month, [1]);
    assert_eq!(cron.fields().days_of_week, [1]);
    assert!(!cron.fields().day_of_month_star);
    assert!(rrule.expression().contains("RRULE:FREQ=MONTHLY"));
    let after = date_parse("2026-10-02T00:00:00Z");
    assert_eq!(
        cron.next(after).unwrap().as_deref(),
        Some("2026-10-05T09:00:00.000Z")
    );
    assert_eq!(
        rrule.next(after).unwrap().as_deref(),
        Some("2026-11-01T09:00:00.000Z")
    );
}

#[test]
fn keeps_discontinuous_minute_groups_through_canonicalization_and_occurrence_searches() {
    let rule = accepted(Cron::parse("8,0,1,4,7 9 * * *", "UTC"));
    assert_eq!(rule.expression(), "0-1,4,7-8 9 * * *");
    assert_eq!(rule.fields().minutes, [0, 1, 4, 7, 8]);
    assert_eq!(
        rule.next(date_parse("2026-10-01T09:04:00.001Z"))
            .unwrap()
            .as_deref(),
        Some("2026-10-01T09:07:00.000Z")
    );
    assert_eq!(
        rule.resolve(
            "2026-10-01T09:00:00.000Z",
            date_parse("2026-10-01T09:08:00.001Z")
        )
        .unwrap(),
        occurrence("2026-10-01T09:08:00.000Z", "2026-10-02T09:00:00.000Z")
    );
}

#[test]
fn skips_a_missing_local_time_without_changing_the_explicit_zone() {
    let rules = [
        Rule::Cron(accepted(Cron::parse("30 2 * * *", "America/New_York"))),
        Rule::RRule(rrule(
            "DTSTART;TZID=America/New_York:20260307T023000\nRRULE:FREQ=DAILY",
        )),
    ];
    for rule in rules {
        assert_eq!(
            rule.next(date_parse("2026-03-08T00:00:00Z"))
                .unwrap()
                .as_deref(),
            Some("2026-03-09T06:30:00.000Z")
        );
    }
}

#[test]
fn does_not_repeat_an_overlapping_wall_clock_occurrence() {
    let rules = [
        Rule::Cron(accepted(Cron::parse("0 1 * * *", "America/New_York"))),
        Rule::RRule(rrule(
            "DTSTART;TZID=America/New_York:20261031T010000\nRRULE:FREQ=DAILY",
        )),
    ];
    for rule in rules {
        assert_eq!(
            rule.next(date_parse("2026-11-01T05:30:00Z"))
                .unwrap()
                .as_deref(),
            Some("2026-11-02T06:00:00.000Z")
        );
    }
}

fn every_minute_rules() -> [Rule; 2] {
    [
        Rule::Cron(accepted(Cron::parse("* * * * *", "UTC"))),
        Rule::RRule(rrule("DTSTART:20260101T000000Z\nRRULE:FREQ=MINUTELY")),
    ]
}

#[test]
fn coalesces_missed_occurrences_from_an_explicit_committed_lower_bound() {
    for rule in every_minute_rules() {
        assert_eq!(
            rule.resolve(
                "2026-01-01T00:00:00.000Z",
                date_parse("2026-10-01T01:02:03Z")
            )
            .unwrap(),
            occurrence("2026-10-01T01:02:00.000Z", "2026-10-01T01:03:00.000Z")
        );
        // The source also tries NaN, which an `i64` cannot hold.
        for after in [MIN_FOUR_DIGIT_YEAR_MS - 1, MAX_FOUR_DIGIT_YEAR_MS + 1] {
            assert!(
                matches!(rule.next(after), Err(TimeError::Range(_))),
                "{after}"
            );
        }
        assert!(matches!(
            rule.resolve("2026-01-01T00:00:00.000Z", 0),
            Err(TimeError::Range(_))
        ));
    }
}

#[test]
fn refuses_a_committed_lower_bound_outside_the_canonical_stored_instant_profile() {
    let decision = date_parse("2026-03-03T00:00:00.000Z");
    for rule in every_minute_rules() {
        for scheduled_at in [
            "2026-02-30T00:00:00.000Z",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00.0000Z",
            "2026-01-01T08:00:00.000+08:00",
            "2026-01-01",
            "0000-01-01T00:00:00.000Z",
            "not-an-instant",
        ] {
            assert!(
                matches!(
                    rule.resolve(scheduled_at, decision),
                    Err(TimeError::Range(_))
                ),
                "{scheduled_at}"
            );
        }
    }
}

#[test]
fn reports_parser_rejections_without_a_consumer_error_code() {
    // The source also asserts the result has no `code` property; `RecurrenceResult::Rejected` has no such field.
    fn reason<T>(result: RecurrenceResult<T>) -> RecurrenceRejection {
        match result {
            RecurrenceResult::Rejected { reason, .. } => reason,
            RecurrenceResult::Accepted { .. } => panic!("expected a rejection"),
        }
    }
    let options = RRuleParseOptions::default();
    assert_eq!(
        reason(Cron::parse("* * * * * *", "UTC")),
        RecurrenceRejection::Syntax
    );
    assert_eq!(
        reason(Cron::parse("* * * * *", "Not/A_Zone")),
        RecurrenceRejection::TimeZone
    );
    assert_eq!(
        reason(RRule::parse("RRULE:FREQ=DAILY", options)),
        RecurrenceRejection::Syntax
    );
    assert_eq!(
        reason(RRule::parse(
            "DTSTART:20260916T000000Z\nRRULE:FREQ=SECONDLY",
            options
        )),
        RecurrenceRejection::Density
    );
    assert_eq!(
        reason(RRule::parse(
            "DTSTART:20260916T000000Z\nRRULE:FREQ=DAILY;BYSECOND=0,30",
            options
        )),
        RecurrenceRejection::Density
    );
}

#[test]
fn permits_a_stricter_authoring_profile_without_changing_valid_rfc_arithmetic() {
    let text = "DTSTART:20260916T090000Z\nRRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1";
    assert!(matches!(
        RRule::parse(text, RRuleParseOptions::default()),
        RecurrenceResult::Accepted { .. }
    ));
    assert!(matches!(
        RRule::parse(
            text,
            RRuleParseOptions {
                reject_ineffective_set_positions: true
            }
        ),
        RecurrenceResult::Rejected {
            reason: RecurrenceRejection::Syntax,
            ..
        }
    ));
}
