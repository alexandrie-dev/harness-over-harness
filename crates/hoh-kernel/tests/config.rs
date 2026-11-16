use hoh_kernel::*;
use serde_json::{Value, json};
#[tokio::test(start_paused = true)]
async fn validates_all_contract_keywords_and_collects_paths() {
    let schema = json!({"type":"object","properties":{"name":{"type":"string","minLength":2,"maxLength":4,"pattern":"^[a-z]+$"},"count":{"type":"number","minimum":2,"maximum":8,"multipleOf":2},"items":{"type":"array","prefixItems":[{"const":true}],"items":{"enum":["x","y"]},"minItems":2,"maxItems":3}},"required":["required"],"additionalProperties":false});
    let error = config::validate(
        &schema,
        &json!({"name":"X","count":9,"items":[false,"bad","x","x"],"extra":1}),
    )
    .unwrap_err();
    assert!(error.issues.len() >= 8);
    assert!(
        error
            .issues
            .iter()
            .any(|i| i.path == [PathSeg::Key("items".into()), PathSeg::Index(1)])
    );
    assert!(
        error
            .to_string()
            .contains("missing required value (at required)")
    );
    assert!(config::validate(&json!({"type":["null","string"]}), &Value::Null).is_ok());
    assert!(
        config::validate(
            &json!({"type":"number","exclusiveMinimum":0,"exclusiveMaximum":2}),
            &json!(0)
        )
        .is_err()
    );
}
#[tokio::test(start_paused = true)]
async fn validates_refs_boolean_schemas_and_combinations() {
    let s = json!({"$ref":"#/$defs/Value","$defs":{"Value":{"anyOf":[{"type":"string"},{"type":"integer"}]}}});
    assert!(config::validate(&s, &json!("yes")).is_ok());
    assert!(config::validate(&s, &json!(1)).is_ok());
    assert!(config::validate(&s, &json!(false)).is_err());
    assert!(config::validate(&json!(false), &json!(1)).is_err());
    assert!(config::validate(&json!(true), &json!(1)).is_ok());
    assert!(config::validate(&json!({"oneOf":[true,true]}), &json!(1)).is_err());
    assert!(config::validate(&json!({"allOf":[{"minimum":0},{"maximum":2}]}), &json!(3)).is_err());
    assert!(config::validate(&json!({"properties":{"unused":{"not":true}}}), &json!({})).is_err());
}
#[tokio::test(start_paused = true)]
async fn validates_javascript_ascii_classes_and_utf16_lengths() {
    assert!(config::validate(&json!({"pattern":r"^\\d$"}), &json!(r"\d")).is_ok());
    assert!(config::validate(&json!({"type":"string","pattern":"^\\d+$"}), &json!("12")).is_ok());
    assert!(config::validate(&json!({"type":"string","pattern":"^\\d+$"}), &json!("١٢")).is_err());
    assert!(
        config::validate(
            &json!({"type":"string","pattern":"^.+$"}),
            &json!("русский")
        )
        .is_ok()
    );
    assert!(config::validate(&json!({"type":"string","maxLength":1}), &json!("😀")).is_err());
}
#[tokio::test(start_paused = true)]
async fn volatile_paths_follow_fixed_object_fields_in_schema_order() {
    let schema = json!({"type":"object","properties":{"a":{"x-volatile":true},"b":{"type":"object","properties":{"c":{"x-volatile":true}}}}});
    assert_eq!(
        config::volatile_paths(
            &schema,
            &json!({"a":1,"b":{"c":2}}),
            &json!({"a":2,"b":{"c":3}})
        ),
        ["a", "b.c"]
    );
}
