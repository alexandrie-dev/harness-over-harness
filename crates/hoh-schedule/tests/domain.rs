use hoh_schedule::*;
use hoh_util_time::{InstantResult, four_digit_year_epoch};
use serde_json::json;

fn id() -> ScheduleId {
    ScheduleId::from("schedule-1")
}
fn ms(value: &str) -> i64 {
    match four_digit_year_epoch(value) {
        InstantResult::Accepted { epoch } => epoch,
        InstantResult::Rejected { reason } => panic!("bad test instant: {reason:?}"),
    }
}

#[test]
fn title_is_trimmed_and_required() {
    assert_eq!(schedule_title("  Backup  ").unwrap(), "Backup");
    let error = schedule_title(" \t ").unwrap_err();
    assert_eq!(error.code(), ScheduleInputErrorCode::InvalidPrompt);
    assert_eq!(error.to_string(), REQUIRED_TITLE_MESSAGE);
}

#[test]
fn title_limit_counts_utf16_code_units() {
    assert!(schedule_title(&"a".repeat(120)).is_ok());
    assert!(schedule_title(&"😀".repeat(60)).is_ok());
    assert_eq!(
        schedule_title(&"😀".repeat(61)).unwrap_err().to_string(),
        "title must be at most 120 characters."
    );
}

#[test]
fn stored_title_requires_canonical_form() {
    assert_eq!(decode_stored_title(&json!("Task")).unwrap(), "Task");
    assert_eq!(
        decode_stored_title(&json!(" Task"))
            .unwrap_err()
            .to_string(),
        "title must be a trimmed string"
    );
}

#[test]
fn canonical_instant_decoder_rejects_offsets_and_bad_dates() {
    assert_eq!(
        decode_instant(&json!("2025-01-01T00:00:00.000Z")).unwrap(),
        "2025-01-01T00:00:00.000Z"
    );
    assert_eq!(
        decode_instant(&json!("2025-01-01T01:00:00.000+01:00"))
            .unwrap_err()
            .code(),
        "corrupt_schedule_log"
    );
    assert_eq!(
        decode_instant(&json!("2025-02-30T00:00:00.000Z"))
            .unwrap_err()
            .to_string(),
        "scheduledAt is not a real UTC calendar instant"
    );
}

#[test]
fn after_record_is_normalized() {
    let record = create_after_schedule_record(
        id(),
        "  ping  ",
        5,
        ms("2025-01-01T00:00:00.000Z"),
        " Ping ",
    )
    .unwrap();
    assert_eq!(record.prompt(), "ping");
    assert_eq!(record.title(), "Ping");
    assert_eq!(record.scheduled_at(), "2025-01-01T00:00:05.000Z");
}

#[test]
fn after_requires_positive_delay() {
    let error = create_after_schedule_record(id(), "p", 0, 0, "t").unwrap_err();
    assert_eq!(error.code(), ScheduleInputErrorCode::InvalidRule);
    assert_eq!(
        error.to_string(),
        "after_seconds must be a positive safe integer."
    );
}

#[test]
fn prompt_validation_precedes_timing_validation() {
    let error = create_after_schedule_record(id(), "  ", 0, 0, "t").unwrap_err();
    assert_eq!(error.code(), ScheduleInputErrorCode::InvalidPrompt);
    assert_eq!(
        error.to_string(),
        "prompt must be non-empty after trimming."
    );
}

#[test]
fn elapsed_selectors_reject_values_above_js_safe_integer() {
    let unsafe_integer = 9_007_199_254_740_992;
    assert_eq!(
        create_after_schedule_record(id(), "p", unsafe_integer, 0, "t")
            .unwrap_err()
            .to_string(),
        "after_seconds must be a positive safe integer."
    );
    assert_eq!(
        create_every_schedule_record(id(), "p", unsafe_integer, 0, "t")
            .unwrap_err()
            .to_string(),
        "every_seconds must be a safe integer."
    );
}

#[test]
fn absolute_selector_requires_explicit_offset() {
    let error = parse_at_input(&AtInput::Instant("2025-01-01T00:00:00".into())).unwrap_err();
    assert_eq!(error.code(), ScheduleInputErrorCode::InvalidRule);
    assert!(error.to_string().contains("explicit Z or numeric offset"));
}

#[test]
fn local_absolute_selector_resolves_zone() {
    let at = AtInput::Local(LocalAtInput {
        date: "2025-01-01".into(),
        time: "12:00:00".into(),
        time_zone: "Europe/Berlin".into(),
    });
    assert_eq!(parse_at_input(&at).unwrap(), ms("2025-01-01T11:00:00.000Z"));
}

#[test]
fn local_absolute_selector_rejects_dst_gap() {
    let at = AtInput::Local(LocalAtInput {
        date: "2025-03-30".into(),
        time: "02:30:00".into(),
        time_zone: "Europe/Berlin".into(),
    });
    assert_eq!(
        parse_at_input(&at).unwrap_err().to_string(),
        "The local at time does not exist in the selected time zone."
    );
}

#[test]
fn every_has_minimum_interval() {
    let now = ms("2025-01-01T00:00:00.000Z");
    let error = create_every_schedule_record(id(), "p", 59, now, "t").unwrap_err();
    assert_eq!(error.code(), ScheduleInputErrorCode::FrequencyTooHigh);
    let record = create_every_schedule_record(id(), "p", 60, now, "t").unwrap();
    assert_eq!(record.scheduled_at(), "2025-01-01T00:01:00.000Z");
}

#[test]
fn every_collapses_backlog_to_latest_due() {
    let record =
        create_every_schedule_record(id(), "p", 60, ms("2025-01-01T00:00:00.000Z"), "t").unwrap();
    let occurrence = resolve_every_occurrence(&record, ms("2025-01-01T00:05:12.000Z")).unwrap();
    assert_eq!(occurrence.occurrence_at, "2025-01-01T00:05:00.000Z");
    assert_eq!(
        occurrence.next_scheduled_at.as_deref(),
        Some("2025-01-01T00:06:00.000Z")
    );
}

#[test]
fn daily_time_is_normalized() {
    let input = parse_daily_input(&DailyInput {
        time: "09:10:11.2".into(),
        time_zone: "UTC".into(),
    })
    .unwrap();
    assert_eq!(input.time, "09:10:11.200");
}

#[test]
fn daily_rejects_invalid_clock() {
    let error = parse_daily_input(&DailyInput {
        time: "24:00:00".into(),
        time_zone: "UTC".into(),
    })
    .unwrap_err();
    assert!(error.to_string().contains("without leap seconds or 24:00"));
}

#[test]
fn daily_skips_nonexistent_local_time() {
    let record = create_daily_schedule_record(
        id(),
        "p",
        &DailyInput {
            time: "02:30:00".into(),
            time_zone: "Europe/Berlin".into(),
        },
        ms("2025-03-29T12:00:00.000Z"),
        "t",
    )
    .unwrap();
    assert_eq!(record.scheduled_at(), "2025-03-31T00:30:00.000Z");
}

#[test]
fn weekdays_are_unique_and_sorted() {
    assert_eq!(normalize_weekdays(&[7, 1, 4]).unwrap(), vec![1, 4, 7]);
    assert_eq!(
        normalize_weekdays(&[1, 1]).unwrap_err().to_string(),
        "weekly.weekdays must not repeat weekday 1."
    );
}

#[test]
fn weekly_finds_selected_day() {
    let record = create_weekly_schedule_record(
        id(),
        "p",
        &WeeklyInput {
            time: "09:00:00".into(),
            time_zone: "UTC".into(),
            weekdays: vec![1],
        },
        ms("2025-01-01T00:00:00.000Z"),
        "t",
    )
    .unwrap();
    assert_eq!(record.scheduled_at(), "2025-01-06T09:00:00.000Z");
}

#[test]
fn cron_is_canonicalized() {
    assert_eq!(
        canonicalize_cron_expression("30,10,20 * * * *").unwrap(),
        "10-30/10 * * * *"
    );
}

#[test]
fn cron_first_occurrence_uses_zone() {
    let record = create_cron_schedule_record(
        id(),
        "p",
        &CronInput {
            expression: "0 9 * * *".into(),
            time_zone: "Europe/Berlin".into(),
        },
        ms("2025-01-01T08:30:00.000Z"),
        "t",
    )
    .unwrap();
    assert_eq!(record.scheduled_at(), "2025-01-02T08:00:00.000Z");
}

#[test]
fn rrule_is_normalized_and_scheduled() {
    let rule = "DTSTART:20250101T090000Z\nRRULE:FREQ=DAILY;COUNT=2";
    let record =
        create_rrule_schedule_record(id(), "t", "p", rule, ms("2025-01-01T00:00:00.000Z")).unwrap();
    assert_eq!(record.scheduled_at(), "2025-01-01T09:00:00.000Z");
}

#[test]
fn stored_legacy_rrule_with_ineffective_setpos_can_resume() {
    let stored = json!({
        "id":"schedule-1", "kind":"rrule", "title":"t", "prompt":"p",
        "rrule":"DTSTART:20260916T010000Z\nRRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1",
        "scheduledAt":"2026-09-16T01:00:00.000Z"
    });
    let record = decode_schedule_record(&stored).unwrap();
    let resumed = update_schedule_record(
        &record,
        &SchedulePatch::default(),
        ms("2026-09-16T02:00:00.000Z"),
        true,
    )
    .unwrap();
    assert_eq!(resumed.scheduled_at(), "2026-09-17T01:00:00.000Z");
}

#[test]
fn record_decoder_requires_exact_keys() {
    let value = json!({"id":"schedule-1","kind":"at","title":"t","prompt":"p","scheduledAt":"2025-01-01T00:00:00.000Z","extra":true});
    assert!(
        decode_schedule_record(&value)
            .unwrap_err()
            .to_string()
            .contains("must contain exactly")
    );
}

#[test]
fn record_decoder_checks_kind_fields_before_identity_and_committed_time() {
    let value = json!({
        "id":"bad id", "kind":"after", "title":"t", "prompt":"p",
        "afterSeconds":0, "scheduledAt":"not-an-instant"
    });
    assert_eq!(
        decode_schedule_record(&value).unwrap_err().to_string(),
        "afterSeconds must be a positive safe integer"
    );
}

#[test]
fn weekly_decoder_wraps_untyped_weekday_failures_like_the_source() {
    let value = json!({
        "id":"schedule-1", "kind":"weekly", "title":"t", "prompt":"p",
        "time":"09:00:00.000", "timeZone":"UTC", "weekdays":[1.5],
        "scheduledAt":"2025-01-01T00:00:00.000Z"
    });
    assert_eq!(
        decode_schedule_record(&value).unwrap_err().to_string(),
        "ScheduleInputError: Each weekly.weekdays entry must be an integer from 1 (Monday) through 7 (Sunday)."
    );
}

#[test]
fn weekly_decoder_distinguishes_empty_weekdays_and_checks_zone_before_time() {
    let empty = json!({
        "id":"schedule-1", "kind":"weekly", "title":"t", "prompt":"p",
        "time":"09:00:00.000", "timeZone":"UTC", "weekdays":[],
        "scheduledAt":"2025-01-01T00:00:00.000Z"
    });
    assert_eq!(
        decode_schedule_record(&empty).unwrap_err().to_string(),
        "ScheduleInputError: weekly.weekdays must be a non-empty array of ISO weekday numbers."
    );

    let conflicting = json!({
        "id":"schedule-1", "kind":"weekly", "title":"t", "prompt":"p",
        "time":"bad", "timeZone":"No/Zone", "weekdays":[1],
        "scheduledAt":"2025-01-01T00:00:00.000Z"
    });
    let error = decode_schedule_record(&conflicting)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "ScheduleInputError: time_zone must be UTC or a valid IANA Area/Location name."
    );

    let invalid_time_and_weekdays = json!({
        "id":"schedule-1", "kind":"weekly", "title":"t", "prompt":"p",
        "time":"bad", "timeZone":"UTC", "weekdays":[],
        "scheduledAt":"2025-01-01T00:00:00.000Z"
    });
    assert_eq!(
        decode_schedule_record(&invalid_time_and_weekdays)
            .unwrap_err()
            .to_string(),
        "ScheduleInputError: weekly.time must use HH:mm:ss with optional 1-3 fractional digits, without leap seconds or 24:00."
    );
}

#[test]
fn malformed_local_at_precedes_an_invalid_time_zone() {
    let error = parse_at_input(&AtInput::Local(LocalAtInput {
        date: "2025-13-45".into(),
        time: "10:00:00".into(),
        time_zone: "No/Zone".into(),
    }))
    .unwrap_err();
    assert_eq!(error.code(), ScheduleInputErrorCode::InvalidRule);
    assert_eq!(
        error.to_string(),
        "The at value must be a real ISO calendar date and time."
    );
}

#[test]
fn historical_display_uses_temporal_minute_precision_for_offsets() {
    let record = create_daily_schedule_record(
        id(),
        "p",
        &DailyInput {
            time: "09:00:00".into(),
            time_zone: "Europe/Paris".into(),
        },
        ms("2025-01-01T00:00:00.000Z"),
        "t",
    )
    .unwrap();
    assert_eq!(
        format_schedule_local_time(&record, "1890-01-01T00:00:00.000Z").unwrap(),
        "1890-01-01T00:09:21+00:09"
    );
}

#[test]
fn record_round_trips_through_wire_json() {
    let record =
        create_every_schedule_record(id(), "p", 60, ms("2025-01-01T00:00:00.000Z"), "t").unwrap();
    assert_eq!(
        decode_schedule_record(&serde_json::to_value(&record).unwrap()).unwrap(),
        record
    );
}

#[test]
fn framing_json_escapes_dynamic_values() {
    let record =
        create_after_schedule_record(ScheduleId::from("a\"b"), "line\nnext", 1, 0, "t").unwrap();
    assert_eq!(
        render_reminder_framing(&record),
        "[SCHEDULE REMINDER]\nThis is a scheduled message from the user\nschedule_id_json: \"a\\\"b\"\noccurrence_at: 1970-01-01T00:00:01.000Z\nreminder_prompt_json: \"line\\nnext\""
    );
}

#[test]
fn unchanged_edit_preserves_committed_target() {
    let record = create_every_schedule_record(id(), "p", 60, 0, "old").unwrap();
    let changed = update_schedule_record(
        &record,
        &SchedulePatch {
            title: Some("new".into()),
            ..SchedulePatch::default()
        },
        50_000,
        false,
    )
    .unwrap();
    assert_eq!(changed.title(), "new");
    assert_eq!(changed.scheduled_at(), record.scheduled_at());
}

#[test]
fn changed_timing_rearms_from_edit_time() {
    let record = create_every_schedule_record(id(), "p", 60, 0, "t").unwrap();
    let changed = update_schedule_record(
        &record,
        &SchedulePatch {
            timing: Some(ScheduleTiming::Every { every_seconds: 120 }),
            ..SchedulePatch::default()
        },
        50_000,
        false,
    )
    .unwrap();
    assert_eq!(changed.scheduled_at(), "1970-01-01T00:02:50.000Z");
}

#[test]
fn target_wire_fields_are_camel_case() {
    let value = serde_json::to_value(ScheduleTarget::Session {
        session_id: hoh_session::SessionId::from("s"),
    })
    .unwrap();
    assert_eq!(value, json!({"kind":"session","sessionId":"s"}));
}

#[test]
fn local_display_uses_authoritative_rule_zone() {
    let record = create_daily_schedule_record(
        id(),
        "p",
        &DailyInput {
            time: "09:00:00".into(),
            time_zone: "Europe/Berlin".into(),
        },
        ms("2025-01-01T00:00:00.000Z"),
        "t",
    )
    .unwrap();
    assert_eq!(
        format_schedule_local_time(&record, "2025-01-01T12:00:00.000Z").unwrap(),
        "2025-01-01T13:00:00+01:00"
    );
}

#[test]
fn config_defaults_and_bounds_match_source_schema() {
    assert_eq!(Config::default().delivery_history_days, 30);
    assert_eq!(Config::default().delivery_history_records, 200);
    assert_eq!(Config::default().new_session_interval_seconds, 60);
    assert!(
        Config {
            delivery_history_days: 0,
            ..Config::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            delivery_history_records: 10_001,
            ..Config::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            new_session_interval_seconds: 59,
            ..Config::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn daily_source_matrix_covers_strict_future_dates_fractional_seconds_and_loose_times() {
    for (now, scheduled) in [
        ("2026-09-16T14:59:59.999Z", "2026-09-16T15:00:00.000Z"),
        ("2026-09-16T15:00:00.000Z", "2026-09-17T15:00:00.000Z"),
        ("2026-09-16T15:00:00.001Z", "2026-09-17T15:00:00.000Z"),
        ("2024-02-28T23:00:00.000Z", "2024-02-29T23:00:00.000Z"),
        ("2025-02-28T23:00:00.000Z", "2025-03-01T23:00:00.000Z"),
        ("2000-02-28T23:00:00.000Z", "2000-02-29T23:00:00.000Z"),
        ("2100-02-28T23:00:00.000Z", "2100-03-01T23:00:00.000Z"),
    ] {
        let zone = if now.starts_with("2026-09-16T1") {
            "Asia/Shanghai"
        } else {
            "UTC"
        };
        let record = create_daily_schedule_record(
            id(),
            "Daily reminder",
            &DailyInput {
                time: "23:00:00".into(),
                time_zone: zone.into(),
            },
            ms(now),
            "Daily reminder",
        )
        .unwrap();
        assert_eq!(record.scheduled_at(), scheduled, "{now}");
    }

    for (time, normalized) in [
        ("23:00:00.1", "23:00:00.100"),
        ("23:00:00.12", "23:00:00.120"),
        ("23:00:00.123", "23:00:00.123"),
    ] {
        assert_eq!(
            parse_daily_input(&DailyInput {
                time: time.into(),
                time_zone: "UTC".into(),
            })
            .unwrap()
            .time,
            normalized
        );
    }
    for time in [
        "23:00",
        "3:00:00",
        "23:0:00",
        "23:00:0",
        "24:00:00",
        "23:60:00",
        "23:59:60",
        "23:00:00.",
        "23:00:00.1234",
        "23:00:00Z",
        "23:00:00+08:00",
        " 23:00:00",
        "23:00:00 ",
        "",
    ] {
        assert!(
            parse_daily_input(&DailyInput {
                time: time.into(),
                time_zone: "UTC".into(),
            })
            .is_err(),
            "accepted {time:?}"
        );
    }
}

#[test]
fn weekly_source_matrix_covers_normalization_strict_future_and_fractional_seconds() {
    for (input, expected) in [
        (vec![1], vec![1]),
        (vec![7], vec![7]),
        (vec![3, 1], vec![1, 3]),
        (vec![7, 2, 5, 1], vec![1, 2, 5, 7]),
        (vec![6, 4, 2, 1, 3, 5, 7], vec![1, 2, 3, 4, 5, 6, 7]),
    ] {
        assert_eq!(normalize_weekdays(&input).unwrap(), expected);
    }
    for input in [vec![], vec![0], vec![8], vec![2, 2], vec![1, 3, 1]] {
        assert!(normalize_weekdays(&input).is_err(), "accepted {input:?}");
    }
    for (now, expected) in [
        ("2026-09-14T00:00:00.000Z", "2026-09-14T09:00:00.000Z"),
        ("2026-09-14T09:00:00.000Z", "2026-09-21T09:00:00.000Z"),
        ("2026-09-14T09:00:00.001Z", "2026-09-21T09:00:00.000Z"),
    ] {
        let record = create_weekly_schedule_record(
            id(),
            "Weekly reminder",
            &WeeklyInput {
                time: "09:00:00".into(),
                time_zone: "UTC".into(),
                weekdays: vec![1],
            },
            ms(now),
            "Weekly reminder",
        )
        .unwrap();
        assert_eq!(record.scheduled_at(), expected, "{now}");
    }
    for (time, normalized) in [
        ("09:00:00.1", "09:00:00.100"),
        ("09:00:00.12", "09:00:00.120"),
        ("09:00:00.123", "09:00:00.123"),
    ] {
        let parsed = parse_weekly_input(&WeeklyInput {
            time: time.into(),
            time_zone: "UTC".into(),
            weekdays: vec![6, 4, 2],
        })
        .unwrap();
        assert_eq!(parsed.time, normalized);
        assert_eq!(parsed.weekdays, vec![2, 4, 6]);
    }
}

#[test]
fn cron_source_matrix_covers_normalization_zones_and_durable_rejections() {
    for (expression, normalized) in [
        ("00 09 * * 7", "0 9 * * 0"),
        ("30,10,20 * * * *", "10-30/10 * * * *"),
        ("*/15 9-17 * * 1-5", "*/15 9-17 * * 1-5"),
    ] {
        assert_eq!(
            canonicalize_cron_expression(expression).unwrap(),
            normalized
        );
    }
    for expression in ["", "0 0 L * *", "0 0 30 2 * *", "0 0 * *"] {
        assert!(
            canonicalize_cron_expression(expression).is_err(),
            "accepted {expression:?}"
        );
    }
    for zone in ["+08:00", "CST", "Unknown/Zone"] {
        let error = parse_cron_input(&CronInput {
            expression: "0 0 * * *".into(),
            time_zone: zone.into(),
        })
        .unwrap_err();
        assert_eq!(error.code(), ScheduleInputErrorCode::InvalidTimeZone);
    }

    let record = create_cron_schedule_record(
        id(),
        "Prompt",
        &CronInput {
            expression: "0 9 * * 1-5".into(),
            time_zone: "UTC".into(),
        },
        ms("2026-09-16T00:00:00.000Z"),
        "Title",
    )
    .unwrap();
    let mut value = serde_json::to_value(&record).unwrap();
    value["expression"] = json!("00 09 * * 1-5");
    assert!(
        decode_schedule_record(&value)
            .unwrap_err()
            .to_string()
            .contains("cron expression must be canonical")
    );
}

#[test]
fn rrule_source_matrix_maps_parser_failures_and_rejects_malformed_storage() {
    let daily = "DTSTART:20260916T010000Z\nRRULE:FREQ=DAILY";
    assert_eq!(normalize_rrule(&format!(" {daily}\n")).unwrap(), daily);
    for (rule, code) in [
        ("bad", ScheduleInputErrorCode::InvalidRule),
        (
            "DTSTART:20260916T010000Z\nRRULE:FREQ=SECONDLY",
            ScheduleInputErrorCode::FrequencyTooHigh,
        ),
        (
            "DTSTART;TZID=Not/A_Zone:20260916T010000\nRRULE:FREQ=DAILY",
            ScheduleInputErrorCode::InvalidTimeZone,
        ),
    ] {
        assert_eq!(normalize_rrule(rule).unwrap_err().code(), code);
    }
    let created = create_rrule_schedule_record(
        id(),
        "Title",
        "Prompt",
        daily,
        ms("2026-09-16T00:00:00.000Z"),
    )
    .unwrap();
    for (field, value) in [
        ("title", json!(" Title")),
        ("prompt", json!(" Prompt")),
        ("rrule", json!("bad")),
        ("id", json!(" task")),
        ("scheduledAt", json!("2026-09-16T01:00:00Z")),
    ] {
        let mut malformed = serde_json::to_value(&created).unwrap();
        malformed[field] = value;
        assert!(
            decode_schedule_record(&malformed).is_err(),
            "accepted {field}"
        );
    }
}

fn timing_kind(record: &ScheduleRecord) -> &'static str {
    match record {
        ScheduleRecord::After { .. } => "after",
        ScheduleRecord::At { .. } => "at",
        ScheduleRecord::Every { .. } => "every",
        ScheduleRecord::Daily { .. } => "daily",
        ScheduleRecord::Weekly { .. } => "weekly",
        ScheduleRecord::Cron { .. } => "cron",
        ScheduleRecord::Rrule { .. } => "rrule",
    }
}

#[test]
fn update_source_matrix_converts_every_timing_pair_without_changing_identity() {
    let now = ms("2026-09-16T00:00:00.125Z");
    let timings = vec![
        ScheduleTiming::After { after_seconds: 61 },
        ScheduleTiming::At {
            at: AtInput::Instant("2026-09-17T01:00:00.123Z".into()),
        },
        ScheduleTiming::Every { every_seconds: 300 },
        ScheduleTiming::Daily {
            daily: DailyInput {
                time: "09:00:00.123".into(),
                time_zone: "UTC".into(),
            },
        },
        ScheduleTiming::Weekly {
            weekly: WeeklyInput {
                time: "09:00:00.123".into(),
                time_zone: "UTC".into(),
                weekdays: vec![1, 5],
            },
        },
        ScheduleTiming::Cron {
            cron: CronInput {
                expression: "0 9 * * 1-5".into(),
                time_zone: "UTC".into(),
            },
        },
        ScheduleTiming::Rrule {
            rrule: "DTSTART:20260916T010000Z\nRRULE:FREQ=DAILY".into(),
        },
    ];
    for from in &timings {
        let current = create_schedule_record(id(), "Title", "Prompt", from, now).unwrap();
        for to in &timings {
            let updated = update_schedule_record(
                &current,
                &SchedulePatch {
                    title: Some(" New title ".into()),
                    prompt: Some(" New prompt ".into()),
                    timing: Some(to.clone()),
                    ..SchedulePatch::default()
                },
                now,
                false,
            )
            .unwrap();
            assert_eq!(updated.id(), current.id());
            assert_eq!(updated.title(), "New title");
            assert_eq!(updated.prompt(), "New prompt");
            assert_eq!(
                timing_kind(&updated),
                timing_kind(
                    &create_schedule_record(id(), "New title", "New prompt", to, now).unwrap()
                )
            );
        }
    }
}
