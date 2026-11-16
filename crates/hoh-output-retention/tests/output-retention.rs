// Port of all source output-retention.spec.ts cases.
use hoh_output_retention::{
    ItemRetainer, ItemRetentionStrategy, Omitted, PushDecision, RetainedText, RetentionLimit,
    RetentionNotice, RetentionStrategy, RetentionUnit, TextRetainer, TextRetentionStrategy,
    describe_omitted, format_retention_notice, truncate_without_splitting_surrogate_pair,
};
use serde_json::json;

fn items<T>(max_items: u64) -> ItemRetainer<T> {
    ItemRetainer::new(ItemRetentionStrategy::Head { max_items })
}
fn head(max_bytes: u64) -> TextRetainer {
    TextRetainer::new(TextRetentionStrategy::Head { max_bytes })
}
fn tail(max_bytes: u64) -> TextRetainer {
    TextRetainer::new(TextRetentionStrategy::Tail { max_bytes })
}
fn head_tail(head_bytes: u64, tail_bytes: u64) -> TextRetainer {
    TextRetainer::new(TextRetentionStrategy::HeadTail {
        head_bytes,
        tail_bytes,
    })
}
fn decision(kept: bool, truncated: bool) -> PushDecision {
    PushDecision { kept, truncated }
}
fn exact(count: u64) -> Omitted {
    Omitted::Exact { count }
}

#[test]
fn keeps_first_max_items_while_draining_for_exact_count() {
    let mut r = items(2);
    assert_eq!(r.push("a"), decision(true, false));
    assert_eq!(r.push("b"), decision(true, false));
    assert_eq!(r.push("c"), decision(false, true));
    let result = r.finish();
    assert_eq!(result.items, ["a", "b"]);
    assert_eq!(result.kept, 2);
    assert_eq!(result.seen, 3);
    assert!(result.truncated);
    assert_eq!(result.omitted, exact(1));
}
#[test]
fn items_report_none_when_everything_fits() {
    let mut r = items(3);
    r.push(1);
    r.push(2);
    let result = r.finish();
    assert_eq!(result.items, [1, 2]);
    assert!(!result.truncated);
    assert_eq!(result.omitted, Omitted::None);
}
#[test]
fn items_keep_draining_past_cap_and_report_exact_count() {
    let mut r = items(1);
    assert_eq!(r.push("a"), decision(true, false));
    assert_eq!(r.push("b"), decision(false, true));
    assert_eq!(r.push("c"), decision(false, true));
    let result = r.finish();
    assert_eq!(result.items, ["a"]);
    assert_eq!(result.seen, 3);
    assert_eq!(result.omitted, exact(2));
}
#[test]
fn zero_item_budget_keeps_nothing_counts_every_item() {
    let mut r = items(0);
    assert_eq!(r.push("a"), decision(false, true));
    let result = r.finish();
    assert!(result.items.is_empty());
    assert_eq!(result.kept, 0);
    assert_eq!(result.omitted, exact(1));
}
#[test]
fn rejects_noninteger_or_negative_max_items() {
    for max in [json!(-1), json!(1.5)] {
        let error =
            serde_json::from_value::<ItemRetentionStrategy>(json!({"kind":"head","maxItems":max}))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("maxItems must be a non-negative integer")
        );
    }
}

#[test]
fn head_keeps_prefix_and_counts_omitted_bytes_exactly() {
    let mut r = head(5);
    assert_eq!(r.push("abc"), decision(true, false));
    assert_eq!(r.push("de"), decision(true, false));
    assert_eq!(r.push("fgh"), decision(false, true));
    let result = r.finish();
    assert_eq!(result.text, "abcde");
    assert!(result.truncated);
    assert_eq!(result.omitted_bytes, exact(3));
}
#[test]
fn flags_partially_dropped_chunk_not_fully_kept() {
    let mut r = head(4);
    r.push("ab");
    assert_eq!(r.push("cde"), decision(false, true));
    assert_eq!(r.finish().text, "abcd");
}
#[test]
fn head_keeps_draining_past_cap() {
    let mut r = head(3);
    r.push("abc");
    assert_eq!(r.push("defg"), decision(false, true));
    let result = r.finish();
    assert_eq!(result.text, "abc");
    assert_eq!(result.omitted_bytes, exact(4));
}
#[test]
fn tail_keeps_final_max_bytes_and_exact_omission() {
    let mut r = tail(4);
    assert_eq!(r.push("hello"), decision(false, true));
    r.push("world");
    let result = r.finish();
    assert_eq!(result.text, "orld");
    assert!(result.truncated);
    assert_eq!(result.omitted_bytes, exact(6));
}
#[test]
fn tail_keeps_everything_under_cap() {
    let mut r = tail(100);
    r.push("short");
    let result = r.finish();
    assert_eq!(result.text, "short");
    assert!(!result.truncated);
    assert_eq!(result.omitted_bytes, Omitted::None);
}
#[test]
fn tail_drops_old_chunks_sliding_out_of_window() {
    let mut r = tail(3);
    for c in ["11", "22", "33", "44"] {
        r.push(c);
    }
    assert_eq!(r.finish().text, "344");
}
#[test]
fn head_tail_keeps_stable_windows_omits_middle_exactly() {
    let mut r = head_tail(3, 3);
    r.push("abcdefghij");
    let result = r.finish();
    assert_eq!(result.text, "abchij");
    assert!(result.truncated);
    assert_eq!(result.omitted_bytes, exact(4));
}
#[test]
fn head_tail_does_not_double_count_whole_stream() {
    let mut r = head_tail(3, 3);
    r.push("abcdef");
    let result = r.finish();
    assert_eq!(result.text, "abcdef");
    assert!(!result.truncated);
    assert_eq!(result.omitted_bytes, Omitted::None);
}
#[test]
fn head_tail_preserves_codepoint_spanning_split_without_omission() {
    let mut r = head_tail(1, 3);
    r.push("éab");
    let result = r.finish();
    assert_eq!(result.text, "éab");
    assert!(!result.truncated);
    assert_eq!(result.omitted_bytes, Omitted::None);
}
#[test]
fn head_tail_trims_boundary_partials_when_middle_omitted() {
    let mut r = head_tail(2, 2);
    r.push("a€€b");
    let result = r.finish();
    assert!(result.truncated);
    assert!(!result.text.contains('�'));
    assert!(result.text.starts_with('a'));
    assert!(result.text.ends_with('b'));
}
#[test]
fn zero_head_budget_counts_every_byte() {
    let mut r = head(0);
    assert_eq!(r.push("x"), decision(false, true));
    let result = r.finish();
    assert_eq!(result.text, "");
    assert_eq!(result.omitted_bytes, exact(1));
}
#[test]
fn empty_stream_omits_nothing() {
    let r = head_tail(2, 2);
    let result = r.finish();
    assert_eq!(result.text, "");
    assert!(!result.truncated);
    assert_eq!(result.omitted_bytes, Omitted::None);
}
#[test]
fn rejects_noninteger_or_negative_byte_budgets() {
    for (value, field) in [
        (json!({"kind":"head","maxBytes":-1}), "maxBytes"),
        (json!({"kind":"tail","maxBytes":2.5}), "maxBytes"),
        (
            json!({"kind":"headTail","headBytes":-1,"tailBytes":2}),
            "headBytes",
        ),
        (
            json!({"kind":"headTail","headBytes":2,"tailBytes":1.1}),
            "tailBytes",
        ),
    ] {
        assert!(
            serde_json::from_value::<TextRetentionStrategy>(value)
                .unwrap_err()
                .to_string()
                .contains(&format!("{field} must be a non-negative integer"))
        );
    }
}
#[test]
fn trims_partial_codepoint_at_head_cut_without_replacement() {
    let mut r = head(2);
    r.push("a€b");
    let result = r.finish();
    assert_eq!(result.text, "a");
    assert!(!result.text.contains('�'));
    assert_eq!(result.omitted_bytes, exact(4));
}
#[test]
fn trims_leading_partial_at_tail_cut() {
    let mut r = tail(2);
    r.push("a€b");
    let result = r.finish();
    assert_eq!(result.text, "b");
    assert!(!result.text.contains('�'));
    assert_eq!(result.omitted_bytes, exact(4));
}
#[test]
fn omitted_count_matches_absent_bytes_after_head_tail_trim() {
    let mut r = head_tail(2, 2);
    r.push("a€€b");
    let result = r.finish();
    assert_eq!(result.omitted_bytes, exact(8 - result.text.len() as u64));
}
#[test]
fn preserves_whole_multibyte_codepoint_fitting_exactly() {
    let mut r = head(3);
    r.push("€x");
    assert_eq!(r.finish().text, "€");
}
#[test]
fn never_reconstructs_codepoint_across_omitted_middle() {
    let mut r = head_tail(2, 2);
    r.push("€€€");
    let result = r.finish();
    assert!(!result.text.contains('�'));
    assert!(result.truncated);
}
#[test]
fn accepts_raw_byte_chunk() {
    let mut r = head(2);
    r.push("xy".as_bytes());
    r.push("z".as_bytes());
    assert_eq!(r.finish().text, "xy");
}
#[test]
fn trims_partial_two_byte_codepoint_at_head() {
    let mut r = head(2);
    r.push("aé");
    let result = r.finish();
    assert_eq!(result.text, "a");
    assert!(!result.text.contains('�'));
}
#[test]
fn trims_partial_four_byte_emoji_at_head() {
    let mut r = head(3);
    r.push("a😀");
    let result = r.finish();
    assert_eq!(result.text, "a");
    assert!(!result.text.contains('�'));
}
#[test]
fn keeps_whole_four_byte_codepoint_fitting_exactly() {
    let mut r = head(4);
    r.push("😀x");
    assert_eq!(r.finish().text, "😀");
}
#[test]
fn leaves_stray_continuation_run_at_head_cut_untouched() {
    let mut r = head(2);
    r.push([0x80, 0x80, 0x7a]);
    assert_eq!(r.finish().omitted_bytes, exact(1));
}
#[test]
fn leaves_invalid_lead_byte_at_head_cut_untouched() {
    let mut r = head(1);
    r.push([0xf8, 0x61]);
    assert_eq!(r.finish().omitted_bytes, exact(1));
}
#[test]
fn describes_exact_count() {
    assert_eq!(
        describe_omitted(&exact(3), RetentionUnit::Items),
        "Omitted 3 items."
    );
    assert_eq!(
        describe_omitted(&exact(12), RetentionUnit::Bytes),
        "Omitted 12 bytes."
    );
}
#[test]
fn describes_unknown_without_count() {
    assert_eq!(
        describe_omitted(&Omitted::Unknown, RetentionUnit::Lines),
        "More lines were omitted."
    );
}
#[test]
fn describes_none_as_empty() {
    assert_eq!(describe_omitted(&Omitted::None, RetentionUnit::Chars), "");
}

fn notice(omitted: Omitted) -> RetentionNotice {
    RetentionNotice {
        scope: "grep".into(),
        strategy: RetentionStrategy::Head,
        unit: RetentionUnit::Items,
        limit: RetentionLimit::Count(100),
        kept: 100,
        omitted,
    }
}
#[test]
fn notice_joins_standard_clause_with_recovery() {
    let out = format_retention_notice(&notice(exact(25)), |n| {
        format!(
            "Results capped at {}. Narrow the pattern, path, or include to see more.",
            n.kept
        )
    });
    assert_eq!(
        out,
        "Omitted 25 items. Results capped at 100. Narrow the pattern, path, or include to see more."
    );
}
#[test]
fn notice_omits_empty_omission_half() {
    assert_eq!(
        format_retention_notice(&notice(Omitted::None), |_| "Recovery text.".into()),
        "Recovery text."
    );
}
#[test]
fn notice_omits_empty_recovery_half() {
    assert_eq!(
        format_retention_notice(&notice(exact(2)), |_| String::new()),
        "Omitted 2 items."
    );
}
#[test]
fn notice_passes_full_head_tail_limit_to_recovery() {
    let n = RetentionNotice {
        scope: "bash stdout".into(),
        strategy: RetentionStrategy::HeadTail,
        unit: RetentionUnit::Bytes,
        limit: RetentionLimit::HeadTail {
            head: 2_000,
            tail: 2_000,
        },
        kept: 4_000,
        omitted: exact(500),
    };
    let out = format_retention_notice(&n, |n| match n.limit {
        RetentionLimit::HeadTail { head, tail } => format!("Kept {head}B head + {tail}B tail."),
        RetentionLimit::Count(_) => String::new(),
    });
    assert_eq!(out, "Omitted 500 bytes. Kept 2000B head + 2000B tail.");
}
#[test]
fn caps_at_max_chars_utf16_units() {
    assert_eq!(
        truncate_without_splitting_surrogate_pair("abcdef", 3),
        "abc"
    );
    assert_eq!(truncate_without_splitting_surrogate_pair("abcdef", 0), "");
    assert_eq!(truncate_without_splitting_surrogate_pair("abcdef", 1), "a");
}
#[test]
fn returns_text_within_cap_unchanged() {
    assert_eq!(truncate_without_splitting_surrogate_pair("abc", 3), "abc");
    assert_eq!(truncate_without_splitting_surrogate_pair("abc", 9), "abc");
    assert_eq!(truncate_without_splitting_surrogate_pair("", 0), "");
}
#[test]
fn drops_unpaired_half_if_cap_inside_surrogate_pair() {
    assert_eq!(truncate_without_splitting_surrogate_pair("ab😀cd", 3), "ab");
    assert_eq!(
        truncate_without_splitting_surrogate_pair("ab😀cd", 4),
        "ab😀"
    );
    assert_eq!(truncate_without_splitting_surrogate_pair("😀😀", 1), "");
    assert_eq!(truncate_without_splitting_surrogate_pair("😀😀", 3), "😀");
}
#[test]
fn never_ends_in_lone_high_surrogate_at_any_cap() {
    let text = "a😀b😀c";
    for max_chars in 0..=text.encode_utf16().count() as u64 {
        let capped = truncate_without_splitting_surrogate_pair(text, max_chars);
        assert!(capped.encode_utf16().count() as u64 <= max_chars);
        assert!(
            !capped
                .encode_utf16()
                .last()
                .is_some_and(|unit| (0xd800..=0xdbff).contains(&unit))
        );
        assert!(text.starts_with(capped));
    }
}

#[test]
fn split_raw_chunks_and_bom_decoding_follow_text_decoder() {
    let bytes = "\u{feff}Привет🙂".as_bytes();
    let mut r = head(100);
    for byte in bytes {
        r.push([*byte]);
    }
    assert_eq!(
        r.finish(),
        RetainedText {
            text: "Привет🙂".into(),
            truncated: false,
            omitted_bytes: Omitted::None
        }
    );
    assert_eq!(
        serde_json::to_string(&r.finish()).unwrap(),
        "{\"text\":\"Привет🙂\",\"truncated\":false,\"omittedBytes\":{\"kind\":\"none\"}}"
    );
}
