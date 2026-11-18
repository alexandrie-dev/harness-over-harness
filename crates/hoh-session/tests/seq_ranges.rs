mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
#[test]
fn round_trips() {
    for numbers in [
        vec![],
        vec![5],
        vec![10, 11, 12, 13, 14],
        vec![16, 17, 100, 200, 201, 202, 203],
        vec![3, 2],
        vec![9_007_199_254_740_990, 0, 9_007_199_254_740_989],
    ] {
        let values = numbers.into_iter().map(seq).collect::<Vec<_>>();
        assert_eq!(
            decode_seq_ranges(
                &serde_json::to_value(encode_seq_ranges(&values)).unwrap(),
                None
            )
            .unwrap(),
            values
        );
    }
}
#[test]
fn encodes_only_profitable_increasing_runs() {
    for (input, expected) in [
        (vec![1, 3, 4, 5, 7], json!([1, [3, 5], 7])),
        (vec![1, 3, 4, 7], json!([1, 3, 4, 7])),
        (vec![3, 2], json!([3, 2])),
    ] {
        assert_eq!(
            serde_json::to_value(encode_seq_ranges(
                &input.into_iter().map(seq).collect::<Vec<_>>()
            ))
            .unwrap(),
            expected
        );
    }
}
#[test]
fn does_not_impose_a_persistence_only_source_event_length_limit() {
    let values = (0..1_000_001).map(seq).collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_value(encode_seq_ranges(&values)).unwrap(),
        json!([[0, 1_000_000]])
    );
}
#[test]
fn rejects_malformed_or_impossible_expansions() {
    for (v, limit, text) in [
        (json!("nope"), None, "must be an array"),
        (json!([-1]), None, "non-negative safe integers"),
        (json!([[1]]), None, "[start, end] pairs"),
        (json!([[4, 2]]), None, "start <= end"),
        (json!([[2, 5], [4, 7]]), None, "strictly increasing"),
        (json!([0]), Some(0), "exceeds its event sequence"),
        (json!([[0, 10]]), Some(10), "exceeds its event sequence"),
    ] {
        error(decode_seq_ranges(&v, limit), text);
    }
}
