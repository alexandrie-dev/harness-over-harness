//! JSON_SCHEMA YAML boundary; the loader's general reader is crate-private.
use crate::Result;
use saphyr_parser::{Event, Parser, ScalarStyle};
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};

pub fn parse(text: &str, expressions: bool) -> Result<Value> {
    let mut events = VecDeque::new();
    for event in Parser::new_from_str(text) {
        events.push_back(event.map_err(|e| e.to_string())?.0);
    }
    if !matches!(events.pop_front(), Some(Event::StreamStart)) {
        return Err("missing YAML stream".into());
    }
    if matches!(events.front(), Some(Event::StreamEnd)) {
        return Ok(Value::Null);
    }
    events.pop_front();
    fn node<'a>(
        events: &mut VecDeque<Event<'a>>,
        anchors: &mut HashMap<usize, Value>,
        depth: usize,
        expressions: bool,
    ) -> Result<Value> {
        if depth > 128 {
            return Err("YAML nesting limit".into());
        }
        let (value, anchor) = match events.pop_front().ok_or("missing YAML node")? {
            Event::Scalar(text, style, anchor, tag) => {
                let value = if let Some(tag) = tag {
                    if tag.is_yaml_core_schema() && tag.suffix == "js" && expressions {
                        serde_json::json!({"__jsExpr":text})
                    } else if tag.is_yaml_core_schema() && tag.suffix == "str" {
                        Value::String(text.to_string())
                    } else {
                        return Err(format!("unsupported YAML tag {tag}"));
                    }
                } else if style == ScalarStyle::Plain {
                    scalar(&text)
                } else {
                    Value::String(text.to_string())
                };
                (value, anchor)
            }
            Event::Alias(id) => {
                return anchors
                    .get(&id)
                    .cloned()
                    .ok_or("unknown/cyclic YAML alias".into());
            }
            Event::SequenceStart(anchor, tag) => {
                if tag.is_some() {
                    return Err("tagged YAML sequence".into());
                }
                let mut items = Vec::new();
                while !matches!(events.front(), Some(Event::SequenceEnd)) {
                    items.push(node(events, anchors, depth + 1, expressions)?);
                }
                events.pop_front();
                (Value::Array(items), anchor)
            }
            Event::MappingStart(anchor, tag) => {
                if tag.is_some() {
                    return Err("tagged YAML mapping".into());
                }
                let mut map = Map::new();
                while !matches!(events.front(), Some(Event::MappingEnd)) {
                    let key = node(events, anchors, depth + 1, expressions)?;
                    let key = key.as_str().ok_or("YAML mapping key must be a string")?;
                    if map.contains_key(key) {
                        return Err(format!("duplicate YAML key {key}"));
                    }
                    map.insert(key.into(), node(events, anchors, depth + 1, expressions)?);
                }
                events.pop_front();
                (Value::Object(map), anchor)
            }
            _ => return Err("expected YAML node".into()),
        };
        if anchor != 0 {
            anchors.insert(anchor, value.clone());
        }
        Ok(value)
    }
    let value = node(&mut events, &mut HashMap::new(), 0, expressions)?;
    if !matches!(events.pop_front(), Some(Event::DocumentEnd))
        || !matches!(events.pop_front(), Some(Event::StreamEnd))
    {
        return Err("expected a single YAML document".into());
    }
    Ok(value)
}

fn scalar(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        "true" | "True" | "TRUE" => true.into(),
        "false" | "False" | "FALSE" => false.into(),
        _ => {
            // js-yaml JSON_SCHEMA uses the core int/float resolvers, without timestamps or merges.
            let unsigned = text.trim_start_matches(['+', '-']);
            let sign = if text.starts_with('-') { -1.0 } else { 1.0 };
            let radix = [("0x", 16), ("0o", 8), ("0b", 2)]
                .into_iter()
                .find_map(|(p, r)| unsigned.strip_prefix(p).map(|s| (s, r)));
            let number = if let Some((digits, r)) = radix {
                u64::from_str_radix(digits, r).ok().map(|v| sign * v as f64)
            } else if unsigned
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
            {
                text.parse::<f64>().ok()
            } else {
                None
            };
            number
                .filter(|n| n.is_finite())
                .and_then(|n| {
                    // js-yaml keeps `-0` as the number -0; its decimal rendering would lose the sign.
                    if n == 0.0 && n.is_sign_negative() {
                        return serde_json::Number::from_f64(n).map(Value::Number);
                    }
                    hoh_util_values::js_json_parse(&hoh_util_values::js_number_string(n)).ok()
                })
                .unwrap_or_else(|| Value::String(text.into()))
        }
    }
}

/// JSON is a YAML subset. Expressions retain their loader representation, without evaluation.
pub fn materialize(text: &str, source: &std::path::Path, refs: &std::path::Path) -> Result<String> {
    let mut value = parse(text, true)?;
    fn row(value: &mut Value, source: &std::path::Path, refs: &std::path::Path) -> Result<()> {
        if let Some(name) = value.get("name").and_then(Value::as_str).map(str::to_owned)
            && (name.starts_with("./") || name.starts_with("../"))
        {
            let base = source
                .parent()
                .ok_or("patch has no parent")?
                .to_string_lossy();
            let path = hoh_util_workspace_path::node_path::resolve(&base, &[&name]);
            let relative = std::path::Path::new(&path)
                .strip_prefix(refs)
                .map_err(|_| format!("plugin outside clone: {path}"))?;
            value["name"] = format!("local:{}", relative.display()).into();
        }
        if value["group"] == true
            && let Some(children) = value["config"].as_array_mut()
        {
            for child in children {
                row(child, source, refs)?;
            }
        }
        Ok(())
    }
    let patches = value.as_array_mut().ok_or_else(|| {
        format!(
            "snapshot profile patch must be a top-level array: {}",
            source.display()
        )
    })?;
    for patch in patches {
        if let Some(items) = patch["insert"].as_array_mut() {
            for item in items {
                row(item, source, refs)?;
            }
        }
    }
    Ok(hoh_util_values::js_json_stringify_pretty(&value, 2) + "\n")
}
