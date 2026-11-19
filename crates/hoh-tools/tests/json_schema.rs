//! Port of tests/json-schema.spec.ts. Cases that exercise JavaScript-only object graphs (realms,
//! proxies, symbols, getters, sparse arrays, cycles) have no `serde_json::Value` counterpart.

use hoh_tools::{
    JsonSchemaError, JsonSchemaNode, assert_object_json_schema, assert_supported_json_schema,
    validate_json_schema_value,
};
use serde_json::{Value, json};

fn asserted(schema: Value) -> JsonSchemaNode {
    assert_supported_json_schema(&schema).expect("schema is supported");
    schema
}

fn violations_of(schema: Value, object_root: bool) -> Vec<String> {
    let result = if object_root {
        assert_object_json_schema(&schema)
    } else {
        assert_supported_json_schema(&schema)
    };
    match result {
        Ok(()) => panic!("expected schema rejection"),
        Err(error) => JsonSchemaError::violations(&error)
            .expect("a JsonSchemaError carries its violations")
            .to_vec(),
    }
}

fn v(schema: Value) -> Vec<String> {
    violations_of(schema, false)
}

fn validate(schema: &Value, value: Value) -> Vec<String> {
    validate_json_schema_value(schema, &value, "value")
}

fn on_big_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(test)
        .expect("thread spawns")
        .join()
        .expect("test thread completes");
}

#[test]
fn accepts_every_json_root_and_every_supported_node() {
    for schema in [
        json!({ "type": "string" }),
        json!({ "type": "number" }),
        json!({ "type": "integer" }),
        json!({ "type": "boolean" }),
        json!({ "type": "null" }),
        json!({ "type": "array", "items": { "type": "string" } }),
        json!({
            "type": "object",
            "properties": {
                "nested": { "type": "object", "properties": {}, "additionalProperties": false },
                "free": {},
            },
            "required": ["nested"],
            "additionalProperties": true,
        }),
        json!({ "oneOf": [{ "type": "string" }, { "type": "number" }] }),
        json!({ "description": "any JSON", "title": "JSON", "default": null, "examples": [1, "x"] }),
    ] {
        assert!(assert_supported_json_schema(&schema).is_ok(), "{schema}");
    }
}

#[test]
fn retains_an_object_root_guard_only_at_consumers_that_need_it() {
    assert!(assert_object_json_schema(&json!({ "type": "object" })).is_ok());
    for schema in [
        json!({}),
        json!({ "type": "string" }),
        json!({ "type": "array" }),
        json!({ "oneOf": [{ "type": "string" }, { "type": "null" }] }),
    ] {
        assert_eq!(
            violations_of(schema, true),
            vec!["schema.type must be \"object\" (structured output is object-rooted)"]
        );
    }
}

#[test]
fn rejects_non_schema_nodes_unknown_types_and_type_arrays() {
    assert_eq!(v(Value::Null), vec!["schema must be a schema object"]);
    assert_eq!(v(json!([])), vec!["schema must be a schema object"]);
    assert_eq!(v(json!("no")), vec!["schema must be a schema object"]);
    assert!(v(json!({ "type": "tuple" }))[0].contains("type must be one of"));
    assert_eq!(
        v(json!({ "type": ["string", "null"] })),
        vec!["schema.type must be a single type string (type arrays are not supported)"]
    );
}

#[test]
fn enforces_one_of_vocabulary_and_its_minimum_branch_count() {
    let at_least_two = vec!["schema.oneOf must be an array of at least two schemas"];
    assert_eq!(v(json!({ "oneOf": [] })), at_least_two);
    assert_eq!(v(json!({ "oneOf": [{}] })), at_least_two);
    assert_eq!(v(json!({ "oneOf": "x" })), at_least_two);
    assert_eq!(
        v(json!({ "type": "string", "oneOf": [{}, {}] })),
        vec!["schema cannot declare both type and oneOf"]
    );
    assert_eq!(
        v(json!({ "oneOf": [{ "type": "string" }, { "type": "number" }], "items": {} })),
        vec!["schema.items is not supported beside oneOf"]
    );
    assert!(
        v(json!({ "oneOf": [{ "type": "string" }, { "type": "weird" }] }))[0]
            .contains("schema.oneOf[1].type")
    );
}

#[test]
fn rejects_unknown_and_misplaced_keywords_without_accepted_then_ignored_behavior() {
    for keyword in [
        "anyOf",
        "allOf",
        "not",
        "pattern",
        "minimum",
        "maxLength",
        "$ref",
    ] {
        let schema = json!({ "type": "object", keyword: [] });
        assert!(
            v(schema)[0].contains(&format!("schema.{keyword} is not a supported keyword")),
            "{keyword}"
        );
    }
    assert_eq!(
        v(json!({ "type": "object", "items": {} })),
        vec!["schema.items is not supported on type \"object\""]
    );
    assert_eq!(
        v(json!({ "type": "array", "properties": {} })),
        vec!["schema.properties is not supported on type \"array\""]
    );
    assert_eq!(
        v(json!({ "type": "object", "enum": ["x"] })),
        vec!["schema.enum is not supported on type \"object\""]
    );
    assert_eq!(
        v(json!({ "type": "array", "const": null })),
        vec!["schema.const is not supported on type \"array\""]
    );
    assert_eq!(
        v(json!({
            "properties": {}, "required": [], "additionalProperties": true,
            "items": {}, "enum": [], "const": null
        })),
        vec![
            "schema.properties requires type or oneOf",
            "schema.required requires type or oneOf",
            "schema.additionalProperties requires type or oneOf",
            "schema.items requires type or oneOf",
            "schema.enum requires type or oneOf",
            "schema.const requires type or oneOf",
        ]
    );
}

#[test]
fn reports_every_independent_schema_violation() {
    assert_eq!(
        v(json!({
            "type": "object",
            "pattern": "x",
            "properties": { "a": { "type": "weird" }, "b": { "type": "string", "minimum": 1 } },
        }))
        .len(),
        3
    );
}

#[test]
fn validates_object_properties_required_names_and_openness() {
    assert_eq!(
        v(json!({ "type": "object", "properties": [] })),
        vec!["schema.properties must be an object of schemas"]
    );
    assert_eq!(
        v(json!({ "type": "object", "properties": { "a": "x" } })),
        vec!["schema.properties.a must be a schema object"]
    );
    assert_eq!(
        v(json!({ "type": "object", "required": "a" })),
        vec!["schema.required must be an array of strings"]
    );
    assert_eq!(
        v(json!({ "type": "object", "required": [1] })),
        vec!["schema.required must be an array of strings"]
    );
    assert_eq!(
        v(json!({ "type": "object", "properties": {}, "required": ["missing"] })),
        vec!["schema.required names \"missing\" which is not in properties"]
    );
    assert_eq!(
        v(json!({ "type": "object", "additionalProperties": "yes" })),
        vec!["schema.additionalProperties must be a boolean"]
    );
    // `properties: undefined` of the source is a present key without an object: `null` here.
    assert_eq!(
        v(json!({ "type": "object", "properties": null })),
        vec!["schema.properties must be an object of schemas"]
    );
    assert_eq!(
        v(json!({ "type": "object", "properties": null, "required": ["missing"] })),
        vec![
            "schema.properties must be an object of schemas",
            "schema.required names \"missing\" which is not in properties",
        ]
    );
}

#[test]
fn requires_type_correct_scalar_enum_and_const_values() {
    for schema in [
        json!({ "type": "string", "enum": ["a"], "const": "a" }),
        json!({ "type": "number", "enum": [1.5], "const": 1.5 }),
        json!({ "type": "integer", "enum": [1], "const": 1 }),
        json!({ "type": "boolean", "enum": [true], "const": true }),
        json!({ "type": "null", "enum": [null], "const": null }),
    ] {
        assert!(assert_supported_json_schema(&schema).is_ok(), "{schema}");
    }
    assert_eq!(
        v(json!({ "type": "string", "enum": [] })),
        vec!["schema.enum must be a non-empty array of string values"]
    );
    assert_eq!(
        v(json!({ "type": "number", "enum": ["1"] })),
        vec!["schema.enum must be a non-empty array of number values"]
    );
    assert_eq!(
        v(json!({ "type": "integer", "enum": [1.5] })),
        vec!["schema.enum must be a non-empty array of integer values"]
    );
    assert_eq!(
        v(json!({ "type": "number", "const": -0.0 })),
        vec!["schema.const must be a number value"]
    );
    assert_eq!(
        v(json!({ "type": "boolean", "const": 1 })),
        vec!["schema.const must be a boolean value"]
    );
    // `enum: undefined` of the source is a present key without an array: `null` here.
    assert_eq!(
        v(json!({ "type": "string", "enum": null })),
        vec!["schema.enum must be a non-empty array of string values"]
    );
    assert_eq!(
        v(json!({ "type": "string", "enum": ["a"], "const": "b" })),
        vec!["schema.const must be one of schema.enum when both are declared"]
    );
}

#[test]
fn validates_annotation_types_and_lossless_json_payloads() {
    assert_eq!(
        v(json!({ "description": 1 })),
        vec!["schema.description must be a string"]
    );
    assert_eq!(
        v(json!({ "title": 1 })),
        vec!["schema.title must be a string"]
    );
    // The lossy annotation values of a `Value` are negative zero, alone or nested.
    for (key, value) in [
        ("default", json!(-0.0)),
        ("examples", json!([1, { "deep": -0.0 }])),
    ] {
        assert_eq!(
            v(json!({ key: value })),
            vec![format!(
                "schema.{key} annotation must be lossless JSON data"
            )]
        );
    }
}

#[test]
fn permits_sibling_reuse_and_rejects_exotic_nodes() {
    let leaf = json!({ "type": "string" });
    assert!(
        assert_supported_json_schema(
            &json!({ "type": "object", "properties": { "a": leaf, "b": leaf } })
        )
        .is_ok()
    );
    assert_eq!(
        v(json!({ "type": "object", "properties": { "at": "1970-01-01T00:00:00.000Z" } })),
        vec!["schema.properties.at must be a schema object"]
    );
}

#[test]
fn asserts_deeply_nested_raw_unions() {
    on_big_stack(|| {
        let mut schema = json!({ "type": "string" });
        for _ in 0..4_000 {
            schema = json!({ "oneOf": [schema, { "type": "null" }] });
        }
        assert!(assert_supported_json_schema(&schema).is_ok());
        std::mem::forget(schema);
    });
}

#[test]
fn uses_own_property_semantics_for_required_declarations() {
    assert_eq!(
        v(json!({ "type": "object", "properties": {}, "required": ["toString"] })),
        vec!["schema.required names \"toString\" which is not in properties"]
    );
}

#[test]
fn validate_validates_scalar_array_object_and_null_roots() {
    assert!(validate(&asserted(json!({ "type": "string" })), json!("x")).is_empty());
    assert!(validate(&asserted(json!({ "type": "number" })), json!(1.5)).is_empty());
    assert!(validate(&asserted(json!({ "type": "integer" })), json!(2)).is_empty());
    assert!(validate(&asserted(json!({ "type": "boolean" })), json!(true)).is_empty());
    assert!(validate(&asserted(json!({ "type": "null" })), Value::Null).is_empty());
    assert!(
        validate(
            &asserted(json!({ "type": "array", "items": { "type": "string" } })),
            json!(["x"])
        )
        .is_empty()
    );
    assert!(validate(&asserted(json!({ "type": "object" })), json!({ "x": 1 })).is_empty());
}

#[test]
fn validate_rejects_wrong_scalar_types_and_lossy_numbers() {
    assert_eq!(
        validate(&asserted(json!({ "type": "string" })), json!(1)),
        vec!["\"value\" must be a string"]
    );
    assert_eq!(
        validate(&asserted(json!({ "type": "number" })), json!("1")),
        vec!["\"value\" must be a number"]
    );
    assert_eq!(
        validate(&asserted(json!({ "type": "number" })), json!(-0.0)),
        vec!["\"value\" must be a finite JSON number"]
    );
    assert_eq!(
        validate(&asserted(json!({ "type": "integer" })), json!(1.5)),
        vec!["\"value\" must be an integer"]
    );
    assert_eq!(
        validate(&asserted(json!({ "type": "boolean" })), json!("true")),
        vec!["\"value\" must be a boolean"]
    );
    assert_eq!(
        validate(&asserted(json!({ "type": "null" })), json!(0)),
        vec!["\"value\" must be null"]
    );
}

#[test]
fn validate_enforces_scalar_enum_and_const_together() {
    let schema = asserted(json!({ "type": "string", "enum": ["a", "b"], "const": "a" }));
    assert!(validate(&schema, json!("a")).is_empty());
    assert_eq!(
        validate(&schema, json!("c")),
        vec!["\"value\" must be one of [\"a\",\"b\"]"]
    );
    assert_eq!(
        validate(&schema, json!("b")),
        vec!["\"value\" must be \"a\""]
    );
}

#[test]
fn validate_validates_object_requiredness_nested_values_and_raw_open_defaults() {
    let open = asserted(json!({
        "type": "object",
        "properties": {
            "file": { "type": "string" },
            "nested": {
                "type": "object",
                "properties": { "line": { "type": "integer" } },
                "required": ["line"],
                "additionalProperties": false,
            },
        },
        "required": ["file"],
    }));
    assert!(
        validate(
            &open,
            json!({ "file": "a", "extra": [1], "nested": { "line": 2 } })
        )
        .is_empty()
    );
    assert_eq!(
        validate(&open, json!({ "nested": { "line": 1 } })),
        vec!["missing required property \"value.file\""]
    );
    assert_eq!(
        validate(&open, json!({ "file": 1, "nested": {} })),
        vec![
            "\"value.file\" must be a string",
            "missing required property \"value.nested.line\"",
        ]
    );
    assert_eq!(
        validate(
            &open,
            json!({ "file": "a", "nested": { "line": 1, "extra": true } })
        ),
        vec!["\"value.nested.extra\" is not a declared property (additionalProperties: false)"]
    );
    assert_eq!(
        validate(&open, json!("x")),
        vec!["\"value\" must be an object"]
    );
}

#[test]
fn validate_rejects_lossy_nested_values() {
    let required =
        asserted(json!({ "type": "object", "properties": { "x": {} }, "required": ["x"] }));
    assert_eq!(
        validate(&required, json!({ "x": -0.0 })),
        vec!["\"value.x\" must be a lossless JSON value".to_owned()]
    );
    assert_eq!(
        validate(&asserted(json!({ "type": "object" })), json!({ "x": -0.0 })),
        vec!["\"value\" must be a lossless JSON object"]
    );
}

#[test]
fn validate_validates_dense_arrays_per_index() {
    let schema = asserted(json!({ "type": "array", "items": { "type": "integer" } }));
    assert!(validate(&schema, json!([1, 2])).is_empty());
    assert_eq!(
        validate(&schema, json!([1, 1.5])),
        vec!["\"value[1]\" must be an integer"]
    );
    assert_eq!(
        validate(&schema, json!("x")),
        vec!["\"value\" must be an array"]
    );
    let loose = asserted(json!({ "type": "array" }));
    assert_eq!(
        validate(&loose, json!([1, -0.0])),
        vec!["\"value\" must be a dense lossless JSON array"]
    );
}

#[test]
fn validate_validates_exact_one_one_of_semantics_including_overlap() {
    let disjoint = asserted(json!({ "oneOf": [{ "type": "string" }, { "type": "number" }] }));
    assert!(validate(&disjoint, json!("x")).is_empty());
    assert_eq!(
        validate(&disjoint, Value::Null),
        vec!["\"value\" must match exactly one oneOf branch (matched 0)"]
    );
    let overlap = asserted(json!({ "oneOf": [{ "type": "number" }, { "type": "integer" }] }));
    assert_eq!(
        validate(&overlap, json!(1)),
        vec!["\"value\" must match exactly one oneOf branch (matched 2)"]
    );
    assert!(validate(&overlap, json!(1.5)).is_empty());
}

#[test]
fn validate_validates_deeply_nested_exact_one_unions() {
    on_big_stack(|| {
        let mut schema = json!({ "type": "string" });
        for _ in 0..4_000 {
            schema = json!({ "oneOf": [schema, { "type": "null" }] });
        }
        assert_supported(&schema);
        assert!(validate(&schema, json!("leaf")).is_empty());
        assert_eq!(
            validate(&schema, json!(42)),
            vec!["\"value\" must match exactly one oneOf branch (matched 0)"]
        );
        std::mem::forget(schema);
    });
}

fn assert_supported(schema: &Value) {
    assert!(assert_supported_json_schema(schema).is_ok());
}

#[test]
fn validate_an_unconstrained_schema_accepts_only_lossless_json_values() {
    let any_json = asserted(json!({}));
    for value in [
        Value::Null,
        json!(true),
        json!(1),
        json!("x"),
        json!([1]),
        json!({ "x": null }),
    ] {
        assert!(validate(&any_json, value.clone()).is_empty(), "{value}");
    }
    assert_eq!(
        validate(&any_json, json!(-0.0)),
        vec!["\"value\" must be a lossless JSON value"]
    );
}

#[test]
fn validate_uses_own_properties_for_requiredness_recursion_and_closed_object_checks() {
    assert_eq!(
        validate(
            &asserted(json!({
                "type": "object",
                "properties": { "toString": { "type": "string" } },
                "required": ["toString"]
            })),
            json!({})
        ),
        vec!["missing required property \"value.toString\""]
    );
    assert_eq!(
        validate(
            &asserted(json!({ "type": "object", "additionalProperties": false })),
            json!({ "toString": 1 })
        ),
        vec!["\"value.toString\" is not a declared property (additionalProperties: false)"]
    );
    assert!(
        validate(
            &asserted(
                json!({ "type": "object", "properties": { "constructor": { "type": "string" } } })
            ),
            json!({})
        )
        .is_empty()
    );
    assert!(validate(&json!({ "type": "object", "properties": null }), json!({})).is_empty());
    assert!(validate(&json!({ "type": "object", "required": null }), json!({})).is_empty());
}

/// json-schema.spec.ts:451 "keeps assertNever as a forged-schema backstop": the validator throws.
#[test]
fn validate_keeps_assert_never_as_a_forged_schema_backstop() {
    let forged = json!({ "type": "tuple" });
    let panic = std::panic::catch_unwind(|| validate(&forged, json!(1))).expect_err("throws");
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .expect("text payload");
    assert_eq!(message, "unreachable variant in JsonSchemaType: \"tuple\"");
}

/// A frame that contains failures turns a forged descendant into its own generic diagnostic, reported at
/// the nearest valid ancestor's path (json-schema.ts `catches`).
#[test]
fn validate_reports_a_nested_forged_type_at_the_nearest_valid_ancestor() {
    let schema = json!({
        "type": "object",
        "properties": { "inner": { "type": "tuple" } },
    });
    assert_eq!(
        validate(&schema, json!({ "inner": 1 })),
        vec!["\"value\" must be a lossless JSON value"]
    );
}
