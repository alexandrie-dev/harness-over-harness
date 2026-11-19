//! Port of tests/schema.spec.ts. The five type-inference cases (`expectTypeOf`, `@ts-expect-error`)
//! check TypeScript types and have no Rust counterpart.

use hoh_tools::{
    JsonSchemaError, parameter_schema_spec_to_json_schema, value_schema_spec_to_json_schema,
};
use serde_json::{Map, Value, json};

fn value_schema(spec: Value) -> Value {
    value_schema_spec_to_json_schema(&spec).expect("spec compiles")
}

fn rejects_value(spec: Value) {
    let error = value_schema_spec_to_json_schema(&spec).expect_err(&spec.to_string());
    assert!(error.is_named(JsonSchemaError::NAME), "{spec}");
    assert_eq!(error.code(), "UNSUPPORTED_SCHEMA");
}

fn rejects_parameters(spec: Value) {
    let error = parameter_schema_spec_to_json_schema(&spec).expect_err(&spec.to_string());
    assert!(error.is_named(JsonSchemaError::NAME), "{spec}");
}

#[test]
fn compiles_every_value_root_and_the_author_only_json_node() {
    assert_eq!(
        value_schema(json!({ "type": "string", "enum": ["a", "b"], "const": "a" })),
        json!({ "type": "string", "enum": ["a", "b"], "const": "a" })
    );
    assert_eq!(
        value_schema(json!({ "type": "number" })),
        json!({ "type": "number" })
    );
    assert_eq!(
        value_schema(json!({ "type": "integer" })),
        json!({ "type": "integer" })
    );
    assert_eq!(
        value_schema(json!({ "type": "boolean" })),
        json!({ "type": "boolean" })
    );
    assert_eq!(
        value_schema(json!({ "type": "null" })),
        json!({ "type": "null" })
    );
    assert_eq!(
        value_schema(json!({ "type": "array", "items": { "type": "json" } })),
        json!({ "type": "array", "items": {} })
    );
    assert_eq!(
        value_schema(json!({ "type": "object", "additionalProperties": false, "properties": {} })),
        json!({ "type": "object", "additionalProperties": false, "properties": {} })
    );
    assert_eq!(
        value_schema(json!({
            "type": "json",
            "description": "anything",
            "title": "Any JSON",
            "default": null,
            "examples": [{ "nested": true }],
        })),
        json!({ "description": "anything", "title": "Any JSON", "default": null, "examples": [{ "nested": true }] })
    );
    assert_eq!(
        value_schema(json!({ "oneOf": [{ "type": "string" }, { "type": "null" }] })),
        json!({ "oneOf": [{ "type": "string" }, { "type": "null" }] })
    );
}

#[test]
fn keeps_the_implicit_parameter_root_open_while_preserving_explicit_object_openness() {
    let compiled = parameter_schema_spec_to_json_schema(&json!({
        "closed": {
            "type": "object",
            "additionalProperties": false,
            "required": true,
            "properties": { "id": { "type": "integer", "required": true } },
        },
        "open": { "type": "object", "additionalProperties": true },
    }))
    .expect("spec compiles");
    let expected = json!({
        "type": "object",
        "properties": {
            "closed": {
                "type": "object",
                "additionalProperties": false,
                "properties": { "id": { "type": "integer" } },
                "required": ["id"],
            },
            "open": { "type": "object", "additionalProperties": true },
        },
        "required": ["closed"],
    });
    assert_eq!(Value::Object(compiled.clone()), expected);
    // Key order is part of the model-visible schema.
    assert_eq!(
        serde_json::to_string(&compiled).expect("serializes"),
        serde_json::to_string(&expected).expect("serializes")
    );
}

#[test]
fn rejects_runtime_forged_author_forms_rather_than_compiling_them_lossily() {
    for spec in [
        json!({ "type": "object" }),
        json!({ "oneOf": [{ "type": "string" }] }),
        json!({ "type": "number", "enum": ["1"] }),
        json!({ "type": "string", "enum": ["a"], "const": "b" }),
        json!({ "type": "integer", "const": 1.5 }),
        // `default: undefined` of the source: the lossy value of a `Value` is negative zero.
        json!({ "type": "json", "default": -0.0 }),
        json!({ "type": "array", "items": { "type": "string", "required": true } }),
        json!({ "type": "array", "items": 42 }),
        json!({ "type": "string", "extra": true }),
        json!({ "type": "string", "oneOf": [{ "type": "string" }, { "type": "null" }] }),
        json!({ "oneOf": "not-an-array" }),
        json!({ "type": "string", "enum": "a" }),
        json!({}),
        Value::Null,
    ] {
        rejects_value(spec);
    }
    rejects_parameters(json!({ "value": { "type": "string", "required": false } }));
    rejects_parameters(Value::Null);
    rejects_parameters(json!({ "bad": 42 }));
}

#[test]
fn compiles_deeply_nested_author_unions() {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let depth = 4_000;
            let mut spec = json!({ "type": "string" });
            for _ in 0..depth {
                spec = json!({ "oneOf": [spec, { "type": "null" }] });
            }
            let compiled = value_schema(spec.clone());
            let mut cursor = &compiled;
            let mut layers = 0;
            while let Some(branches) = cursor.get("oneOf") {
                cursor = &branches[0];
                layers += 1;
            }
            assert_eq!(layers, depth);
            assert_eq!(cursor, &json!({ "type": "string" }));
            // The deep values are dropped by the OS with the thread; their recursive drop is not under test.
            std::mem::forget(spec);
            std::mem::forget(compiled);
        })
        .expect("thread spawns")
        .join()
        .expect("test thread completes");
}

#[test]
fn preserves_a_property_literally_named_proto_as_schema_data() {
    let mut properties = Map::new();
    properties.insert(
        "__proto__".into(),
        json!({ "type": "string", "required": true }),
    );
    let schema =
        parameter_schema_spec_to_json_schema(&Value::Object(properties)).expect("compiles");
    assert!(
        schema["properties"]
            .as_object()
            .expect("properties")
            .contains_key("__proto__")
    );
    assert_eq!(
        schema["properties"]["__proto__"],
        json!({ "type": "string" })
    );
    assert_eq!(schema["required"], json!(["__proto__"]));
}
