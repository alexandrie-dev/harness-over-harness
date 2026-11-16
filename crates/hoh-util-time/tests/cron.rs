//! Port of `tests/cron.spec.ts`: Cron parsing and bounded calendar arithmetic without Schedule task records or policy.

use hoh_util_time::{
    CRON_SEARCH_HORIZON_YEARS, Cron, InstantResult, RecurrenceOccurrence, RecurrenceRejection,
    RecurrenceResult, parse_offset_instant,
};

fn date_parse(text: &str) -> i64 {
    match parse_offset_instant(text) {
        InstantResult::Accepted { epoch } => epoch,
        InstantResult::Rejected { reason } => panic!("{text}: {}", reason.as_str()),
    }
}

fn parsed_in(expression: &str, time_zone: &str) -> Cron {
    match Cron::parse(expression, time_zone) {
        RecurrenceResult::Accepted { rule } => rule,
        RecurrenceResult::Rejected { detail, .. } => panic!("{detail}"),
    }
}

fn parsed(expression: &str) -> Cron {
    parsed_in(expression, "UTC")
}

fn canonical_expression(expression: &str) -> String {
    parsed(expression).expression().to_owned()
}

/// A valid rule with the committed floor used by its resolve operation.
struct Reference {
    rule: Cron,
    scheduled_at: String,
}

fn reference_in(now: &str, expression: &str, time_zone: &str) -> Reference {
    let rule = parsed_in(expression, time_zone);
    let scheduled_at = rule
        .next(date_parse(now))
        .unwrap()
        .expect("Cron fixture has no future occurrence");
    Reference { rule, scheduled_at }
}

fn reference(now: &str, expression: &str) -> Reference {
    reference_in(now, expression, "UTC")
}

fn decision(record: &Reference, now: &str) -> RecurrenceOccurrence {
    record
        .rule
        .resolve(&record.scheduled_at, date_parse(now))
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

// cron occurrence resolution across a DST fall-back

#[test]
fn keeps_the_repeated_hour_as_the_latest_due_occurrence() {
    // 2026-11-01 repeats 01:00-01:59 in America/New_York: 01:45 EDT is 05:45Z and the decision at 06:30Z renders as
    // 01:30 EST, the second pass.
    let record = reference_in("2026-10-25T05:45:00.000Z", "45 1 * * 0", "America/New_York");
    assert_eq!(
        decision(&record, "2026-11-01T06:30:00.000Z").occurrence_at,
        "2026-11-01T05:45:00.000Z"
    );
}

// cron occurrence resolution when the due local date is not the UTC decision date

#[test]
fn resolves_a_due_local_date_one_day_after_the_utc_decision_date() {
    // Pacific/Kiritimati runs UTC+14, so at 11:00Z its local date is already the next day.
    let zone = "Pacific/Kiritimati";
    let decision_at = date_parse("2026-01-01T11:00:00.000Z");
    let record = reference_in("2025-12-31T00:00:00.000Z", "30 0 * * *", zone);
    let resolved = date_parse(
        &record
            .rule
            .resolve(&record.scheduled_at, decision_at)
            .unwrap()
            .occurrence_at,
    );
    assert_eq!(resolved, date_parse("2026-01-01T10:30:00.000Z"));
    // An exhaustive scan of every minute-aligned instant around the decision finds the same occurrence.
    let tz = jiff::tz::TimeZone::get(zone).unwrap();
    let mut scanned = i64::MIN;
    let mut at = decision_at - 4 * 86_400_000;
    while at <= decision_at {
        let local = jiff::Timestamp::from_millisecond(at)
            .unwrap()
            .to_zoned_datetime(tz.clone());
        if local.hour() == 0 && local.minute() == 30 && local.second() == 0 {
            scanned = at;
        }
        at += 60_000;
    }
    assert_eq!(resolved, scanned);
    assert_ne!(resolved, decision_at);
    // The occurrence a walk bounded by the decision's UTC date would take is earlier.
    let utc_date = jiff::Timestamp::from_millisecond(decision_at)
        .unwrap()
        .to_zoned_datetime(jiff::tz::TimeZone::UTC)
        .date();
    let bounded = utc_date
        .at(0, 30, 0, 0)
        .to_zoned_datetime(tz)
        .unwrap()
        .timestamp()
        .as_millisecond();
    assert!(bounded < resolved);
    assert!(bounded <= decision_at);
}

// cron expression parsing and canonicalization

#[test]
fn canonicalizes_each_expression_and_is_idempotent() {
    let cases = [
        ("0 0 * * *", "0 0 * * *"),
        ("*/15 9-17 * * 1-5", "*/15 9-17 * * 1-5"),
        ("30,10,20 * * * *", "10-30/10 * * * *"),
        ("1,2,5 0 * * *", "1-2,5 0 * * *"),
        ("20,30,10 * * * *", "10-30/10 * * * *"),
        ("0,0,30 * * * *", "0-30/30 * * * *"),
        ("*/1 * * * *", "* * * * *"),
        ("0 0 */2 * *", "0 0 */2 * *"),
        ("0-59/1 0 * * *", "0-59 0 * * *"),
        ("0-59 0-23 1-31 1-12 0-7", "0-59 0-23 1-31 1-12 0-6"),
        ("0 0 * * 0", "0 0 * * 0"),
        ("0 0 * * 7", "0 0 * * 0"),
        ("0 0 * * 1-2,3-4", "0 0 * * 1-4"),
        ("0 0 * * 5-7", "0 0 * * 0,5-6"),
        ("1,3,5 * * * *", "1-5/2 * * * *"),
        ("1-5/2 * * * *", "1-5/2 * * * *"),
        ("00 09 * * 5,4,3,2,1", "0 9 * * 1-5"),
        ("0 0 1-15/2 * *", "0 0 1-15/2 * *"),
        ("0 0 * * */7", "0 0 * * */7"),
        ("0 0 * * */3", "0 0 * * */3"),
        // A field's canonical spelling keeps its star flag: a `*`-led field stays `*`-led, and a field that did not
        // start with `*` never gains one, so day-field AND/OR selection survives the stored canonical expression.
        ("0 9 1-31 * */7", "0 9 1-31 * */7"),
        ("0 9 * * */7", "0 9 * * */7"),
        ("0 9 * * */2", "0 9 * * */2"),
        ("0 9 */31 * 1", "0 9 */31 * 1"),
        ("0 9 */2 * 1", "0 9 */2 * 1"),
        ("0 9 */2,4 * *", "0 9 */2,4 * *"),
        ("0 9 1-31/2 * 1", "0 9 1-31/2 * 1"),
        ("0 9 * * 1", "0 9 * * 1"),
        ("* * * * *", "* * * * *"),
    ];
    for (input, canonical) in cases {
        assert_eq!(
            canonical_expression(input),
            canonical,
            "canonicalizes {input:?}"
        );
        assert_eq!(
            canonical_expression(canonical),
            canonical,
            "re-canonicalizes {canonical:?}"
        );
    }
}

#[test]
fn keeps_the_unrestricted_star_distinct_from_an_explicit_full_range() {
    assert_eq!(canonical_expression("* * * * *"), "* * * * *");
    assert_eq!(
        canonical_expression("0-59 0-23 1-31 1-12 0-7"),
        "0-59 0-23 1-31 1-12 0-6"
    );
    assert_eq!(canonical_expression("*,5 0 * * *"), "* 0 * * *");
    assert_eq!(canonical_expression("5,* 0 * * *"), "0-59 0 * * *");
}

#[test]
fn preserves_every_field_star_flag_and_idempotence_across_canonicalization() {
    let expressions = [
        "0 9 1-31 * */7",
        "0 9 */31 * 1",
        "0 9 */2 * 1",
        "0 9 * * */7",
        "0 9 * * 1",
        "*/1 * * * *",
        "0 9 */2,4 * *",
        "0 9 1-31/2 * 1",
        "0 9 * * */2",
        "0 9 */30 * *",
        "0,0,30 * * * *",
    ];
    for expression in expressions {
        let canonical = canonical_expression(expression);
        assert_eq!(canonical_expression(&canonical), canonical);
        let fields: Vec<&str> = expression.split(' ').collect();
        let canonical_fields: Vec<&str> = canonical.split(' ').collect();
        assert_eq!(canonical_fields.len(), fields.len());
        for (field, canonical_field) in fields.iter().zip(&canonical_fields) {
            assert_eq!(
                canonical_field.starts_with('*'),
                field.starts_with('*'),
                "{expression}"
            );
        }
    }
}

#[test]
fn resolves_an_expression_exactly_like_its_stored_canonical_spelling() {
    let expressions = [
        "0 9 1-31 * */7",
        "0 9 */31 * 1",
        "0 9 */2 * 1",
        "0 9 * * */7",
        "0 9 * * 1",
        "0 9 */2,4 * *",
        "0 9 1-31/2 * 1",
        "0 9 * * */2",
        "0 0 1-31 * 5",
    ];
    let floors = [
        "2026-09-02T00:00:00Z",
        "2026-09-13T00:00:00Z",
        "2026-09-20T00:00:00Z",
    ];
    for expression in expressions {
        let canonical = canonical_expression(expression);
        for floor in floors {
            let record = reference(floor, expression);
            let raw = parsed(expression);
            let stored = parsed(&canonical);
            let later = (date_parse(floor) + 86_400_000).max(date_parse(&record.scheduled_at) + 1);
            let later = hoh_util_time::format_four_digit_year_instant(later).unwrap();
            assert_eq!(
                raw.resolve(&record.scheduled_at, date_parse(&later))
                    .unwrap(),
                stored
                    .resolve(&record.scheduled_at, date_parse(&later))
                    .unwrap(),
                "{expression} from {floor}"
            );
        }
    }
}

#[test]
fn keeps_a_star_led_day_field_from_changing_the_day_rule_after_storage() {
    let star_led = reference("2026-09-01T00:00:00Z", "0 9 1-31 * */7");
    let restricted = Reference {
        rule: parsed("0 9 1-31 * 0"),
        scheduled_at: star_led.scheduled_at.clone(),
    };
    assert_eq!(star_led.rule.expression(), "0 9 1-31 * */7");
    assert_eq!(
        decision(&star_led, "2026-09-07T00:00:00Z")
            .next_scheduled_at
            .as_deref(),
        Some("2026-09-13T09:00:00.000Z")
    );
    assert_eq!(
        decision(&restricted, "2026-09-07T00:00:00Z")
            .next_scheduled_at
            .as_deref(),
        Some("2026-09-07T09:00:00.000Z")
    );
}

#[test]
fn rejects_malformed_expressions_with_a_stable_syntax_detail() {
    let cases: [(&str, &str, &str); 28] = [
        (
            "six fields",
            "0 0 * * * *",
            "cron.expression must contain exactly five whitespace-separated fields: minute hour day-of-month month day-of-week.",
        ),
        (
            "four fields",
            "0 0 * *",
            "cron.expression must contain exactly five whitespace-separated fields: minute hour day-of-month month day-of-week.",
        ),
        (
            "a macro",
            "@daily",
            "cron.expression must contain exactly five whitespace-separated fields: minute hour day-of-month month day-of-week.",
        ),
        (
            "last day",
            "0 0 L * *",
            "cron.expression day-of-month field element \"L\" must be *, a value, a-b, */n, a-b/n, or a comma-separated list of those.",
        ),
        (
            "nearest weekday",
            "0 0 15W * *",
            "cron.expression day-of-month field element \"15W\" must be *, a value, a-b, */n, a-b/n, or a comma-separated list of those.",
        ),
        (
            "nth weekday",
            "0 0 * * 1#2",
            "cron.expression day-of-week field element \"1#2\" must be *, a value, a-b, */n, a-b/n, or a comma-separated list of those.",
        ),
        (
            "a month name",
            "0 0 * JAN *",
            "cron.expression month field element \"JAN\" must be *, a value, a-b, */n, a-b/n, or a comma-separated list of those.",
        ),
        (
            "a weekday name",
            "0 0 * * MON",
            "cron.expression day-of-week field element \"MON\" must be *, a value, a-b, */n, a-b/n, or a comma-separated list of those.",
        ),
        (
            "minute above range",
            "60 0 * * *",
            "cron.expression minute field value 60 is outside 0-59.",
        ),
        (
            "hour above range",
            "0 24 * * *",
            "cron.expression hour field value 24 is outside 0-23.",
        ),
        (
            "day-of-month below range",
            "0 0 0 * *",
            "cron.expression day-of-month field value 0 is outside 1-31.",
        ),
        (
            "day-of-month above range",
            "0 0 32 * *",
            "cron.expression day-of-month field value 32 is outside 1-31.",
        ),
        (
            "month below range",
            "0 0 * 0 *",
            "cron.expression month field value 0 is outside 1-12.",
        ),
        (
            "month above range",
            "0 0 * 13 *",
            "cron.expression month field value 13 is outside 1-12.",
        ),
        (
            "weekday above range",
            "0 0 * * 8",
            "cron.expression day-of-week field value 8 is outside 0-7.",
        ),
        (
            "inverted day-of-month range",
            "0 0 20-10 * *",
            "cron.expression day-of-month field range 20-10 is inverted.",
        ),
        (
            "inverted month range",
            "0 0 * 12-1 *",
            "cron.expression month field range 12-1 is inverted.",
        ),
        (
            "zero step",
            "*/0 * * * *",
            "cron.expression minute field step must be a positive integer.",
        ),
        (
            "zero range step",
            "0-10/0 * * * *",
            "cron.expression minute field step must be a positive integer.",
        ),
        (
            "empty first element",
            "0,,1 * * * *",
            "cron.expression minute field must not contain an empty list element.",
        ),
        (
            "trailing comma",
            "0 0 * * 1,",
            "cron.expression day-of-week field must not contain an empty list element.",
        ),
        (
            "a bare step",
            "5/2 * * * *",
            "cron.expression minute field element \"5/2\" must be *, a value, a-b, */n, a-b/n, or a comma-separated list of those.",
        ),
        (
            "a bare star suffix",
            "*5 * * * *",
            "cron.expression minute field element \"*5\" must be *, a value, a-b, */n, a-b/n, or a comma-separated list of those.",
        ),
        (
            "a negative value",
            "-5 * * * *",
            "cron.expression minute field element \"-5\" must be *, a value, a-b, */n, a-b/n, or a comma-separated list of those.",
        ),
        (
            "leading whitespace",
            " 0 0 * * *",
            "cron.expression must be a non-empty trimmed string.",
        ),
        (
            "trailing whitespace",
            "0 0 * * * ",
            "cron.expression must be a non-empty trimmed string.",
        ),
        (
            "empty text",
            "",
            "cron.expression must be a non-empty trimmed string.",
        ),
        (
            "blank text",
            " ",
            "cron.expression must be a non-empty trimmed string.",
        ),
    ];
    for (label, expression, message) in cases {
        match Cron::parse(expression, "UTC") {
            RecurrenceResult::Rejected { reason, detail, .. } => {
                assert_eq!(reason, RecurrenceRejection::Syntax, "rejects {label}");
                assert_eq!(detail, message, "rejects {label}");
            }
            RecurrenceResult::Accepted { .. } => panic!("{label} was accepted"),
        }
    }
}

// cron target selection

#[test]
fn selects_the_strictly_future_instant() {
    for (now, scheduled_at) in [
        ("2026-09-16T09:29:59.999Z", "2026-09-16T09:30:00.000Z"),
        ("2026-09-16T09:30:00.000Z", "2026-09-17T09:30:00.000Z"),
        ("2026-09-16T09:30:00.001Z", "2026-09-17T09:30:00.000Z"),
    ] {
        assert_eq!(
            reference(now, "30 9 * * *").scheduled_at,
            scheduled_at,
            "{now}"
        );
    }
}

#[test]
fn selects_the_next_calendar_occurrence() {
    for (now, expression, scheduled_at) in [
        (
            "2026-04-01T00:00:00Z",
            "0 0 31 * *",
            "2026-05-31T00:00:00.000Z",
        ),
        (
            "2026-01-01T00:00:00Z",
            "0 0 29 2 *",
            "2028-02-29T00:00:00.000Z",
        ),
        (
            "2026-09-16T00:00:00Z",
            "0 0 1 * *",
            "2026-10-01T00:00:00.000Z",
        ),
        (
            "2026-09-16T00:00:00Z",
            "0 0 * 1 *",
            "2027-01-01T00:00:00.000Z",
        ),
    ] {
        assert_eq!(
            reference(now, expression).scheduled_at,
            scheduled_at,
            "{now} {expression}"
        );
    }
}

#[test]
fn skips_a_local_time_and_whole_date_that_fall_in_a_dst_gap() {
    let record = reference_in("2026-03-01T00:00:00Z", "30 2 * 3 0", "America/New_York");
    assert_eq!(record.scheduled_at, "2026-03-01T07:30:00.000Z");
    assert_eq!(
        decision(&record, "2026-03-08T12:00:00Z"),
        due("2026-03-01T07:30:00.000Z", "2026-03-15T06:30:00.000Z")
    );
    assert_eq!(
        reference_in("2026-03-02T00:00:00Z", "30 2 * 3 0", "America/New_York").scheduled_at,
        "2026-03-15T06:30:00.000Z"
    );
    assert_eq!(
        decision(&record, "2026-03-15T06:30:00Z"),
        due("2026-03-15T06:30:00.000Z", "2026-03-22T06:30:00.000Z")
    );
}

#[test]
fn uses_the_earlier_occurrence_of_a_dst_overlap_once_for_that_date() {
    let record = reference_in("2026-10-25T00:00:00Z", "30 1 * 11 0", "America/New_York");
    assert_eq!(record.scheduled_at, "2026-11-01T05:30:00.000Z");
    for now in [
        "2026-11-01T05:30:00Z",
        "2026-11-01T06:00:00Z",
        "2026-11-01T06:30:00Z",
    ] {
        assert_eq!(
            decision(&record, now),
            due("2026-11-01T05:30:00.000Z", "2026-11-08T06:30:00.000Z"),
            "{now}"
        );
    }
}

#[test]
fn matches_days_under_vixie_day_of_month_and_day_of_week_semantics() {
    for (label, now, expression, scheduled_at) in [
        (
            "friday the 13th matches an earlier Friday",
            "2026-09-01T00:00:00Z",
            "0 0 13 * 5",
            "2026-09-04T00:00:00.000Z",
        ),
        (
            "friday the 13th matches the 13th alone",
            "2026-09-12T00:00:00Z",
            "0 0 13 * 5",
            "2026-09-13T00:00:00.000Z",
        ),
        (
            "a restricted day-of-month constrains alone",
            "2026-09-01T00:00:00Z",
            "0 0 13 * *",
            "2026-09-13T00:00:00.000Z",
        ),
        (
            "a restricted day-of-week constrains alone",
            "2026-09-01T00:00:00Z",
            "0 0 * * 5",
            "2026-09-04T00:00:00.000Z",
        ),
        (
            "two stars match every date",
            "2026-09-01T00:00:00Z",
            "0 0 * * *",
            "2026-09-02T00:00:00.000Z",
        ),
        (
            "an explicit full day-of-month stays restricted",
            "2026-09-01T00:00:00Z",
            "0 0 1-31 * 5",
            "2026-09-02T00:00:00.000Z",
        ),
        (
            "an explicit full day-of-week stays restricted",
            "2026-09-01T00:00:00Z",
            "0 0 13 * 0-7",
            "2026-09-02T00:00:00.000Z",
        ),
        (
            "a stepped day-of-month star accepts an odd matching Monday",
            "2026-09-01T00:00:00Z",
            "0 9 */2 * 1",
            "2026-09-07T09:00:00.000Z",
        ),
        (
            "a stepped day-of-month star rejects an even matching Monday",
            "2026-09-13T00:00:00Z",
            "0 9 */2 * 1",
            "2026-09-21T09:00:00.000Z",
        ),
        (
            "a stepped day-of-week star still requires its own match",
            "2026-09-02T00:00:00Z",
            "0 9 1-31 * */2",
            "2026-09-03T09:00:00.000Z",
        ),
        (
            "two stepped stars require both fields",
            "2026-09-02T00:00:00Z",
            "0 9 */2 * */2",
            "2026-09-03T09:00:00.000Z",
        ),
        (
            "a stepped day-of-month star with a bare day-of-week star",
            "2026-09-02T00:00:00Z",
            "0 9 */2 * *",
            "2026-09-03T09:00:00.000Z",
        ),
        (
            "a bare day-of-month star with a stepped day-of-week star",
            "2026-09-02T00:00:00Z",
            "0 9 * * */2",
            "2026-09-03T09:00:00.000Z",
        ),
        (
            "a bare day-of-month star keeps only the weekday restriction",
            "2026-09-13T00:00:00Z",
            "0 9 * * 1",
            "2026-09-14T09:00:00.000Z",
        ),
    ] {
        assert_eq!(
            reference(now, expression).scheduled_at,
            scheduled_at,
            "{label}"
        );
    }
}

#[test]
fn selects_only_the_latest_due_occurrence_after_downtime_including_a_same_date_later_time() {
    let record = reference("2026-09-15T00:00:00Z", "0 9,17 * * *");
    assert_eq!(record.scheduled_at, "2026-09-15T09:00:00.000Z");
    assert_eq!(
        decision(&record, "2026-09-15T10:00:00Z"),
        due("2026-09-15T09:00:00.000Z", "2026-09-15T17:00:00.000Z")
    );
    assert_eq!(
        decision(&record, "2026-09-15T20:00:00Z"),
        due("2026-09-15T17:00:00.000Z", "2026-09-16T09:00:00.000Z")
    );
}

#[test]
fn honors_a_pinned_committed_target_and_never_repeats_a_same_date_occurrence() {
    let pinned = Reference {
        scheduled_at: "2026-09-15T12:00:00.000Z".to_owned(),
        ..reference("2026-09-15T00:00:00Z", "0 9,17 * * *")
    };
    assert_eq!(
        decision(&pinned, "2026-09-15T12:30:00Z"),
        due(&pinned.scheduled_at, "2026-09-15T17:00:00.000Z")
    );
    assert_eq!(
        decision(&pinned, "2026-09-15T17:30:00Z"),
        due("2026-09-15T17:00:00.000Z", "2026-09-16T09:00:00.000Z")
    );
}

#[test]
fn selects_only_the_latest_due_weekday_after_a_long_downtime() {
    let record = reference("2026-06-01T00:00:00Z", "0 9 * * 1-5");
    assert_eq!(record.scheduled_at, "2026-06-01T09:00:00.000Z");
    assert_eq!(
        decision(&record, "2026-09-16T08:59:59Z"),
        due("2026-09-15T09:00:00.000Z", "2026-09-16T09:00:00.000Z")
    );
    assert_eq!(
        decision(&record, "2026-09-16T09:00:00Z"),
        due("2026-09-16T09:00:00.000Z", "2026-09-17T09:00:00.000Z")
    );
}

#[test]
fn retains_the_committed_target_when_no_local_occurrence_is_representable() {
    let impossible = Reference {
        rule: parsed("0 0 30 2 *"),
        scheduled_at: "2026-09-15T00:00:00.000Z".to_owned(),
    };
    assert_eq!(
        decision(&impossible, "2026-09-16T00:00:00Z"),
        last(&impossible.scheduled_at)
    );
}

#[test]
fn stops_an_unsatisfiable_rule_at_the_documented_horizon_instead_of_the_year_ceiling() {
    // The source counts `Temporal.PlainDate` add/subtract calls with spies on the polyfill; that instrumentation has
    // no counterpart here. What is observable is kept: the search ends with the committed target, and the horizon
    // constant is the documented one.
    assert_eq!(CRON_SEARCH_HORIZON_YEARS, 400);
    let impossible = Reference {
        rule: parsed("0 0 30 2 *"),
        scheduled_at: "2026-09-15T00:00:00.000Z".to_owned(),
    };
    assert_eq!(
        decision(&impossible, "2026-09-16T00:00:00Z"),
        last(&impossible.scheduled_at)
    );
    assert_eq!(
        impossible
            .rule
            .next(date_parse("2026-09-15T00:00:00Z"))
            .unwrap(),
        None
    );
}

#[test]
fn converts_a_walked_date_once_while_it_lies_beyond_the_decision() {
    // The source bounds the number of `PlainDate.toPlainDateTime` calls with a spy; only the result is observable
    // here: an every-minute rule resolves to the decision's own minute.
    let record = reference_in("2026-09-16T00:00:00Z", "* * * * *", "America/New_York");
    assert_eq!(
        decision(&record, "2026-09-16T12:00:00Z"),
        due("2026-09-16T12:00:00.000Z", "2026-09-16T12:01:00.000Z")
    );
}

#[test]
fn resolves_local_dates_on_either_side_of_the_lowest_supported_utc_year() {
    assert_eq!(
        reference("0001-01-01T00:00:00Z", "30 0 * * *").scheduled_at,
        "0001-01-01T00:30:00.000Z"
    );
    assert_eq!(
        reference_in("0001-01-01T00:00:00Z", "0 0 * * *", "Etc/GMT-1").scheduled_at,
        "0001-01-01T23:00:00.000Z"
    );
    // A decision this early keeps the backward search on the four-digit floor, not the horizon.
    assert_eq!(
        decision(
            &reference("0001-01-01T00:00:00Z", "0 0 * * *"),
            "0001-01-02T00:00:00Z"
        ),
        due("0001-01-02T00:00:00.000Z", "0001-01-03T00:00:00.000Z")
    );
}

#[test]
fn resolves_a_negative_offset_due_minute_inside_the_floor_date_of_its_own_zone() {
    // The four-digit floor is an instant, so this rule's latest due minute before a decision inside the floor's UTC
    // day is local and dated one day earlier.
    let record = reference_in("0001-01-01T00:01:00Z", "* * * * *", "Etc/GMT+1");
    assert_eq!(
        decision(&record, "0001-01-01T00:30:00Z"),
        due("0001-01-01T00:30:00.000Z", "0001-01-01T00:31:00.000Z")
    );
}

#[test]
fn retains_the_final_occurrence_and_reports_exhaustion_without_a_five_digit_target() {
    let record = reference("9999-12-31T23:00:00Z", "* * * * *");
    assert_eq!(
        decision(&record, "9999-12-31T23:59:59.999Z"),
        last("9999-12-31T23:59:00.000Z")
    );
    assert_eq!(
        parsed("* * * * *")
            .next(date_parse("9999-12-31T23:59:00Z"))
            .unwrap(),
        None
    );
}
