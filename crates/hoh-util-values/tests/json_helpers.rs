//! The source has no spec for `isJsonValue`, `snapshotJsonValue` and `deepEqualJson`; these
//! tests cover the rules of the source README for the Rust form of those helpers.

use hoh_util_values::{deep_equal_json, is_json_value, snapshot_json_value};
use serde_json::{Number, Value, json};

#[test]
fn accepts_lossless_json_and_rejects_negative_zero() {
    assert!(is_json_value(
        &json!({"a": [1, 2.5, "x", null, true, {"b": []}]})
    ));
    let negative_zero = Value::Number(Number::from_f64(-0.0).unwrap());
    assert!(!is_json_value(&negative_zero));
    assert!(!is_json_value(&json!({"a": [negative_zero]})));
    assert!(is_json_value(&Value::Number(
        Number::from_f64(0.0).unwrap()
    )));
}

#[test]
fn snapshot_detaches_a_valid_value_and_refuses_an_invalid_one() {
    let value = json!({"a": [1, {"b": "c"}]});
    assert_eq!(snapshot_json_value(&value), Some(value.clone()));
    assert_eq!(
        snapshot_json_value(&Value::Number(Number::from_f64(-0.0).unwrap())),
        None
    );
}

#[test]
fn deep_equal_compares_structure_ignoring_key_order_and_number_representation() {
    assert!(deep_equal_json(
        &json!({"a": 1, "b": [1, 2]}),
        &json!({"b": [1, 2], "a": 1.0})
    ));
    assert!(!deep_equal_json(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
    assert!(!deep_equal_json(&json!([1, 2]), &json!([1, 2, 3])));
    assert!(!deep_equal_json(&json!([]), &json!({})));
    assert!(!deep_equal_json(&json!(null), &json!(0)));
    assert!(deep_equal_json(&json!("x"), &json!("x")));
    assert!(!deep_equal_json(&json!({"a": 1}), &json!({"b": 1})));
}
