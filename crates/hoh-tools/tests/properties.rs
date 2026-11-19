//! Port of tests/properties.spec.ts. The source drives the cases with fast-check; the workspace
//! has no property-testing library, so a fixed-seed generator produces the same families of specs
//! and inputs (leaf, enum/const, oneOf, nested object and array levels) deterministically.

use hoh_tools::{parameter_schema_spec_to_json_schema, validate_args};
use serde_json::{Map, Value, json};

/// SplitMix64: a small deterministic generator, enough to explore the spec space.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn flip(&mut self) -> bool {
        self.below(2) == 0
    }

    fn string(&mut self, max: u64) -> String {
        let length = 1 + self.below(max);
        (0..length)
            .map(|_| char::from(b'a' + self.below(26) as u8))
            .collect()
    }
}

fn with_required(mut prop: Value, rng: &mut Rng) -> Value {
    if rng.flip() {
        prop["required"] = json!(true);
    }
    prop
}

/// A leaf prop (no nesting) with optional required/enum.
fn leaf_prop(rng: &mut Rng) -> Value {
    let prop = match rng.below(9) {
        0 => json!({ "type": "string" }),
        1 => json!({ "type": "number" }),
        2 => json!({ "type": "integer" }),
        3 => json!({ "type": "boolean" }),
        4 => json!({ "type": "null" }),
        5 => json!({ "type": "json" }),
        6 => {
            let mut values: Vec<String> = Vec::new();
            for _ in 0..=rng.below(3) {
                let value = rng.string(5);
                if !values.contains(&value) {
                    values.push(value);
                }
            }
            json!({ "type": "string", "enum": values })
        }
        7 => json!({ "type": "string", "const": rng.string(5) }),
        _ => json!({ "oneOf": [{ "type": "string" }, { "type": "null" }] }),
    };
    with_required(prop, rng)
}

/// A prop up to `depth` levels of nesting (objects and arrays).
fn prop(rng: &mut Rng, depth: u32) -> Value {
    if depth == 0 {
        return leaf_prop(rng);
    }
    match rng.below(5) {
        0..=2 => leaf_prop(rng),
        3 => {
            let prop = json!({
                "type": "object",
                "additionalProperties": rng.flip(),
                "properties": spec(rng, depth - 1),
            });
            with_required(prop, rng)
        }
        _ => {
            let mut items = prop(rng, depth - 1);
            // Remove parameter-only requiredness before nesting a schema as an array item.
            items.as_object_mut().expect("object").remove("required");
            with_required(json!({ "type": "array", "items": items }), rng)
        }
    }
}

fn spec(rng: &mut Rng, depth: u32) -> Value {
    let mut map = Map::new();
    for _ in 0..rng.below(5) {
        map.insert(rng.string(6), prop(rng, depth));
    }
    Value::Object(map)
}

/// A value that satisfies a prop (used to build valid args).
fn value_for_prop(prop: &Value, rng: &mut Rng) -> Value {
    if let Some(branches) = prop.get("oneOf").and_then(Value::as_array) {
        return value_for_prop(&branches[rng.below(branches.len() as u64) as usize], rng);
    }
    if let Some(constant) = prop.get("const") {
        return constant.clone();
    }
    match prop["type"].as_str().expect("type") {
        "string" => match prop.get("enum").and_then(Value::as_array) {
            Some(values) => values[rng.below(values.len() as u64) as usize].clone(),
            None => json!(rng.string(8)),
        },
        "number" => json!(rng.below(1_000) as f64 / 8.0 + 0.5),
        "integer" => json!(rng.below(2_000) as i64 - 1_000),
        "boolean" => json!(rng.flip()),
        "null" => Value::Null,
        "object" => match prop.get("properties") {
            Some(properties) => valid_args_for_spec(properties, rng),
            None => json!({}),
        },
        "array" => match prop.get("items") {
            Some(items) => Value::Array(
                (0..rng.below(4))
                    .map(|_| value_for_prop(items, rng))
                    .collect(),
            ),
            None => json!([]),
        },
        "json" => match rng.below(4) {
            0 => Value::Null,
            1 => json!(rng.string(4)),
            2 => json!([1, "x", { "k": null }]),
            _ => json!({ "k": rng.below(10) }),
        },
        other => panic!("unexpected type {other}"),
    }
}

/// Args satisfying every required key of a spec (optionals included randomly).
fn valid_args_for_spec(spec: &Value, rng: &mut Rng) -> Value {
    let mut out = Map::new();
    for (key, prop) in spec.as_object().expect("spec") {
        let required = prop.get("required") == Some(&json!(true));
        // required keys are always present; optional keys are present about half the time
        if required || rng.flip() {
            out.insert(key.clone(), value_for_prop(prop, rng));
        }
    }
    Value::Object(out)
}

/// The `required: true` keys at the top level of a spec.
fn required_keys(spec: &Value) -> Vec<String> {
    spec.as_object()
        .expect("spec")
        .iter()
        .filter(|(_, prop)| prop.get("required") == Some(&json!(true)))
        .map(|(key, _)| key.clone())
        .collect()
}

const CASES: u32 = 300;

fn check_level(spec: &Value, compiled: &Value) {
    let compiled_required: std::collections::BTreeSet<String> = compiled
        .get("required")
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .map(|key| key.as_str().expect("string").to_owned())
                .collect()
        })
        .unwrap_or_default();
    let expected: std::collections::BTreeSet<String> = required_keys(spec).into_iter().collect();
    assert_eq!(compiled_required, expected);
    for (key, prop) in spec.as_object().expect("spec") {
        if prop["type"] == "object"
            && let Some(properties) = prop.get("properties")
        {
            check_level(properties, &compiled["properties"][key]);
        }
    }
}

#[test]
fn json_schema_required_equals_the_required_true_keys_at_every_level() {
    let mut rng = Rng(1);
    for _ in 0..CASES {
        let spec = spec(&mut rng, 2);
        let compiled =
            Value::Object(parameter_schema_spec_to_json_schema(&spec).expect("compiles"));
        check_level(&spec, &compiled);
    }
}

#[test]
fn conversion_is_total_never_fails_for_any_spec() {
    let mut rng = Rng(2);
    for _ in 0..CASES {
        let spec = spec(&mut rng, 3);
        assert!(
            parameter_schema_spec_to_json_schema(&spec).is_ok(),
            "{spec}"
        );
    }
}

#[test]
fn validate_args_is_total_never_fails_for_any_spec_and_any_input() {
    let mut rng = Rng(3);
    let inputs = [
        Value::Null,
        json!(1),
        json!("x"),
        json!([1, 2]),
        json!({}),
        json!({ "a": null, "b": [{}], "c": { "d": -0.0 } }),
    ];
    for _ in 0..CASES {
        let spec = spec(&mut rng, 2);
        for input in &inputs {
            assert!(validate_args(&spec, input).is_ok(), "{spec} {input}");
        }
    }
}

#[test]
fn args_satisfying_the_spec_pass_validate_args() {
    let mut rng = Rng(4);
    for _ in 0..CASES {
        let spec = spec(&mut rng, 2);
        let args = valid_args_for_spec(&spec, &mut rng);
        assert_eq!(
            validate_args(&spec, &args).expect("validates"),
            Vec::<String>::new(),
            "{spec} {args}"
        );
    }
}

#[test]
fn dropping_a_required_key_is_always_rejected() {
    let mut rng = Rng(5);
    let mut checked = 0;
    while checked < CASES {
        let spec = spec(&mut rng, 1);
        let required = required_keys(&spec);
        let Some(victim) = required.first() else {
            continue;
        };
        checked += 1;
        let mut args = valid_args_for_spec(&spec, &mut rng);
        args.as_object_mut().expect("object").remove(victim);
        let violations = validate_args(&spec, &args).expect("validates");
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains(&format!("\"{victim}\""))),
            "{spec} {args} {violations:?}"
        );
    }
}

#[test]
fn a_non_object_top_level_is_always_rejected() {
    let mut rng = Rng(6);
    for _ in 0..CASES {
        let spec = spec(&mut rng, 1);
        let not_an_object = match rng.below(5) {
            0 => json!(rng.string(5)),
            1 => json!(rng.below(100)),
            2 => json!(rng.flip()),
            3 => Value::Null,
            _ => json!([1, "x"]),
        };
        assert!(
            !validate_args(&spec, &not_an_object)
                .expect("validates")
                .is_empty()
        );
    }
}
