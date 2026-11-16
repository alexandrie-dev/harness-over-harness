//! Cases of ECMAScript `Number::toString` and `String(value)`; expected strings come from the
//! ECMAScript algorithm, not from a reference run.

use hoh_util_values::{js_number_string, js_string};
use serde_json::json;

#[test]
fn number_special_values_and_zeros() {
    assert_eq!(js_number_string(f64::NAN), "NaN");
    assert_eq!(js_number_string(f64::INFINITY), "Infinity");
    assert_eq!(js_number_string(f64::NEG_INFINITY), "-Infinity");
    assert_eq!(js_number_string(0.0), "0");
    assert_eq!(js_number_string(-0.0), "0");
}

#[test]
fn number_integers_and_decimals() {
    for (value, expected) in [
        (1.0, "1"),
        (-42.0, "-42"),
        (100.0, "100"),
        (123.456, "123.456"),
        (100_000_000_000_000_000_000.0, "100000000000000000000"),
        (123_456_789_012_345_680_000.0, "123456789012345680000"),
        (0.000001, "0.000001"),
        (0.1 + 0.2, "0.30000000000000004"),
        (9_007_199_254_740_992.0, "9007199254740992"),
        (9_007_199_254_740_994.0, "9007199254740994"),
        (-1.5, "-1.5"),
    ] {
        assert_eq!(js_number_string(value), expected);
    }
}

#[test]
fn number_exponent_boundaries() {
    for (value, expected) in [
        (1e21, "1e+21"),
        (999_999_999_999_999_900_000.0, "999999999999999900000"),
        (1e-7, "1e-7"),
        (1.5e-7, "1.5e-7"),
        (0.000_001_5, "0.0000015"),
        (1.2345e25, "1.2345e+25"),
        (1e300, "1e+300"),
        (5e-324, "5e-324"),
        (f64::MAX, "1.7976931348623157e+308"),
        (-1e21, "-1e+21"),
        (-1e-7, "-1e-7"),
    ] {
        assert_eq!(js_number_string(value), expected);
    }
}

#[test]
fn string_of_scalars() {
    assert_eq!(js_string(&json!(null)), "null");
    assert_eq!(js_string(&json!(true)), "true");
    assert_eq!(js_string(&json!(false)), "false");
    assert_eq!(js_string(&json!("a b")), "a b");
    assert_eq!(js_string(&json!(1)), "1");
    assert_eq!(js_string(&json!(1.0)), "1");
    assert_eq!(js_string(&json!(-42)), "-42");
    assert_eq!(js_string(&json!(0.5)), "0.5");
    assert_eq!(js_string(&json!(1e21)), "1e+21");
}

#[test]
fn string_of_arrays_and_objects() {
    assert_eq!(js_string(&json!([1, [2, 3], null, "a"])), "1,2,3,,a");
    assert_eq!(js_string(&json!([])), "");
    assert_eq!(js_string(&json!([{"a": 1}])), "[object Object]");
    assert_eq!(js_string(&json!([1, {}])), "1,[object Object]");
    assert_eq!(js_string(&json!({})), "[object Object]");
    assert_eq!(js_string(&json!({"a": {}})), "[object Object]");
    assert_eq!(js_string(&json!([[]])), "");
    assert_eq!(js_string(&json!([[], []])), ",");
}
