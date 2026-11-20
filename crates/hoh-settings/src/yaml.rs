use saphyr_parser::{Event, Parser, ScalarStyle};
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};

pub(crate) fn parse(text: &str) -> Result<Value, String> {
    // SHIM(hoh-yaml): replace this local YAML 1.2-core materializer when that crate exists.
    let mut events = VecDeque::new();
    for event in Parser::new_from_str(text) {
        events.push_back(event.map_err(|error| error.to_string())?.0);
    }
    if !matches!(events.pop_front(), Some(Event::StreamStart)) {
        return Err("missing YAML stream".into());
    }
    if matches!(events.front(), Some(Event::StreamEnd)) {
        return Ok(Value::Null);
    }
    events.pop_front();
    let value = node(&mut events, &mut HashMap::new(), &mut 0, 0)?;
    if !matches!(events.pop_front(), Some(Event::DocumentEnd))
        || !matches!(events.pop_front(), Some(Event::StreamEnd))
    {
        return Err("expected a single YAML document".into());
    }
    Ok(value)
}

fn node<'a>(
    events: &mut VecDeque<Event<'a>>,
    anchors: &mut HashMap<usize, Value>,
    expanded: &mut usize,
    depth: usize,
) -> Result<Value, String> {
    if depth > 128 {
        return Err("YAML nesting limit exceeded".into());
    }
    *expanded = expanded.saturating_add(1);
    if *expanded > 100_000 {
        return Err("expanded YAML node limit exceeded (100000)".into());
    }
    let (value, anchor) = match events.pop_front().ok_or("missing YAML node")? {
        Event::Scalar(text, style, anchor, tag) => {
            let suffix = match tag.as_deref() {
                Some(tag) if tag.is_yaml_core_schema() => Some(tag.suffix.as_ref()),
                Some(_) => return Err("legacy settings contains an unsupported YAML tag".into()),
                None => None,
            };
            let value = scalar(&text, style, suffix)?;
            (value, anchor)
        }
        Event::Alias(id) => {
            let value = anchors
                .get(&id)
                .cloned()
                .ok_or_else(|| "unknown or cyclic YAML alias".to_string())?;
            *expanded = expanded.saturating_add(node_count(&value));
            if *expanded > 100_000 {
                return Err("expanded YAML node limit exceeded (100000)".into());
            }
            return Ok(value);
        }
        Event::SequenceStart(anchor, tag) => {
            if tag.is_some() {
                return Err("tagged YAML sequence is unsupported".into());
            }
            let mut items = vec![];
            while !matches!(events.front(), Some(Event::SequenceEnd)) {
                items.push(node(events, anchors, expanded, depth + 1)?);
            }
            events.pop_front();
            (Value::Array(items), anchor)
        }
        Event::MappingStart(anchor, tag) => {
            if tag.is_some() {
                return Err("tagged YAML mapping is unsupported".into());
            }
            let mut map = Map::new();
            while !matches!(events.front(), Some(Event::MappingEnd)) {
                let key = node(events, anchors, expanded, depth + 1)?;
                let key = key.as_str().ok_or("YAML mapping key must be a string")?;
                if map.contains_key(key) {
                    return Err(format!("duplicate YAML key {key:?}"));
                }
                map.insert(key.into(), node(events, anchors, expanded, depth + 1)?);
            }
            events.pop_front();
            (Value::Object(map), anchor)
        }
        _ => return Err("expected YAML value".into()),
    };
    if anchor != 0 {
        anchors.insert(anchor, value.clone());
    }
    Ok(value)
}

fn node_count(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1usize.saturating_add(items.iter().map(node_count).sum()),
        Value::Object(items) => 1usize.saturating_add(items.values().map(node_count).sum()),
        _ => 1,
    }
}

fn scalar(text: &str, style: ScalarStyle, tag: Option<&str>) -> Result<Value, String> {
    if tag == Some("str") || tag.is_none() && style != ScalarStyle::Plain {
        return Ok(Value::String(text.into()));
    }
    let automatic = tag.is_none();
    if tag == Some("null") || automatic && matches!(text, "" | "~" | "null" | "Null" | "NULL") {
        return Ok(Value::Null);
    }
    if tag == Some("bool")
        || automatic && matches!(text, "true" | "True" | "TRUE" | "false" | "False" | "FALSE")
    {
        return match text {
            "true" | "True" | "TRUE" => Ok(Value::Bool(true)),
            "false" | "False" | "FALSE" => Ok(Value::Bool(false)),
            _ => Err(format!("invalid explicit boolean {text:?}")),
        };
    }
    if tag == Some("int") || automatic {
        if let Some(integer) = integer(text) {
            return Ok(integer);
        }
        if tag == Some("int") {
            return Err(format!("invalid explicit integer {text:?}"));
        }
    }
    if tag == Some("float") || automatic {
        if matches!(
            text.trim_start_matches(['+', '-']),
            ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN"
        ) {
            return Err("non-finite YAML numbers are not valid settings JSON".into());
        }
        let normalized = if let Some(rest) = text.strip_prefix("+.") {
            format!("0.{rest}")
        } else if let Some(rest) = text.strip_prefix("-.") {
            format!("-0.{rest}")
        } else if let Some(rest) = text.strip_prefix('.') {
            format!("0.{rest}")
        } else {
            text.replace('_', "")
        };
        if let Ok(number) = normalized.parse::<f64>()
            && let Some(number) = serde_json::Number::from_f64(number)
        {
            return Ok(Value::Number(number));
        }
        if tag == Some("float") {
            return Err(format!("invalid explicit float {text:?}"));
        }
    }
    if let Some(tag) = tag {
        return Err(format!("unsupported YAML core tag !!{tag}"));
    }
    Ok(Value::String(text.into()))
}

fn integer(text: &str) -> Option<Value> {
    let cleaned = text.replace('_', "");
    let (negative, unsigned) = if let Some(value) = cleaned.strip_prefix('-') {
        (true, value)
    } else {
        (false, cleaned.strip_prefix('+').unwrap_or(&cleaned))
    };
    let (digits, radix) = if let Some(value) = unsigned.strip_prefix("0x") {
        (value, 16)
    } else if let Some(value) = unsigned.strip_prefix("0o") {
        (value, 8)
    } else if let Some(value) = unsigned.strip_prefix("0b") {
        (value, 2)
    } else {
        (unsigned, 10)
    };
    if digits.is_empty() {
        return None;
    }
    let magnitude = u64::from_str_radix(digits, radix).ok()?;
    if negative {
        let magnitude = i128::from(magnitude);
        i64::try_from(-magnitude).ok().map(Value::from)
    } else {
        Some(Value::from(magnitude))
    }
}
