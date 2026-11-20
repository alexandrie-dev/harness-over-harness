use crate::redact::redact_secrets_at;
use serde_json::{Map, Value};
use std::collections::HashSet;

fn dereference<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|path| path.strip_prefix('#'))
        .and_then(|path| root.pointer(path))
        .unwrap_or(schema)
}

fn is_volatile(schema: &Value) -> bool {
    schema.get("x-volatile").and_then(Value::as_bool) == Some(true)
        || schema.pointer("/meta/volatile").and_then(Value::as_bool) == Some(true)
}

fn is_secret(schema: &Value) -> bool {
    schema.get("x-role").and_then(Value::as_str) == Some("secret")
        || schema.pointer("/meta/role").and_then(Value::as_str) == Some("secret")
}

fn plain_node(root: &Value, schema: &Value, depth: usize) -> Value {
    if depth > 128 {
        return schema.clone();
    }
    if schema.get("$ref").is_some() {
        let mut node = schema.clone();
        let Some(object) = node.as_object_mut() else {
            return node;
        };
        object.remove("x-volatile");
        if let Some(meta) = object.get_mut("meta").and_then(Value::as_object_mut) {
            meta.remove("volatile");
        }
        if is_secret(schema) || is_secret(dereference(root, schema)) {
            object.remove("default");
        } else if let Some(default) = object.get("default").cloned() {
            object.insert(
                "default".into(),
                redact_secrets_at(root, schema, &default).value,
            );
        }
        return node;
    }
    let mut node = schema.clone();
    let Some(object) = node.as_object_mut() else {
        return node;
    };
    object.remove("x-volatile");
    if let Some(meta) = object.get_mut("meta").and_then(Value::as_object_mut) {
        meta.remove("volatile");
    }
    if is_secret(schema) || is_secret(&Value::Object(object.clone())) {
        object.remove("default");
        if let Some(meta) = object.get_mut("meta").and_then(Value::as_object_mut) {
            meta.remove("default");
            meta.remove("required");
        }
    } else if let Some(default) = object.get("default").cloned() {
        object.insert(
            "default".into(),
            redact_secrets_at(root, schema, &default).value,
        );
    }
    if let Some(mut children) = object
        .remove("properties")
        .and_then(|value| value.as_object().cloned())
    {
        let secret_keys = children
            .iter()
            .filter(|(_, child)| is_secret(child) || is_secret(dereference(root, child)))
            .map(|(key, _)| key.clone())
            .collect::<HashSet<_>>();
        if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
            required.retain(|key| key.as_str().is_none_or(|key| !secret_keys.contains(key)));
        }
        for child in children.values_mut() {
            *child = plain_node(root, child, depth + 1);
        }
        object.insert("properties".into(), Value::Object(children));
    }
    if let Some(children) = object.get_mut("$defs").and_then(Value::as_object_mut) {
        for child in children.values_mut() {
            *child = plain_node(root, child, depth + 1);
        }
    }
    for key in ["items", "additionalProperties"] {
        if object.get(key).is_some_and(Value::is_object) {
            let child = object.get(key).cloned().unwrap_or(Value::Null);
            object.insert(key.into(), plain_node(root, &child, depth + 1));
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                *child = plain_node(root, child, depth + 1);
            }
        }
    }
    node
}

/// Select schema fields below the nearest `x-volatile` declaration.
pub fn volatile_form(schema: &Value) -> Option<Value> {
    fn select(root: &Value, node: &Value, depth: usize) -> Option<Value> {
        if depth > 128 {
            return None;
        }
        if is_volatile(node) {
            let expanded = if let Some(target) = node
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|path| path.strip_prefix('#'))
                .and_then(|path| root.pointer(path))
            {
                let mut expanded = target.clone();
                if let (Some(expanded), Some(siblings)) =
                    (expanded.as_object_mut(), node.as_object())
                {
                    for (key, value) in siblings {
                        if key != "$ref" {
                            expanded.insert(key.clone(), value.clone());
                        }
                    }
                }
                expanded
            } else {
                node.clone()
            };
            let mut form = plain_node(root, &expanded, depth + 1);
            if depth == 0
                && let Some(definitions) = root.get("$defs")
            {
                form["$defs"] =
                    plain_node(root, &serde_json::json!({"$defs":definitions}), depth + 1)["$defs"]
                        .clone();
            }
            return Some(form);
        }
        let node = dereference(root, node);
        if is_volatile(node) {
            let mut form = plain_node(root, node, depth + 1);
            if depth == 0
                && let Some(definitions) = root.get("$defs")
                && let Some(object) = form.as_object_mut()
            {
                object.insert(
                    "$defs".into(),
                    plain_node(root, &serde_json::json!({"$defs":definitions}), depth + 1)["$defs"]
                        .clone(),
                );
            }
            return Some(form);
        }
        let properties = node.get("properties")?.as_object()?;
        let mut selected = Map::new();
        for (key, child) in properties {
            if let Some(child) = select(root, child, depth + 1) {
                selected.insert(key.clone(), child);
            }
        }
        if selected.is_empty() {
            None
        } else {
            let mut form = serde_json::json!({"type":"object", "properties":selected});
            if depth == 0
                && let Some(definitions) = root.get("$defs")
            {
                form["$defs"] =
                    plain_node(root, &serde_json::json!({"$defs":definitions}), depth + 1)["$defs"]
                        .clone();
            }
            Some(form)
        }
    }
    select(schema, schema, 0)
}

fn is_expression(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == 1 && object.get("__jsExpr").is_some_and(Value::is_string)
    })
}

fn has_type(schema: &Value, expected: &str) -> bool {
    match schema.get("type") {
        Some(Value::String(actual)) => actual == expected,
        Some(Value::Array(actual)) => actual
            .iter()
            .any(|actual| actual.as_str() == Some(expected)),
        _ => false,
    }
}

/// Project only fields declared by a filtered form schema.
pub fn project_form(schema: &Value, value: &Value) -> Value {
    fn project(root: &Value, schema: &Value, value: &Value, depth: usize) -> Value {
        if depth > 128 {
            return value.clone();
        }
        let schema = dereference(root, schema);
        if schema.get("properties").is_none()
            && schema
                .get("additionalProperties")
                .is_some_and(Value::is_object)
        {
            return value.clone();
        }
        let object_like = has_type(schema, "object") || schema.get("properties").is_some();
        if object_like && value.is_object() && !is_expression(value) {
            let source = value.as_object().expect("checked object");
            let mut projected = Map::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (key, child) in properties {
                    if let Some(field) = source.get(key) {
                        projected.insert(key.clone(), project(root, child, field, depth + 1));
                    }
                }
            }
            return Value::Object(projected);
        }
        value.clone()
    }
    project(schema, schema, value, 0)
}

fn volatile_at(root: &Value, schema: &Value, path: &[String]) -> bool {
    if is_volatile(schema) {
        return true;
    }
    let schema = dereference(root, schema);
    if is_volatile(schema) {
        return true;
    }
    let Some((head, rest)) = path.split_first() else {
        return false;
    };
    let child = schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(head));
    child.is_some_and(|child| volatile_at(root, child, rest))
}

/// Whether a raw config path is below a volatile declaration.
pub fn is_volatile_path(schema: &Value, path: &[String]) -> bool {
    volatile_at(schema, schema, path)
}

/// Rust snapshots are already detached JSON; keep the named source operation explicit.
pub fn plain_config(value: &Value) -> Value {
    value.clone()
}

pub(crate) fn child_schema<'a>(root: &'a Value, schema: &'a Value, key: &str) -> Option<&'a Value> {
    let schema = dereference(root, schema);
    schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(key))
        .or_else(|| {
            strict_array_index(key)
                .and_then(|_| schema.get("items").filter(|value| value.is_object()))
        })
        .or_else(|| {
            schema
                .get("additionalProperties")
                .filter(|value| value.is_object())
        })
}

pub(crate) fn object_properties<'a>(
    root: &'a Value,
    schema: &'a Value,
) -> Option<&'a Map<String, Value>> {
    dereference(root, schema)
        .get("properties")
        .and_then(Value::as_object)
}

fn strict_array_index(key: &str) -> Option<usize> {
    if key == "0"
        || key
            .strip_prefix(|first: char| ('1'..='9').contains(&first))
            .is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_digit()))
    {
        key.parse().ok()
    } else {
        None
    }
}
