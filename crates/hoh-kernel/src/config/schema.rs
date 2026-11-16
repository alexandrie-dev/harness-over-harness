use super::{Issue, PathSeg, ValidationError};
use hoh_util_values::{JS_JSON_MAX_DEPTH, deep_equal_json, js_json_stringify, js_string};
use indexmap::IndexSet;
use serde_json::{Number, Value, json};
use std::cmp::Ordering;

// Default draft-2020-12 output of schemars 1.2.3, plus standard applicators used by extensions.
const WORDS: &[&str] = &[
    "$schema",
    "$id",
    "$ref",
    "$defs",
    "title",
    "description",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "format",
    "type",
    "enum",
    "const",
    "properties",
    "patternProperties",
    "propertyNames",
    "required",
    "additionalProperties",
    "unevaluatedProperties",
    "minProperties",
    "maxProperties",
    "items",
    "prefixItems",
    "uniqueItems",
    "minItems",
    "maxItems",
    "contains",
    "minContains",
    "maxContains",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "pattern",
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "dependentRequired",
    "dependentSchemas",
];
pub(super) fn target<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    reference.strip_prefix('#').and_then(|p| root.pointer(p))
}
pub(super) fn children(schema: &Value) -> Vec<&Value> {
    let mut out = vec![];
    for word in ["properties", "patternProperties", "dependentSchemas"] {
        if let Some(map) = schema[word].as_object() {
            out.extend(map.values());
        }
    }
    for word in [
        "items",
        "additionalProperties",
        "unevaluatedProperties",
        "propertyNames",
        "contains",
        "not",
        "if",
        "then",
        "else",
    ] {
        if let Some(child) = schema.get(word) {
            out.push(child);
        }
    }
    for word in ["prefixItems", "allOf", "anyOf", "oneOf"] {
        if let Some(array) = schema[word].as_array() {
            out.extend(array);
        }
    }
    out
}
pub(super) fn nullable_choice(schema: &Value) -> Option<&Value> {
    for key in ["anyOf", "oneOf"] {
        if let Some(a) = schema[key].as_array()
            && a.len() == 2
            && a.iter().any(|s| s["type"] == "null")
        {
            return a.iter().find(|s| s["type"] != "null");
        }
    }
    None
}
fn allows_null(schema: &Value) -> bool {
    schema["type"] == "null"
        || schema["type"]
            .as_array()
            .is_some_and(|a| a.iter().any(|t| t == "null"))
        || ["anyOf", "oneOf"].iter().any(|k| {
            schema[k]
                .as_array()
                .is_some_and(|a| a.iter().any(allows_null))
        })
}
pub(crate) fn normalized(schema: &Value, value: &Value) -> Value {
    if value.is_null()
        && !allows_null(schema)
        && (schema["type"] == "object" || schema["properties"].is_object())
    {
        json!({})
    } else {
        value.clone()
    }
}
fn add(out: &mut Vec<Issue>, path: &[PathSeg], message: impl Into<String>) {
    out.push(Issue {
        message: message.into(),
        path: path.to_vec(),
    });
}
fn pattern(source: &str) -> Result<regex::Regex, regex::Error> {
    let mut output = String::new();
    let mut chars = source.chars();
    let mut class = false;
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(c) = chars.next() {
                match c {
                    'd' => output.push_str("[0-9]"),
                    'D' => output.push_str("[^0-9]"),
                    'w' => output.push_str("[A-Za-z0-9_]"),
                    'W' => output.push_str("[^A-Za-z0-9_]"),
                    'b' if !class => output.push_str(r"(?-u:\b)"),
                    'B' if !class => output.push_str(r"(?-u:\B)"),
                    'b' => output.push_str(r"\x08"),
                    _ => {
                        output.push('\\');
                        output.push(c);
                    }
                }
            } else {
                output.push('\\');
            }
        } else {
            if c == '[' {
                class = true;
            }
            if c == ']' {
                class = false;
            }
            output.push(c);
        }
    }
    regex::Regex::new(&output)
}
fn inspect(
    root: &Value,
    s: &Value,
    path: &mut Vec<PathSeg>,
    restricted: bool,
    wrapped: bool,
    refs: &mut Vec<String>,
    out: &mut Vec<Issue>,
) {
    let Some(obj) = s.as_object() else { return };
    for key in obj.keys() {
        if !WORDS.contains(&key.as_str()) && !key.starts_with("x-") {
            add(out, path, format!("unsupported schema keyword: {key}"));
        }
    }
    if let Some(p) = s["pattern"].as_str()
        && let Err(e) = pattern(p)
    {
        add(out, path, format!("invalid schema pattern {p:?}: {e}"));
    }
    if let Some(props) = s["patternProperties"].as_object() {
        for p in props.keys() {
            if let Err(e) = pattern(p) {
                add(out, path, format!("invalid schema pattern {p:?}: {e}"));
            }
        }
    }
    let live = s["x-volatile"] == true;
    if live && (restricted || wrapped) || s.get("x-volatile-invalid").is_some() {
        add(
            out,
            path,
            "invalid volatile placement or already wrapped value",
        );
    }
    if let Some(r) = s["$ref"].as_str()
        && !refs.iter().any(|v| v == r)
    {
        if let Some(t) = target(root, r) {
            refs.push(r.into());
            inspect(root, t, path, restricted, wrapped || live, refs, out);
            refs.pop();
        } else {
            add(out, path, format!("unsupported schema reference: {r}"));
        }
    }
    for word in ["properties", "patternProperties", "dependentSchemas"] {
        if let Some(props) = s[word].as_object() {
            for (k, v) in props {
                path.push(PathSeg::Key(k.clone()));
                inspect(
                    root,
                    v,
                    path,
                    restricted || word != "properties",
                    wrapped || live,
                    refs,
                    out,
                );
                path.pop();
            }
        }
    }
    for word in [
        "items",
        "additionalProperties",
        "unevaluatedProperties",
        "propertyNames",
        "contains",
        "not",
        "if",
        "then",
        "else",
    ] {
        if let Some(v) = s.get(word) {
            inspect(root, v, path, true, wrapped || live, refs, out);
        }
    }
    for word in ["prefixItems", "anyOf", "oneOf", "allOf"] {
        if let Some(a) = s[word].as_array() {
            for v in a {
                inspect(
                    root,
                    v,
                    path,
                    if nullable_choice(s).is_some() {
                        restricted
                    } else {
                        true
                    },
                    wrapped || live,
                    refs,
                    out,
                );
            }
        }
    }
}
pub fn validate(schema: &Value, value: &Value) -> Result<(), ValidationError> {
    let mut issues = vec![];
    inspect(
        schema,
        schema,
        &mut vec![],
        false,
        false,
        &mut vec![],
        &mut issues,
    );
    // Definitions are declarations even when no instance reaches them.
    let mut pending = vec![schema];
    while let Some(s) = pending.pop() {
        if let Some(defs) = s["$defs"].as_object() {
            for d in defs.values() {
                inspect(
                    schema,
                    d,
                    &mut vec![],
                    false,
                    false,
                    &mut vec![],
                    &mut issues,
                );
                pending.push(d);
            }
        }
        pending.extend(children(s));
    }
    if !issues.is_empty() {
        return Err(ValidationError { issues });
    }
    let value = normalized(schema, value);
    eval(
        schema,
        schema,
        &value,
        &mut vec![],
        &mut vec![],
        &mut issues,
    );
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ValidationError { issues })
    }
}
fn description(s: &Value) -> String {
    if let Some(v) = s.get("const") {
        return js_json_stringify(v);
    }
    if let Some(a) = s["enum"].as_array() {
        return a
            .iter()
            .map(js_json_stringify)
            .collect::<Vec<_>>()
            .join(" | ");
    }
    for k in ["anyOf", "oneOf"] {
        if let Some(a) = s[k].as_array() {
            return a.iter().map(description).collect::<Vec<_>>().join(" | ");
        }
    }
    if let Some(a) = s["type"].as_array() {
        return a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" | ");
    }
    s["type"].as_str().unwrap_or("unknown").into()
}
fn integer(n: &Number) -> Option<i128> {
    n.as_i64()
        .map(i128::from)
        .or_else(|| n.as_u64().map(i128::from))
        .or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && f.abs() <= 9007199254740992.0)
                .map(|f| f as i128)
        })
}
fn limits(format: &str) -> Option<(i128, i128)> {
    match format {
        "int8" => Some((i8::MIN.into(), i8::MAX.into())),
        "int16" => Some((i16::MIN.into(), i16::MAX.into())),
        "int32" => Some((i32::MIN.into(), i32::MAX.into())),
        "int64" | "int" => Some((i64::MIN.into(), i64::MAX.into())),
        "uint8" => Some((0, u8::MAX.into())),
        "uint16" => Some((0, u16::MAX.into())),
        "uint32" => Some((0, u32::MAX.into())),
        "uint64" | "uint" => Some((0, u64::MAX.into())),
        _ => None,
    }
}
fn numeric_cmp(a: &Number, b: &Number) -> Ordering {
    match (integer(a), integer(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .unwrap_or(Ordering::Equal),
    }
}
fn matches_type(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "number" => v.is_number(),
        "integer" => v.as_number().and_then(integer).is_some(),
        _ => false,
    }
}
fn branch(
    root: &Value,
    s: &Value,
    v: &Value,
    path: &mut Vec<PathSeg>,
    seen: &mut Vec<*const Value>,
) -> (IndexSet<String>, Vec<Issue>) {
    let mut out = vec![];
    let keys = eval(root, s, v, path, seen, &mut out);
    (keys, out)
}
fn eval(
    root: &Value,
    s: &Value,
    v: &Value,
    path: &mut Vec<PathSeg>,
    seen: &mut Vec<*const Value>,
    out: &mut Vec<Issue>,
) -> IndexSet<String> {
    let mut evaluated = IndexSet::new();
    if path.len() >= JS_JSON_MAX_DEPTH && (v.is_object() || v.is_array()) {
        add(out, path, "config nesting limit exceeded");
        return evaluated;
    }
    let pointer = std::ptr::from_ref(s);
    if seen.contains(&pointer) {
        return evaluated;
    }
    seen.push(pointer);
    eval_inner(root, s, v, path, seen, out, &mut evaluated);
    seen.pop();
    evaluated
}
fn eval_inner(
    root: &Value,
    s: &Value,
    v: &Value,
    path: &mut Vec<PathSeg>,
    seen: &mut Vec<*const Value>,
    out: &mut Vec<Issue>,
    evaluated: &mut IndexSet<String>,
) {
    if s == &Value::Bool(false) {
        add(
            out,
            path,
            format!("expected nullable but got {}", js_string(v)),
        );
        return;
    }
    if !s.is_object() {
        return;
    }
    if let Some(t) = s.get("type") {
        let valid = match t {
            Value::String(t) => matches_type(t, v),
            Value::Array(a) => a
                .iter()
                .filter_map(Value::as_str)
                .any(|t| matches_type(t, v)),
            _ => false,
        };
        if !valid {
            add(
                out,
                path,
                format!("expected {} but got {}", description(s), js_string(v)),
            );
            return;
        }
    }
    if s["type"] == "integer"
        && let (Some(n), Some((min, max))) = (
            v.as_number().and_then(integer),
            s["format"].as_str().and_then(limits),
        )
    {
        for (bad, op, limit) in [(n < min, ">=", min), (n > max, "<=", max)] {
            if bad {
                add(
                    out,
                    path,
                    format!(
                        "expected number {op} {} but got {}",
                        hoh_util_values::js_number_string(limit as f64),
                        js_string(v)
                    ),
                );
            }
        }
    }
    if let Some(r) = s["$ref"].as_str()
        && let Some(t) = target(root, r)
    {
        evaluated.extend(eval(root, t, v, path, seen, out));
    }
    for word in ["enum", "const"] {
        if let Some(expected) = s.get(word) {
            let valid = if word == "enum" {
                expected
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| deep_equal_json(x, v)))
            } else {
                deep_equal_json(expected, v)
            };
            if !valid {
                add(
                    out,
                    path,
                    format!(
                        "expected {} but got {}",
                        description(s),
                        js_json_stringify(v)
                    ),
                );
            }
        }
    }
    for word in ["allOf", "anyOf", "oneOf"] {
        if let Some(choices) = s[word].as_array() {
            let results = choices
                .iter()
                .map(|c| branch(root, c, v, path, seen))
                .collect::<Vec<_>>();
            let valid = results.iter().filter(|(_, e)| e.is_empty()).count();
            if word == "allOf" {
                for (keys, errors) in results {
                    evaluated.extend(keys);
                    out.extend(errors);
                }
            } else if valid == 0 || (word == "oneOf" && valid != 1) {
                add(
                    out,
                    path,
                    format!(
                        "expected {} but got {}",
                        description(s),
                        js_json_stringify(v)
                    ),
                );
            } else {
                for (keys, errors) in results {
                    if errors.is_empty() {
                        evaluated.extend(keys);
                    }
                }
            }
        }
    }
    if let Some(n) = s.get("not")
        && branch(root, n, v, path, seen).1.is_empty()
    {
        add(
            out,
            path,
            format!(
                "expected value outside {} but got {}",
                description(n),
                js_json_stringify(v)
            ),
        );
    }
    if let Some(condition) = s.get("if") {
        let keyword = if branch(root, condition, v, path, seen).1.is_empty() {
            "then"
        } else {
            "else"
        };
        if let Some(next) = s.get(keyword) {
            evaluated.extend(eval(root, next, v, path, seen, out));
        }
    }
    if let Some(map) = v.as_object() {
        if let Some(req) = s["required"].as_array() {
            for key in req.iter().filter_map(Value::as_str) {
                if !map.contains_key(key) {
                    path.push(PathSeg::Key(key.into()));
                    add(out, path, "missing required value");
                    path.pop();
                }
            }
        }
        for (key, value) in map {
            path.push(PathSeg::Key(key.clone()));
            let mut matched = false;
            if let Some(child) = s["properties"].get(key) {
                matched = true;
                evaluated.insert(key.clone());
                eval(root, child, value, path, &mut vec![], out);
            }
            if let Some(patterns) = s["patternProperties"].as_object() {
                for (p, child) in patterns {
                    if pattern(p).is_ok_and(|r| r.is_match(key)) {
                        matched = true;
                        evaluated.insert(key.clone());
                        eval(root, child, value, path, &mut vec![], out);
                    }
                }
            }
            if !matched && let Some(child) = s.get("additionalProperties") {
                evaluated.insert(key.clone());
                eval(root, child, value, path, &mut vec![], out);
            }
            if let Some(child) = s.get("propertyNames") {
                eval(
                    root,
                    child,
                    &Value::String(key.clone()),
                    path,
                    &mut vec![],
                    out,
                );
            }
            path.pop();
        }
        if let Some(deps) = s["dependentRequired"].as_object() {
            for (key, list) in deps {
                if map.contains_key(key)
                    && let Some(list) = list.as_array()
                {
                    for name in list.iter().filter_map(Value::as_str) {
                        if !map.contains_key(name) {
                            path.push(PathSeg::Key(name.into()));
                            add(out, path, "missing required value");
                            path.pop();
                        }
                    }
                }
            }
        }
        if let Some(deps) = s["dependentSchemas"].as_object() {
            for (key, child) in deps {
                if map.contains_key(key) {
                    evaluated.extend(eval(root, child, v, path, seen, out));
                }
            }
        }
        if let Some(child) = s.get("unevaluatedProperties") {
            for (key, value) in map {
                if !evaluated.contains(key) {
                    path.push(PathSeg::Key(key.clone()));
                    eval(root, child, value, path, &mut vec![], out);
                    path.pop();
                    evaluated.insert(key.clone());
                }
            }
        }
        lengths(s, map.len(), "Properties", "object size", path, out);
    }
    if let Some(array) = v.as_array() {
        for (i, value) in array.iter().enumerate() {
            if let Some(child) = s["prefixItems"].get(i).or_else(|| s.get("items")) {
                path.push(PathSeg::Index(i));
                eval(root, child, value, path, &mut vec![], out);
                path.pop();
            }
        }
        lengths(s, array.len(), "Items", "array length", path, out);
        if s["uniqueItems"] == true {
            for (i, value) in array.iter().enumerate() {
                if array[..i]
                    .iter()
                    .any(|previous| deep_equal_json(previous, value))
                {
                    path.push(PathSeg::Index(i));
                    add(out, path, "expected unique array item");
                    path.pop();
                }
            }
        }
        if let Some(child) = s.get("contains") {
            let count = array
                .iter()
                .filter(|value| branch(root, child, value, path, &mut vec![]).1.is_empty())
                .count();
            let min = s["minContains"].as_u64().unwrap_or(1);
            let max = s["maxContains"].as_u64().unwrap_or(u64::MAX);
            if (count as u64) < min || (count as u64) > max {
                add(out, path, "expected array containing matching items");
            }
        }
    }
    if let Some(n) = v.as_number() {
        for (key, op) in [
            ("minimum", ">="),
            ("maximum", "<="),
            ("exclusiveMinimum", ">"),
            ("exclusiveMaximum", "<"),
            ("multipleOf", "multiple of"),
        ] {
            if let Some(bound) = s[key].as_number() {
                let cmp = numeric_cmp(n, bound);
                let valid = match key {
                    "minimum" => cmp != Ordering::Less,
                    "maximum" => cmp != Ordering::Greater,
                    "exclusiveMinimum" => cmp == Ordering::Greater,
                    "exclusiveMaximum" => cmp == Ordering::Less,
                    _ => match (integer(n), integer(bound)) {
                        (Some(a), Some(b)) => b != 0 && a % b == 0,
                        _ => {
                            let a = n.as_f64().unwrap_or(0.0);
                            let b = bound.as_f64().unwrap_or(0.0);
                            b != 0.0 && ((a / b).round() - a / b).abs() < 1e-10
                        }
                    },
                };
                if !valid {
                    add(
                        out,
                        path,
                        format!(
                            "expected number {op} {} but got {}",
                            js_string(&s[key]),
                            js_string(v)
                        ),
                    );
                }
            }
        }
    }
    if let Some(text) = v.as_str() {
        lengths(
            s,
            text.encode_utf16().count(),
            "Length",
            "string length",
            path,
            out,
        );
        if let Some(p) = s["pattern"].as_str()
            && pattern(p).is_ok_and(|r| !r.is_match(text))
        {
            add(out, path, format!("expect string to match regexp /{p}/"));
        }
    }
}
fn lengths(
    s: &Value,
    len: usize,
    suffix: &str,
    label: &str,
    path: &[PathSeg],
    out: &mut Vec<Issue>,
) {
    for (prefix, min, op) in [("min", true, ">="), ("max", false, "<=")] {
        if let Some(n) = s[format!("{prefix}{suffix}")].as_u64()
            && ((min && (len as u64) < n) || (!min && (len as u64) > n))
        {
            add(
                out,
                path,
                format!("expected {label} {op} {n} but got {len}"),
            );
        }
    }
}
pub(crate) fn coerce_integers(root: &Value, s: &Value, value: &mut Value) {
    fn visit(root: &Value, s: &Value, v: &mut Value, seen: &mut Vec<*const Value>) {
        let p = std::ptr::from_ref(s);
        if seen.contains(&p) {
            return;
        }
        seen.push(p);
        if s["type"] == "integer"
            && let Some(n) = v.as_number().and_then(integer)
        {
            if let Ok(n) = u64::try_from(n) {
                *v = Value::Number(n.into());
            } else if let Ok(n) = i64::try_from(n) {
                *v = Value::Number(n.into());
            }
        }
        if let Some(r) = s["$ref"].as_str()
            && let Some(t) = target(root, r)
        {
            visit(root, t, v, seen);
        }
        for key in ["allOf", "anyOf", "oneOf"] {
            if let Some(choices) = s[key].as_array() {
                for child in choices {
                    if key == "allOf"
                        || branch(root, child, v, &mut vec![], &mut vec![])
                            .1
                            .is_empty()
                    {
                        visit(root, child, v, seen);
                    }
                }
            }
        }
        if let Some(map) = v.as_object_mut() {
            for (key, value) in map {
                if let Some(child) = s["properties"].get(key) {
                    visit(root, child, value, &mut vec![]);
                } else if let Some(child) = s.get("additionalProperties") {
                    visit(root, child, value, &mut vec![]);
                }
                if let Some(patterns) = s["patternProperties"].as_object() {
                    for (p, child) in patterns {
                        if pattern(p).is_ok_and(|r| r.is_match(key)) {
                            visit(root, child, value, &mut vec![]);
                        }
                    }
                }
            }
        }
        if let Some(a) = v.as_array_mut() {
            for (i, v) in a.iter_mut().enumerate() {
                if let Some(child) = s["prefixItems"].get(i).or_else(|| s.get("items")) {
                    visit(root, child, v, &mut vec![]);
                }
            }
        }
        seen.pop();
    }
    visit(root, s, value, &mut vec![]);
}
pub(crate) fn merge(mut raw: Value, typed: Value) -> Value {
    match (&mut raw, typed) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                let slot = a.entry(k).or_insert(Value::Null);
                let old = std::mem::take(slot);
                *slot = merge(old, v);
            }
            raw
        }
        (Value::Array(a), Value::Array(b)) => Value::Array(
            b.into_iter()
                .enumerate()
                .map(|(i, v)| merge(a.get(i).cloned().unwrap_or(Value::Null), v))
                .collect(),
        ),
        (_, typed) => typed,
    }
}
