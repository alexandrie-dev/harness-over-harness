use crate::{Value as Erased, read};
use serde_json::{Value, json};
use std::{
    fmt,
    sync::{Arc, RwLock},
};

pub trait Config:
    serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema + Send + Sync + 'static
{
}
impl<T> Config for T where
    T: serde::de::DeserializeOwned
        + serde::Serialize
        + schemars::JsonSchema
        + Send
        + Sync
        + 'static
{
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct NoConfig {}
pub struct Cfg<C: Config> {
    pub(crate) value: Arc<RwLock<Erased>>,
    pub(crate) marker: std::marker::PhantomData<C>,
}
impl<C: Config> Clone for Cfg<C> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            marker: std::marker::PhantomData,
        }
    }
}
impl<C: Config> Cfg<C> {
    pub fn get(&self) -> Arc<C> {
        // PluginDef creates and updates this cell only with its associated Config type.
        read(&self.value)
            .clone()
            .downcast::<C>()
            .expect("typed configuration cell")
    }
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Volatile<T>(pub T);
impl<T> std::ops::Deref for Volatile<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: schemars::JsonSchema> schemars::JsonSchema for Volatile<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("Volatile_{}", T::schema_name()).into()
    }
    fn inline_schema() -> bool {
        true
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = T::json_schema(generator);
        if schema.get("x-volatile") == Some(&Value::Bool(true)) {
            schema.insert(
                "x-volatile-invalid".into(),
                Value::String("already wrapped".into()),
            );
        }
        schema.insert("x-volatile".into(), Value::Bool(true));
        schema
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum PathSeg {
    Key(String),
    Index(usize),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub message: String,
    pub path: Vec<PathSeg>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ValidationError {
    pub issues: Vec<Issue>,
}
impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid config:")?;
        for issue in &self.issues {
            write!(f, "\n  - {}", issue.message)?;
            if !issue.path.is_empty() {
                let path = issue
                    .path
                    .iter()
                    .map(|s| match s {
                        PathSeg::Key(k) => k.clone(),
                        PathSeg::Index(i) => i.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(".");
                write!(f, " (at {path})")?;
            }
        }
        Ok(())
    }
}
impl std::error::Error for ValidationError {}
pub(crate) fn issue(message: impl Into<String>) -> ValidationError {
    ValidationError {
        issues: vec![Issue {
            message: message.into(),
            path: vec![],
        }],
    }
}
fn js(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Object(_) => "[object Object]".into(),
        Value::Array(a) => a
            .iter()
            .map(|v| if v.is_null() { String::new() } else { js(v) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Number(number) => js_number(number),
        _ => value.to_string(),
    }
}
fn js_number(number: &serde_json::Number) -> String {
    if let Some(n) = number.as_i64() {
        return n.to_string();
    }
    if let Some(n) = number.as_u64() {
        return n.to_string();
    }
    let n = number.as_f64().unwrap_or(0.0);
    if n == 0.0 {
        return "0".into();
    }
    let text = n.abs().to_string();
    let sign = if n < 0.0 { "-" } else { "" };
    if n.abs() >= 1e-6 && n.abs() < 1e21 {
        return format!("{sign}{text}");
    }
    let (integer, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let exponent = if integer != "0" {
        integer.len() as i32 - 1
    } else {
        -(fraction.chars().take_while(|c| *c == '0').count() as i32) - 1
    };
    let digits = format!("{integer}{fraction}");
    let digits = digits.trim_start_matches('0').trim_end_matches('0');
    let mantissa = if digits.len() > 1 {
        format!("{}.{}", &digits[..1], &digits[1..])
    } else {
        digits.to_owned()
    };
    format!(
        "{sign}{mantissa}e{}{exponent}",
        if exponent >= 0 { "+" } else { "" }
    )
}
fn json_text(value: &Value) -> String {
    match value {
        Value::Number(number) => js_number(number),
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(json_text).collect::<Vec<_>>().join(",")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}:{}", Value::String(key.clone()), json_text(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => value.to_string(),
    }
}
fn description(schema: &Value) -> String {
    if let Some(v) = schema.get("const") {
        return json_text(v);
    }
    if let Some(values) = schema["enum"].as_array() {
        return values.iter().map(json_text).collect::<Vec<_>>().join(" | ");
    }
    for word in ["anyOf", "oneOf"] {
        if let Some(values) = schema[word].as_array() {
            return values
                .iter()
                .map(description)
                .collect::<Vec<_>>()
                .join(" | ");
        }
    }
    schema["type"].as_str().unwrap_or("unknown").into()
}
fn nullable_choice(schema: &Value) -> Option<&Value> {
    for key in ["anyOf", "oneOf"] {
        if let Some(choices) = schema[key].as_array()
            && choices.len() == 2
            && choices.iter().any(|c| c["type"] == "null")
        {
            return choices.iter().find(|c| c["type"] != "null");
        }
    }
    None
}
fn ascii_pattern(pattern: &str) -> String {
    let mut output = String::new();
    let mut chars = pattern.chars();
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
    output
}
pub(crate) fn normalized(schema: &Value, value: &Value) -> Value {
    if value.is_null() && (schema["type"] == "object" || schema["properties"].is_object()) {
        json!({})
    } else {
        value.clone()
    }
}
pub fn validate(schema: &Value, value: &Value) -> Result<(), ValidationError> {
    let value = normalized(schema, value);
    let mut issues = Vec::new();
    inspect_schema(
        schema,
        schema,
        &mut vec![],
        false,
        false,
        &mut vec![],
        &mut issues,
    );
    if !issues.is_empty() {
        return Err(ValidationError { issues });
    }
    walk(schema, schema, &value, &mut vec![], &mut issues, 0);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ValidationError { issues })
    }
}
const WORDS: &[&str] = &[
    "type",
    "enum",
    "const",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "prefixItems",
    "minItems",
    "maxItems",
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
    "$ref",
    "title",
    "description",
    "default",
    "format",
    "examples",
    "deprecated",
    "$schema",
    "$defs",
];
fn inspect_schema(
    root: &Value,
    schema: &Value,
    path: &mut Vec<PathSeg>,
    forbidden: bool,
    wrapped: bool,
    refs: &mut Vec<String>,
    issues: &mut Vec<Issue>,
) {
    let Some(object) = schema.as_object() else {
        return;
    };
    for key in object.keys() {
        if !WORDS.contains(&key.as_str()) && !key.starts_with("x-") {
            issues.push(Issue {
                message: format!("unsupported schema keyword: {key}"),
                path: path.clone(),
            });
        }
    }
    let volatile = schema["x-volatile"] == true;
    if volatile && (forbidden || wrapped) || schema.get("x-volatile-invalid").is_some() {
        issues.push(Issue {
            message: "invalid volatile placement or already wrapped value".into(),
            path: path.clone(),
        });
    }
    if let Some(reference) = schema["$ref"].as_str()
        && !refs.iter().any(|r| r == reference)
    {
        if let Some(target) = reference
            .strip_prefix("#/$defs/")
            .and_then(|_| root.pointer(&reference[1..]))
        {
            refs.push(reference.into());
            inspect_schema(
                root,
                target,
                path,
                forbidden,
                wrapped || volatile,
                refs,
                issues,
            );
            refs.pop();
        } else {
            issues.push(Issue {
                message: format!("unsupported schema reference: {reference}"),
                path: path.clone(),
            });
        }
    }
    if let Some(properties) = schema["properties"].as_object() {
        for (key, child) in properties {
            path.push(PathSeg::Key(key.clone()));
            inspect_schema(
                root,
                child,
                path,
                forbidden,
                wrapped || volatile,
                refs,
                issues,
            );
            path.pop();
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(child) = schema.get(key) {
            inspect_schema(root, child, path, true, wrapped || volatile, refs, issues);
        }
    }
    for key in ["prefixItems", "anyOf", "oneOf", "allOf"] {
        if let Some(children) = schema[key].as_array() {
            for child in children {
                inspect_schema(
                    root,
                    child,
                    path,
                    if nullable_choice(schema).is_some() {
                        forbidden
                    } else {
                        true
                    },
                    wrapped || volatile,
                    refs,
                    issues,
                );
            }
        }
    }
}
fn walk(
    root: &Value,
    schema: &Value,
    value: &Value,
    path: &mut Vec<PathSeg>,
    out: &mut Vec<Issue>,
    depth: usize,
) {
    if depth > 128 {
        out.push(Issue {
            message: "schema recursion limit exceeded".into(),
            path: path.clone(),
        });
        return;
    }
    let mut add = |message: String| {
        out.push(Issue {
            message,
            path: path.clone(),
        })
    };
    if schema == &Value::Bool(false) {
        add(format!("expected nullable but got {}", js(value)));
        return;
    }
    let Some(object) = schema.as_object() else {
        return;
    };
    const WORDS: &[&str] = &[
        "type",
        "enum",
        "const",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "prefixItems",
        "minItems",
        "maxItems",
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
        "$ref",
        "title",
        "description",
        "default",
        "format",
        "examples",
        "deprecated",
        "$schema",
        "$defs",
    ];
    for key in object.keys() {
        if !WORDS.contains(&key.as_str()) && !key.starts_with("x-") {
            add(format!("unsupported schema keyword: {key}"));
        }
    }
    if let Some(reference) = schema["$ref"].as_str() {
        if let Some(target) = reference.strip_prefix('#').and_then(|p| root.pointer(p)) {
            walk(root, target, value, path, out, depth + 1);
        } else {
            out.push(Issue {
                message: format!("unsupported schema reference: {reference}"),
                path: path.clone(),
            });
        }
    }
    if let Some(t) = schema.get("type") {
        let matches = |ty: &str| match ty {
            "null" => value.is_null(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "number" => value.is_number(),
            "integer" => value.as_f64().is_some_and(|v| v.fract() == 0.0),
            _ => false,
        };
        let valid = match t {
            Value::String(t) => matches(t),
            Value::Array(a) => a.iter().any(|t| t.as_str().is_some_and(matches)),
            _ => false,
        };
        if !valid {
            out.push(Issue {
                message: format!(
                    "expected {} but got {}",
                    t.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| t.to_string()),
                    js(value)
                ),
                path: path.clone(),
            });
            return;
        }
    }
    for word in ["enum", "const"] {
        if let Some(expected) = schema.get(word) {
            let valid = if word == "enum" {
                expected.as_array().is_some_and(|a| a.contains(value))
            } else {
                expected == value
            };
            if !valid {
                out.push(Issue {
                    message: format!(
                        "expected {} but got {}",
                        description(schema),
                        json_text(value)
                    ),
                    path: path.clone(),
                });
            }
        }
    }
    for word in ["anyOf", "oneOf", "allOf"] {
        if let Some(choices) = schema[word].as_array() {
            let mut results = Vec::new();
            for choice in choices {
                let mut errors = vec![];
                walk(root, choice, value, path, &mut errors, depth + 1);
                results.push(errors);
            }
            let count = results.iter().filter(|e| e.is_empty()).count();
            if word == "allOf" {
                for errors in results {
                    out.extend(errors);
                }
            } else if count == 0 || (word == "oneOf" && count != 1) {
                out.push(Issue {
                    message: format!(
                        "expected {} but got {}",
                        description(schema),
                        json_text(value)
                    ),
                    path: path.clone(),
                });
            }
        }
    }
    if let Some(map) = value.as_object() {
        if let Some(required) = schema["required"].as_array() {
            for key in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(key) {
                    let mut p = path.clone();
                    p.push(PathSeg::Key(key.into()));
                    out.push(Issue {
                        message: "missing required value".into(),
                        path: p,
                    });
                }
            }
        }
        for (key, value) in map {
            path.push(PathSeg::Key(key.clone()));
            if let Some(child) = schema["properties"].get(key) {
                walk(root, child, value, path, out, depth + 1);
            } else if let Some(child) = schema.get("additionalProperties") {
                walk(root, child, value, path, out, depth + 1);
            }
            path.pop();
        }
    }
    if let Some(array) = value.as_array() {
        for (i, value) in array.iter().enumerate() {
            let prefix = schema["prefixItems"].as_array().and_then(|a| a.get(i));
            if let Some(child) = prefix.or_else(|| schema.get("items")) {
                path.push(PathSeg::Index(i));
                walk(root, child, value, path, out, depth + 1);
                path.pop();
            }
        }
        for (key, valid, op) in [("minItems", true, ">="), ("maxItems", false, "<=")] {
            if let Some(limit) = schema[key].as_u64()
                && ((valid && array.len() < limit as usize)
                    || (!valid && array.len() > limit as usize))
            {
                out.push(Issue {
                    message: format!("expected array length {op} {limit} but got {}", array.len()),
                    path: path.clone(),
                });
            }
        }
    }
    if let Some(number) = value.as_f64() {
        for (key, op) in [
            ("minimum", ">="),
            ("maximum", "<="),
            ("exclusiveMinimum", ">"),
            ("exclusiveMaximum", "<"),
            ("multipleOf", "multiple of"),
        ] {
            if let Some(limit) = schema[key].as_f64() {
                let valid = match key {
                    "minimum" => number >= limit,
                    "maximum" => number <= limit,
                    "exclusiveMinimum" => number > limit,
                    "exclusiveMaximum" => number < limit,
                    _ => limit != 0.0 && ((number / limit).round() - number / limit).abs() < 1e-10,
                };
                if !valid {
                    out.push(Issue {
                        message: format!(
                            "expected number {op} {} but got {}",
                            js(&schema[key]),
                            js(value)
                        ),
                        path: path.clone(),
                    });
                }
            }
        }
    }
    if let Some(text) = value.as_str() {
        for (key, min, op) in [("minLength", true, ">="), ("maxLength", false, "<=")] {
            if let Some(limit) = schema[key].as_u64() {
                let length = text.encode_utf16().count();
                if (min && length < limit as usize) || (!min && length > limit as usize) {
                    out.push(Issue {
                        message: format!("expected string length {op} {limit} but got {length}"),
                        path: path.clone(),
                    });
                }
            }
        }
        if let Some(pattern) = schema["pattern"].as_str() {
            let pattern_ascii = ascii_pattern(pattern);
            match regex::Regex::new(&pattern_ascii) {
                Ok(re) if re.is_match(text) => {}
                _ => out.push(Issue {
                    message: format!("expect string to match regexp /{pattern}/"),
                    path: path.clone(),
                }),
            }
        }
    }
}

pub fn equal_except_volatile(schema: &Value, a: &Value, b: &Value) -> bool {
    compare(schema, Some(a), Some(b))
}
fn defaulted<'a>(schema: &'a Value, value: Option<&'a Value>) -> Option<&'a Value> {
    value
        .filter(|v| {
            !(v.is_null() && (schema["properties"].is_object() || schema["type"] == "object"))
        })
        .or_else(|| schema.get("default"))
}
fn compare(schema: &Value, a: Option<&Value>, b: Option<&Value>) -> bool {
    if schema["x-volatile"] == true {
        return true;
    }
    if let Some(child) = nullable_choice(schema) {
        return compare(child, a, b);
    }
    let a = defaulted(schema, a);
    let b = defaulted(schema, b);
    if a == b {
        return true;
    }
    if [a, b].into_iter().flatten().any(|v| {
        v.as_object()
            .is_some_and(|o| o.len() == 1 && o.contains_key("__jsExpr"))
    }) {
        return false;
    }
    if schema.get("$ref").is_some() {
        return false;
    }
    if let Some(properties) = schema["properties"].as_object() {
        let object = |v: Option<&Value>| v.is_none_or(|v| v.is_object() || v.is_null());
        if object(a) && object(b) {
            let mut keys: indexmap::IndexSet<&str> =
                properties.keys().map(String::as_str).collect();
            for v in [a, b].into_iter().flatten().filter_map(Value::as_object) {
                keys.extend(v.keys().map(String::as_str));
            }
            return keys.into_iter().all(|k| {
                compare(
                    properties.get(k).unwrap_or(&Value::Null),
                    a.and_then(|v| v.get(k)),
                    b.and_then(|v| v.get(k)),
                )
            });
        }
    }
    if let (Some(Value::Array(a)), Some(Value::Array(b))) = (a, b) {
        return a.len() == b.len()
            && a.iter().zip(b).enumerate().all(|(i, (a, b))| {
                compare(
                    schema["prefixItems"]
                        .get(i)
                        .or_else(|| schema.get("items"))
                        .unwrap_or(&Value::Null),
                    Some(a),
                    Some(b),
                )
            });
    }
    for word in ["allOf", "anyOf", "oneOf"] {
        if let Some(choices) = schema[word].as_array() {
            return choices.iter().all(|s| compare(s, a, b));
        }
    }
    false
}
pub fn volatile_paths(schema: &Value, a: &Value, b: &Value) -> Vec<String> {
    fn visit(
        schema: &Value,
        a: Option<&Value>,
        b: Option<&Value>,
        path: String,
        out: &mut Vec<String>,
    ) {
        if a == b {
            return;
        }
        if schema["x-volatile"] == true {
            out.push(path);
            return;
        }
        if let Some(child) = nullable_choice(schema) {
            visit(child, a, b, path, out);
            return;
        }
        if let Some(props) = schema["properties"].as_object() {
            for (key, s) in props {
                visit(
                    s,
                    a.and_then(|v| v.get(key)),
                    b.and_then(|v| v.get(key)),
                    if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    },
                    out,
                );
            }
        }
        let length = [a, b]
            .into_iter()
            .flatten()
            .filter_map(Value::as_array)
            .map(Vec::len)
            .max()
            .unwrap_or(0);
        for i in 0..length {
            if let Some(s) = schema["prefixItems"].get(i).or_else(|| schema.get("items")) {
                visit(
                    s,
                    a.and_then(|v| v.get(i)),
                    b.and_then(|v| v.get(i)),
                    if path.is_empty() {
                        i.to_string()
                    } else {
                        format!("{path}.{i}")
                    },
                    out,
                );
            }
        }
    }
    let mut paths = vec![];
    visit(schema, Some(a), Some(b), String::new(), &mut paths);
    paths
}
