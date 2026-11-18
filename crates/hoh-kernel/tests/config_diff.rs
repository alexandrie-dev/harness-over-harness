use hoh_kernel::config::equal_except_volatile as equal;
use serde_json::json;
#[tokio::test(start_paused = true)]
async fn ignores_volatile_fields_in_frozen_raw_inputs_without_running_schema_callbacks() {
    let s = json!({"type":"object","properties":{"title":{"type":"string","x-volatile":true},"fixed":{"type":"string"}}});
    let a = json!({"title":"old","fixed":"same"});
    let b = json!({"title":{"__jsExpr":"throw new Error(\"must not run\")"},"fixed":"same"});
    assert!(equal(&s, &a, &b));
    assert!(equal(&s, &a, &b));
    assert_eq!(a["title"], "old");
    assert_eq!(b["title"]["__jsExpr"], "throw new Error(\"must not run\")");
}
#[tokio::test(start_paused = true)]
async fn ignores_absent_nested_and_whole_object_array_union_volatile_fields() {
    let s = json!({"type":"object","properties":{"nested":{"type":"object","default":{},"properties":{"value":{"type":"string","x-volatile":true}}},"block":{"type":"object","x-volatile":true},"list":{"type":"array","x-volatile":true},"choice":{"oneOf":[{"type":"string"},{"type":"number"}],"x-volatile":true}}});
    let b = json!({"nested":{"value":"new"},"block":{"title":"new"},"list":["new"],"choice":1});
    assert!(equal(&s, &json!({}), &b));
    assert!(equal(&s, &b, &json!({})));
    assert!(equal(
        &json!({"type":"string","x-volatile":true}),
        &json!("old"),
        &json!("new")
    ));
}
#[tokio::test(start_paused = true)]
async fn preserves_ordinary_and_unknown_fields_object_defaults_and_parent_expressions() {
    let s = json!({"type":"object","properties":{"nested":{"type":"object","properties":{"live":{"x-volatile":true},"fixed":{"type":"string"}},"default":{"fixed":"default"}}}});
    assert!(equal(
        &s,
        &json!({}),
        &json!({"nested":{"live":"new","fixed":"default"}})
    ));
    for b in [json!({"nested":{"live":"new"}}), json!({"unknown":"new"})] {
        assert!(!equal(&s, &json!({}), &b));
    }
    assert!(!equal(
        &s,
        &json!({"nested":{"__jsExpr":"source.a"}}),
        &json!({"nested":{"__jsExpr":"source.b"}})
    ));
    assert!(!equal(
        &s,
        &json!({"nested":"invalid"}),
        &json!({"nested":{"live":"new"}})
    ));
    assert!(!equal(
        &s,
        &json!({"nested":[]}),
        &json!({"nested":["new"]})
    ));
}
#[tokio::test(start_paused = true)]
async fn keeps_ordinary_raw_changes_even_if_schema_validation_would_normalize_them_to_equal_values()
{
    let s = json!({"type":"object","properties":{"fixed":{"type":"string"},"live":{"x-volatile":true}}});
    assert!(!equal(
        &s,
        &json!({"fixed":"first"}),
        &json!({"fixed":"second"})
    ));
    assert!(!equal(&json!(true), &json!("first"), &json!("second")));
}
#[tokio::test(start_paused = true)]
async fn compares_raw_values_strictly_without_a_schema() {
    assert!(equal(
        &json!(true),
        &json!({"fixed":"same"}),
        &json!({"fixed":"same"})
    ));
    assert!(!equal(&json!(true), &json!([null]), &json!(["new"])));
    assert!(!equal(&json!(true), &json!(0), &json!(false)));
}
#[tokio::test(start_paused = true)]
async fn compares_recursive_schemas_conservatively_and_visits_shared_schema_nodes_at_each_path() {
    let s = json!({"$ref":"#/$defs/Tree","$defs":{"Tree":{"type":"object","properties":{"value":{"x-volatile":true},"child":{"$ref":"#/$defs/Tree"}}}}});
    assert!(!equal(&s, &json!({"value":"old"}), &json!({"value":"new"})));
    let shared = json!({"type":"object","default":{},"properties":{"value":{"x-volatile":true}}});
    let s = json!({"type":"object","properties":{"first":shared,"second":shared}});
    assert!(equal(
        &s,
        &json!({}),
        &json!({"first":{"value":"one"},"second":{"value":"two"}})
    ));
}
#[tokio::test(start_paused = true)]
async fn equates_absent_objects_with_defaults_while_preserving_ordinary_field_differences() {
    let s = json!({"type":"object","default":{},"properties":{"nested":{"type":"object","properties":{"fixed":{"type":"string"}},"default":{"fixed":"default"}}}});
    assert!(equal(&s, &json!(null), &json!({})));
    assert!(equal(&s, &json!({}), &json!(null)));
    assert!(equal(
        &s,
        &json!({"nested":null}),
        &json!({"nested":{"fixed":"default"}})
    ));
    assert!(!equal(
        &s,
        &json!({}),
        &json!({"nested":{"fixed":"changed"}})
    ));
    assert!(!equal(&s, &json!({}), &json!({"nested":{}})));
}
#[tokio::test(start_paused = true)]
async fn preserves_parent_expression_edits_even_when_their_marker_matches_a_volatile_field() {
    let s = json!({"type":"object","properties":{"__jsExpr":{"x-volatile":true}}});
    assert!(!equal(
        &s,
        &json!({"__jsExpr":"first"}),
        &json!({"__jsExpr":"second"})
    ));
}
#[tokio::test(start_paused = true)]
async fn compares_metadata_free_serialized_schema_nodes() {
    let s:serde_json::Value=serde_json::from_str(r#"{"type":"object","properties":{"nested":{"type":"object","properties":{"enabled":{"type":"boolean"}}}}}"#).unwrap();
    assert!(!equal(
        &s,
        &json!({"nested":{"enabled":true}}),
        &json!({"nested":{"enabled":false}})
    ));
}

#[tokio::test(start_paused = true)]
async fn compares_unknown_fields_whose_names_match_object_prototype_properties() {
    let schema = json!({"type":"object","properties":{"live":{"x-volatile":true}}});
    for key in ["constructor", "toString", "__proto__"] {
        let before = json!({key:"first","live":"old"});
        assert!(before.get(key).is_some());
        assert!(!equal(
            &schema,
            &before,
            &json!({key:"second","live":"new"})
        ));
        assert!(equal(&schema, &before, &json!({key:"first","live":"new"})));
    }
}
