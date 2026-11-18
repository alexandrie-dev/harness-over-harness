use hoh_util_values::{is_json_value, snapshot_json_value};
use serde_json::json;
#[test]
fn copies_the_complete_json_scalar_vocabulary_and_rejects_unsupported_scalars() {
    for value in [json!(null), json!(true), json!("text"), json!(1.25)] {
        assert_eq!(snapshot_json_value(&value), Some(value));
    }
    assert!(snapshot_json_value(&json!(-0.0)).is_none());
    assert!(!is_json_value(&json!(-0.0)));
}
#[test]
fn recursively_detaches_dense_arrays_and_plain_objects() {
    let mut source = json!({"list":[{"shared":{"value":1}},{"value":1}],"alias":{"value":1}});
    let snapshot = snapshot_json_value(&source).unwrap();
    source["alias"]["value"] = json!(2);
    assert_eq!(snapshot["alias"]["value"], 1);
    assert_eq!(snapshot["list"][0]["shared"]["value"], 1);
}
#[test]
fn detaches_every_occurrence_of_a_shared_child_in_a_wide_array() {
    let source = json!(vec![json!({"nested":[{"value":1}]}); 4096]);
    let mut snapshot = snapshot_json_value(&source).unwrap();
    snapshot[0]["nested"][0]["value"] = json!(3);
    assert_eq!(source[0]["nested"][0]["value"], 1);
    assert_eq!(snapshot[4095]["nested"][0]["value"], 1);
}
#[test]
fn preserves_a_literal_proto_json_key_without_changing_the_snapshot_prototype() {
    let source = json!({"__proto__":{"safe":true}});
    assert_eq!(snapshot_json_value(&source).unwrap(), source);
}
#[test]
fn recognizes_supported_scalars_and_rejects_every_lossy_scalar_case() {
    for v in [json!(null), json!(false), json!("text"), json!(1.25)] {
        assert!(is_json_value(&v));
    }
    assert!(!is_json_value(&json!(-0.0)));
}
#[test]
fn accepts_dense_arrays_and_plain_objects() {
    for v in [
        json!([1,{"nested":null},{"value":true}]),
        json!({"value":[1,2]}),
        json!({"value":true}),
    ] {
        assert!(is_json_value(&v));
    }
}
