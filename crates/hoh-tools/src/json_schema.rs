//! Enforced JSON Schema subset shared by tool outputs, generated PTC mode
//! types, subagents, and workflows. The subset accepts any JSON root, an
//! annotation-only schema for unconstrained JSON, one scalar `type`, object
//! `properties`/`required`/boolean `additionalProperties`, array `items`,
//! type-correct scalar `enum`/`const`, and exact-one `oneOf`.
//!
//! Unsupported or misplaced keywords reject rather than being accepted without
//! enforcement. Consumers that require an object root apply
//! [`assert_object_json_schema`] before accepting input.
//!
//! A schema node is a plain [`serde_json::Value`]: the source's checks of realm,
//! prototype, symbol keys, getters and cycles guard JavaScript object graphs that a
//! `Value` cannot express, so only the keyword rules remain.

use hoh_error::HarnessError;
use hoh_util_values::{JsonValue, deep_equal_json, is_json_value, js_json_stringify};
use serde_json::{Map, Value};

/// Single-type keywords accepted by the enforced subset.
const SCHEMA_TYPES: [&str; 7] = [
    "object", "array", "string", "number", "integer", "boolean", "null",
];

const CONSTRAINT_KEYWORDS: [&str; 8] = [
    "type",
    "oneOf",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "const",
];
const ANNOTATION_KEYWORDS: [&str; 4] = ["description", "title", "default", "examples"];
/// Keywords that are invalid beside `oneOf`.
const ONE_OF_SIBLING_KEYWORDS: [&str; 6] = [
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "const",
];

/// One raw JSON Schema node in the enforced subset; [`assert_supported_json_schema`] rejects
/// invalid combinations before a caller treats the node as trusted.
pub type JsonSchemaNode = JsonValue;

/// The `violations` list of a `JsonSchemaError`, attached with `HarnessError::with_detail`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonSchemaViolations(pub Vec<String>);

/// Name of `JsonSchemaError`.
pub const JSON_SCHEMA_ERROR_NAME: &str = "JsonSchemaError";

/// Thrown when a raw schema falls outside the enforced subset. The violations list every
/// offending path instead of stopping at the first author error.
pub struct JsonSchemaError;

impl JsonSchemaError {
    pub const NAME: &'static str = JSON_SCHEMA_ERROR_NAME;
    pub const CODE: &'static str = "UNSUPPORTED_SCHEMA";

    pub fn new(violations: Vec<String>) -> HarnessError {
        HarnessError::named(
            Self::NAME,
            format!("unsupported JSON schema: {}", violations.join("; ")),
            Self::CODE,
        )
        .with_detail(JsonSchemaViolations(violations))
    }

    /// The violations an error of this class carries.
    pub fn violations(error: &HarnessError) -> Option<&[String]> {
        error
            .detail::<JsonSchemaViolations>()
            .map(|detail| detail.0.as_slice())
    }
}

/// `isPlainJsonRecord`: whether a candidate is a plain JSON record. A `Value` has no prototype chain, so
/// every JSON object qualifies; arrays and scalars do not.
pub fn is_plain_json_record(value: &Value) -> bool {
    value.is_object()
}

/// `isJsonSchemaRecord`: a plain record whose keys survive JSON projection. A `Value` object has only
/// own enumerable string keys, so it coincides with [`is_plain_json_record`].
pub fn is_json_schema_record(value: &Value) -> bool {
    is_plain_json_record(value)
}

/// `isPlainJsonArray`: whether a candidate is a dense, undecorated array. A `Value` array is always so.
pub fn is_plain_json_array(value: &Value) -> bool {
    value.is_array()
}

/// JavaScript `Object.keys` order: array-index keys ascending first, then insertion order.
pub(crate) fn js_keys(map: &Map<String, Value>) -> Vec<&String> {
    let mut indices: Vec<(u32, &String)> = Vec::new();
    let mut rest: Vec<&String> = Vec::new();
    for key in map.keys() {
        match array_index(key) {
            Some(index) => indices.push((index, key)),
            None => rest.push(key),
        }
    }
    indices.sort_by_key(|(index, _)| *index);
    indices
        .into_iter()
        .map(|(_, key)| key)
        .chain(rest)
        .collect()
}

/// `Object.entries` order, see [`js_keys`].
pub(crate) fn js_entries(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    js_keys(map)
        .into_iter()
        .filter_map(|key| map.get(key).map(|value| (key, value)))
        .collect()
}

fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: u64 = key.parse().ok()?;
    (value < u32::MAX as u64).then_some(value as u32)
}

/// Lossless finite JSON number, excluding negative zero.
fn is_json_number(value: &Value) -> bool {
    match value {
        Value::Number(number) => {
            !(number.is_f64()
                && number
                    .as_f64()
                    .is_some_and(|f| f == 0.0 && f.is_sign_negative()))
        }
        _ => false,
    }
}

/// `Number.isInteger` over a JSON number.
fn is_integer(value: &Value) -> bool {
    match value {
        Value::Number(number) => {
            number.is_i64()
                || number.is_u64()
                || number
                    .as_f64()
                    .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
        }
        _ => false,
    }
}

/// Whether a scalar is valid for one declared schema type.
fn scalar_matches(schema_type: &str, value: &Value) -> bool {
    match schema_type {
        "string" => value.is_string(),
        "number" => is_json_number(value),
        "integer" => is_json_number(value) && is_integer(value),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn has(node: &Map<String, Value>, key: &str) -> bool {
    node.contains_key(key)
}

/// Validate object-only fields after its property schemas have been visited.
fn check_object_schema_tail(
    node: &Map<String, Value>,
    path: &str,
    properties: Option<&Value>,
    violations: &mut Vec<String>,
) {
    if let Some(required) = node.get("required") {
        let names: Option<Vec<&str>> = required
            .as_array()
            .and_then(|items| items.iter().map(Value::as_str).collect());
        match names {
            None => violations.push(format!("{path}.required must be an array of strings")),
            Some(names) => {
                let declared = properties.and_then(Value::as_object);
                for key in names {
                    if !declared.is_some_and(|declared| declared.contains_key(key)) {
                        violations.push(format!(
                            "{path}.required names \"{key}\" which is not in properties"
                        ));
                    }
                }
            }
        }
    }
    if node
        .get("additionalProperties")
        .is_some_and(|value| !value.is_boolean())
    {
        violations.push(format!("{path}.additionalProperties must be a boolean"));
    }
}

/// Collect every violation for one raw schema tree.
fn check_schema_node(node: &Value, path: &str, violations: &mut Vec<String>) {
    let Some(node) = node.as_object() else {
        violations.push(format!("{path} must be a schema object"));
        return;
    };

    for key in js_keys(node) {
        if CONSTRAINT_KEYWORDS.contains(&key.as_str()) {
            continue;
        }
        if ANNOTATION_KEYWORDS.contains(&key.as_str()) {
            if !is_json_value(&node[key]) {
                violations.push(format!(
                    "{path}.{key} annotation must be lossless JSON data"
                ));
            }
            continue;
        }
        violations.push(format!(
            "{path}.{key} is not a supported keyword (subset: type/oneOf/properties/required/additionalProperties/items/enum/const + annotations)"
        ));
    }
    if node.get("description").is_some_and(|v| !v.is_string()) {
        violations.push(format!("{path}.description must be a string"));
    }
    if node.get("title").is_some_and(|v| !v.is_string()) {
        violations.push(format!("{path}.title must be a string"));
    }

    let has_type = has(node, "type");
    let has_one_of = has(node, "oneOf");
    if has_type && has_one_of {
        violations.push(format!("{path} cannot declare both type and oneOf"));
        return;
    }
    if !has_type && !has_one_of {
        for key in ONE_OF_SIBLING_KEYWORDS {
            if has(node, key) {
                violations.push(format!("{path}.{key} requires type or oneOf"));
            }
        }
        return;
    }

    if has_one_of {
        match node["oneOf"].as_array() {
            Some(branches) if branches.len() >= 2 => {
                for (index, branch) in branches.iter().enumerate() {
                    check_schema_node(branch, &format!("{path}.oneOf[{index}]"), violations);
                }
            }
            _ => violations.push(format!(
                "{path}.oneOf must be an array of at least two schemas"
            )),
        }
        for key in ONE_OF_SIBLING_KEYWORDS {
            if has(node, key) {
                violations.push(format!("{path}.{key} is not supported beside oneOf"));
            }
        }
        return;
    }

    let schema_type = match node["type"].as_str() {
        Some(schema_type) if SCHEMA_TYPES.contains(&schema_type) => schema_type,
        _ => {
            violations.push(if node["type"].is_array() {
                format!("{path}.type must be a single type string (type arrays are not supported)")
            } else {
                format!("{path}.type must be one of {}", SCHEMA_TYPES.join("/"))
            });
            return;
        }
    };
    let allowed_for: [(&str, &[&str]); 6] = [
        ("properties", &["object"]),
        ("required", &["object"]),
        ("additionalProperties", &["object"]),
        ("items", &["array"]),
        ("enum", &["string", "number", "integer", "boolean", "null"]),
        ("const", &["string", "number", "integer", "boolean", "null"]),
    ];
    for (key, types) in allowed_for {
        if has(node, key) && !types.contains(&schema_type) {
            violations.push(format!(
                "{path}.{key} is not supported on type \"{schema_type}\""
            ));
        }
    }

    match schema_type {
        "object" => {
            let properties = node.get("properties");
            if let Some(properties) = properties {
                match properties.as_object() {
                    None => {
                        violations.push(format!("{path}.properties must be an object of schemas"))
                    }
                    Some(properties) => {
                        for (key, child) in js_entries(properties) {
                            check_schema_node(
                                child,
                                &format!("{path}.properties.{key}"),
                                violations,
                            );
                        }
                    }
                }
            }
            check_object_schema_tail(node, path, properties, violations);
        }
        "array" => {
            if let Some(items) = node.get("items") {
                check_schema_node(items, &format!("{path}.items"), violations);
            }
        }
        _ => {
            let allowed = node.get("enum").and_then(Value::as_array);
            let enum_valid = allowed.is_some_and(|allowed| {
                !allowed.is_empty()
                    && allowed
                        .iter()
                        .all(|entry| scalar_matches(schema_type, entry))
            });
            if has(node, "enum") && !enum_valid {
                violations.push(format!(
                    "{path}.enum must be a non-empty array of {schema_type} values"
                ));
            }
            if let Some(declared) = node.get("const") {
                if !scalar_matches(schema_type, declared) {
                    violations.push(format!("{path}.const must be a {schema_type} value"));
                } else if enum_valid
                    && !allowed.is_some_and(|allowed| {
                        allowed.iter().any(|entry| deep_equal_json(entry, declared))
                    })
                {
                    violations.push(format!(
                        "{path}.const must be one of {path}.enum when both are declared"
                    ));
                }
            }
        }
    }
}

/// Assert that an arbitrary raw schema uses only the enforced subset.
/// Annotation-only schemas are accepted as the standard unconstrained-JSON
/// form; callers that require an object root use [`assert_object_json_schema`].
pub fn assert_supported_json_schema(schema: &Value) -> Result<(), HarnessError> {
    let mut violations = Vec::new();
    check_schema_node(schema, "schema", &mut violations);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(JsonSchemaError::new(violations))
    }
}

/// Assert the enforced subset plus the object-root constraint retained by
/// subagent and workflow structured outputs.
pub fn assert_object_json_schema(schema: &Value) -> Result<(), HarnessError> {
    let mut violations = Vec::new();
    check_schema_node(schema, "schema", &mut violations);
    if violations.is_empty()
        && !schema
            .as_object()
            .is_some_and(|node| node.get("type").and_then(Value::as_str) == Some("object"))
    {
        violations
            .push("schema.type must be \"object\" (structured output is object-rooted)".into());
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(JsonSchemaError::new(violations))
    }
}

/// Root-aware diagnostic path for the parameter validator's empty sentinel.
fn diagnostic_path(path: &str) -> &str {
    if path.is_empty() { "arguments" } else { path }
}

/// Append one object property without a leading dot at an implicit root.
fn property_path(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn lossless_value_violation(path: &str) -> Vec<String> {
    vec![format!(
        "\"{}\" must be a lossless JSON value",
        diagnostic_path(path)
    )]
}

/// Validate one scalar node after its primitive type check.
fn check_scalar_value(node: &Map<String, Value>, value: &Value, path: &str) -> Vec<String> {
    if let Some(allowed) = node.get("enum").and_then(Value::as_array)
        && !allowed.iter().any(|entry| deep_equal_json(entry, value))
    {
        return vec![format!(
            "\"{}\" must be one of {}",
            diagnostic_path(path),
            js_json_stringify(&Value::Array(allowed.clone()))
        )];
    }
    if let Some(constant) = node.get("const")
        && !deep_equal_json(constant, value)
    {
        return vec![format!(
            "\"{}\" must be {}",
            diagnostic_path(path),
            js_json_stringify(constant)
        )];
    }
    Vec::new()
}

/// A schema node whose `type` lies outside the closed table: the source's `assertNever` backstop throws.
struct Forged(String);

/// Whether a node contains the failures below it: a node whose `type` is absent or in the closed table
/// turns a failure raised inside it into its own generic lossless-value diagnostic.
fn contains_failures(schema: &Value) -> bool {
    match schema.as_object().and_then(|node| node.get("type")) {
        None => true,
        Some(Value::String(node_type)) => SCHEMA_TYPES.contains(&node_type.as_str()),
        Some(_) => false,
    }
}

/// One validation frame. A failure inside a frame that contains failures is replaced by that frame's
/// lossless-value diagnostic; otherwise it unwinds to the nearest enclosing frame that does.
fn check_value(schema: &Value, value: &Value, path: &str) -> Result<Vec<String>, Forged> {
    match check_node(schema, value, path) {
        Err(_) if contains_failures(schema) => Ok(lossless_value_violation(path)),
        result => result,
    }
}

fn check_node(schema: &Value, value: &Value, path: &str) -> Result<Vec<String>, Forged> {
    let Some(node) = schema.as_object() else {
        return Ok(lossless_value_violation(path));
    };
    if let Some(branches) = node.get("oneOf").and_then(Value::as_array) {
        let mut matches = 0_usize;
        for branch in branches {
            if check_value(branch, value, path)?.is_empty() {
                matches += 1;
            }
        }
        return Ok(if matches == 1 {
            Vec::new()
        } else {
            vec![format!(
                "\"{}\" must match exactly one oneOf branch (matched {matches})",
                diagnostic_path(path)
            )]
        });
    }
    let Some(node_type) = node.get("type") else {
        return Ok(if is_json_value(value) {
            Vec::new()
        } else {
            lossless_value_violation(path)
        });
    };
    let shown = diagnostic_path(path);
    Ok(match node_type.as_str() {
        Some("object") => {
            let Some(object) = value.as_object() else {
                return Ok(vec![format!("\"{shown}\" must be an object")]);
            };
            let empty = Map::new();
            let properties = node
                .get("properties")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            let mut violations = Vec::new();
            if let Some(required) = node.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(key) {
                        violations.push(format!(
                            "missing required property \"{}\"",
                            property_path(path, key)
                        ));
                    }
                }
            }
            for (key, child) in js_entries(properties) {
                if let Some(entry) = object.get(key) {
                    violations.extend(check_value(child, entry, &property_path(path, key))?);
                }
            }
            if node.get("additionalProperties") == Some(&Value::Bool(false)) {
                for key in js_keys(object) {
                    if !properties.contains_key(key) {
                        violations.push(format!(
                            "\"{}\" is not a declared property (additionalProperties: false)",
                            property_path(path, key)
                        ));
                    }
                }
            }
            if violations.is_empty() && !is_json_value(value) {
                return Ok(vec![format!("\"{shown}\" must be a lossless JSON object")]);
            }
            violations
        }
        Some("array") => {
            let Some(items) = value.as_array() else {
                return Ok(vec![format!("\"{shown}\" must be an array")]);
            };
            let mut violations = Vec::new();
            if let Some(item_schema) = node.get("items") {
                for (index, entry) in items.iter().enumerate() {
                    violations.extend(check_value(
                        item_schema,
                        entry,
                        &format!("{path}[{index}]"),
                    )?);
                }
            }
            if violations.is_empty() && !is_json_value(value) {
                return Ok(vec![format!(
                    "\"{shown}\" must be a dense lossless JSON array"
                )]);
            }
            violations
        }
        Some("string") => {
            if value.is_string() {
                check_scalar_value(node, value, path)
            } else {
                vec![format!("\"{shown}\" must be a string")]
            }
        }
        Some("number") => {
            if !value.is_number() {
                vec![format!("\"{shown}\" must be a number")]
            } else if !is_json_number(value) {
                vec![format!("\"{shown}\" must be a finite JSON number")]
            } else {
                check_scalar_value(node, value, path)
            }
        }
        Some("integer") => {
            if !is_json_number(value) || !is_integer(value) {
                vec![format!("\"{shown}\" must be an integer")]
            } else {
                check_scalar_value(node, value, path)
            }
        }
        Some("boolean") => {
            if value.is_boolean() {
                check_scalar_value(node, value, path)
            } else {
                vec![format!("\"{shown}\" must be a boolean")]
            }
        }
        Some("null") => {
            if value.is_null() {
                check_scalar_value(node, value, path)
            } else {
                vec![format!("\"{shown}\" must be null")]
            }
        }
        // A forged schema outside the closed type table: the source's `assertNever` backstop throws and the
        // nearest valid ancestor reports the generic diagnostic. A `Value` schema reaches this only unasserted.
        _ => return Err(Forged(js_json_stringify(node_type))),
    })
}

/// Validate a candidate value against an asserted raw schema. The function is
/// total for arbitrary values and returns path-qualified violations in walk order;
/// empty means valid. `path` labels the root in diagnostics (the source default is `"value"`).
///
/// # Panics
/// With `unreachable variant in JsonSchemaType: …` when the schema was not asserted and its root (or a
/// chain of nodes up to the root) carries a `type` outside the closed table: the source's `assertNever`
/// backstop throws there.
pub fn validate_json_schema_value(
    schema: &JsonSchemaNode,
    value: &Value,
    path: &str,
) -> Vec<String> {
    match check_value(schema, value, path) {
        Ok(violations) => violations,
        // The source throws; an asserted schema never reaches this arm.
        Err(Forged(rendered)) => panic!("unreachable variant in JsonSchemaType: {rendered}"),
    }
}
