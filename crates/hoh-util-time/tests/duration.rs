//! Port of `tests/duration.spec.ts`.

use hoh_util_time::{Duration, DurationParts, TimeError};

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Rebuild a span from its own parts, so a wrong split shows up as a wrong total.
fn part_sum(parts: &DurationParts) -> f64 {
    parts.days * 86_400_000.0
        + parts.hours * 3_600_000.0
        + parts.minutes * 60_000.0
        + parts.seconds * 1_000.0
        + parts.milliseconds
}

/// Deterministic span source, so a failure is reproducible from the seed alone (the same floating-point steps).
fn spans(count: usize, seed: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut state = seed;
    for _ in 0..count {
        state = (state * 1_103_515_245.0 + 12_345.0) % 2_147_483_648.0;
        out.push((state / 2_147_483_648.0) * MAX_SAFE_INTEGER);
    }
    out
}

fn parts(milliseconds: f64) -> DurationParts {
    Duration::from_milliseconds(milliseconds)
        .unwrap()
        .decompose()
        .unwrap()
}

fn expected(days: f64, hours: f64, minutes: f64, seconds: f64, milliseconds: f64) -> DurationParts {
    DurationParts {
        days,
        hours,
        minutes,
        seconds,
        milliseconds,
    }
}

#[test]
fn construction_accepts_a_fractional_millisecond_count_and_keeps_the_fraction() {
    assert_eq!(parts(1_500.25), expected(0.0, 0.0, 0.0, 1.0, 500.25));
}

#[test]
fn construction_accepts_the_largest_exactly_representable_count() {
    let p = parts(MAX_SAFE_INTEGER);
    assert_eq!(p, expected(104_249_991.0, 8.0, 59.0, 0.0, 991.0));
    assert_eq!(part_sum(&p), MAX_SAFE_INTEGER);
}

#[test]
fn construction_refuses_a_count_that_is_not_a_finite_number() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            matches!(Duration::from_milliseconds(value), Err(TimeError::Type(_))),
            "{value}"
        );
    }
    assert_eq!(
        Duration::from_milliseconds(f64::NAN),
        Err(TimeError::Type(
            "duration milliseconds must be finite, got NaN".to_owned()
        ))
    );
}

#[test]
fn construction_refuses_a_count_above_the_exactly_representable_millisecond_range() {
    assert!(matches!(
        Duration::from_milliseconds(MAX_SAFE_INTEGER + 1.0),
        Err(TimeError::Type(_))
    ));
    assert!(matches!(
        Duration::from_milliseconds(-MAX_SAFE_INTEGER - 2.0),
        Err(TimeError::Type(_))
    ));
    assert_eq!(
        Duration::from_milliseconds(MAX_SAFE_INTEGER + 1.0),
        Err(TimeError::Type(
            "duration milliseconds must not exceed 9007199254740991 so its parts stay exact, got 9007199254740992"
                .to_owned()
        ))
    );
    // The bound is what keeps the whole-part identity exact, so it is asserted, not assumed.
    assert_eq!(part_sum(&parts(MAX_SAFE_INTEGER)), MAX_SAFE_INTEGER);
}

#[test]
fn decompose_splits_a_span_into_floored_whole_parts_and_the_fractional_remainder() {
    assert_eq!(parts(0.0), expected(0.0, 0.0, 0.0, 0.0, 0.0));
    assert_eq!(parts(999.0), expected(0.0, 0.0, 0.0, 0.0, 999.0));
    assert_eq!(parts(1_000.0), expected(0.0, 0.0, 0.0, 1.0, 0.0));
    assert_eq!(parts(3_661_999.0), expected(0.0, 1.0, 1.0, 1.0, 999.0));
    assert_eq!(parts(90_061_500.0), expected(1.0, 1.0, 1.0, 1.0, 500.0));
    assert_eq!(parts(90_061_500.75), expected(1.0, 1.0, 1.0, 1.0, 500.75));
    assert_eq!(parts(86_400_000.0), expected(1.0, 0.0, 0.0, 0.0, 0.0));
}

#[test]
fn decompose_keeps_the_whole_part_identity_across_the_accepted_range() {
    for span in 0..=120_000 {
        let span = f64::from(span);
        assert_eq!(part_sum(&parts(span)), span);
    }
    for span in spans(2_000, 20_260_924.0) {
        let p = parts(span);
        assert_eq!(part_sum(&p), span);
        assert!(p.milliseconds >= 0.0);
        assert!(p.milliseconds < 1_000.0);
        assert!(p.hours < 24.0);
        assert!(p.minutes < 60.0);
        assert!(p.seconds < 60.0);
    }
}

#[test]
fn decompose_refuses_a_negative_span_instead_of_inventing_a_sign_policy() {
    for value in [-1.0, -0.5] {
        assert!(matches!(
            Duration::from_milliseconds(value).unwrap().decompose(),
            Err(TimeError::Range(_))
        ));
    }
}
