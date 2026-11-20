use crate::types::SettingsSecretView;
use indexmap::IndexMap;
use serde_json::{Map, Value};

/// One schema-declared secret position, matching the source export.
pub type RedactedSecret = SettingsSecretView;

#[derive(Clone, Debug, PartialEq)]
pub struct RedactedValue {
    pub value: Value,
    pub secrets: Vec<RedactedSecret>,
}

fn role_secret(schema: &Value) -> bool {
    schema.get("x-role").and_then(Value::as_str) == Some("secret")
        || schema.pointer("/meta/role").and_then(Value::as_str) == Some("secret")
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

fn reference<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|path| path.strip_prefix('#'))
        .and_then(|path| root.pointer(path))
        .unwrap_or(schema)
}

fn walk(
    root: &Value,
    schema: &Value,
    value: &Value,
    present: bool,
    path: &mut Vec<String>,
    secrets: &mut Vec<SettingsSecretView>,
    depth: usize,
) -> Option<Value> {
    if depth > 128 {
        return Some(value.clone());
    }
    let direct_secret = role_secret(schema);
    let schema = reference(root, schema);
    if direct_secret || role_secret(schema) {
        secrets.push(SettingsSecretView {
            path: path.clone(),
            set: present,
        });
        return None;
    }
    let mut branches_seen = false;
    let mut current = Some(value.clone());
    for key in ["anyOf", "oneOf", "allOf"] {
        let Some(branches) = schema.get(key).and_then(Value::as_array) else {
            continue;
        };
        branches_seen = true;
        for branch in branches {
            if let Some(input) = current.as_ref() {
                current = walk(root, branch, input, present, path, secrets, depth + 1);
            } else {
                let _ = walk(root, branch, &Value::Null, false, path, secrets, depth + 1);
            }
        }
    }
    if branches_seen {
        return current;
    }
    let object_like = has_type(schema, "object") || schema.get("properties").is_some();
    if object_like {
        let source = value.as_object();
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let additional = schema.get("additionalProperties").filter(|v| v.is_object());
        let mut rebuilt = Map::new();
        if let Some(source) = source {
            for (key, entry) in source {
                if properties.contains_key(key) {
                    continue;
                }
                path.push(key.clone());
                let stripped = match additional {
                    Some(child) => walk(root, child, entry, true, path, secrets, depth + 1),
                    None => Some(entry.clone()),
                };
                path.pop();
                if let Some(stripped) = stripped {
                    rebuilt.insert(key.clone(), stripped);
                }
            }
        }
        for (key, child) in properties {
            let has = source.is_some_and(|source| source.contains_key(&key));
            let entry = source
                .and_then(|source| source.get(&key))
                .unwrap_or(&Value::Null);
            path.push(key.clone());
            if let Some(stripped) = walk(root, &child, entry, has, path, secrets, depth + 1)
                && has
            {
                rebuilt.insert(key, stripped);
            }
            path.pop();
        }
        return if source.is_none() && rebuilt.is_empty() {
            Some(value.clone())
        } else {
            Some(Value::Object(rebuilt))
        };
    }
    if has_type(schema, "array") {
        let Some(source) = value.as_array() else {
            return Some(value.clone());
        };
        let Some(items) = schema.get("items").filter(|v| v.is_object()) else {
            return Some(value.clone());
        };
        return Some(Value::Array(
            source
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    path.push(index.to_string());
                    let stripped = walk(root, items, entry, true, path, secrets, depth + 1)
                        .unwrap_or(Value::Null);
                    path.pop();
                    stripped
                })
                .collect(),
        ));
    }
    Some(value.clone())
}

/// Remove `x-role: secret` values and return their concrete schema positions.
pub fn redact_secrets(schema: &Value, value: &Value) -> RedactedValue {
    redact_secrets_at(schema, schema, value)
}

pub(crate) fn redact_secrets_at(root: &Value, schema: &Value, value: &Value) -> RedactedValue {
    let mut secrets = vec![];
    let stripped =
        walk(root, schema, value, true, &mut vec![], &mut secrets, 0).unwrap_or(Value::Null);
    let mut positions = IndexMap::<Vec<String>, bool>::new();
    for secret in secrets {
        positions
            .entry(secret.path)
            .and_modify(|set| *set |= secret.set)
            .or_insert(secret.set);
    }
    RedactedValue {
        value: stripped,
        secrets: positions
            .into_iter()
            .map(|(path, set)| SettingsSecretView { path, set })
            .collect(),
    }
}
