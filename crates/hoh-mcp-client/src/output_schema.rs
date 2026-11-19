use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

const TYPES: [&str; 7] = [
    "object", "array", "string", "number", "integer", "boolean", "null",
];

#[derive(Clone)]
pub(crate) struct CompiledOutputSchema {
    schema: Value,
    patterns: HashMap<String, CompatiblePattern>,
}

#[derive(Clone)]
enum CompatiblePattern {
    Native(Regex),
    LeadingAssertions {
        assertions: Vec<(bool, Regex)>,
        tail: Regex,
    },
}

impl CompatiblePattern {
    fn compile(pattern: &str) -> Result<Self, regex::Error> {
        if let Ok(regex) = Regex::new(pattern) {
            return Ok(Self::Native(regex));
        }
        let Some(mut rest) = pattern.strip_prefix('^') else {
            return Regex::new(pattern).map(Self::Native);
        };
        let mut assertions = Vec::new();
        while rest.starts_with("(?=") || rest.starts_with("(?!") {
            let positive = rest.as_bytes()[2] == b'=';
            let end = assertion_end(rest).ok_or_else(|| Regex::new(pattern).unwrap_err())?;
            let body = &rest[3..end];
            assertions.push((positive, Regex::new(&format!("^(?:{body})"))?));
            rest = &rest[end + 1..];
        }
        if assertions.is_empty() {
            return Regex::new(pattern).map(Self::Native);
        }
        Ok(Self::LeadingAssertions {
            assertions,
            tail: Regex::new(&format!("^(?:{rest})"))?,
        })
    }

    fn is_match(&self, text: &str) -> bool {
        match self {
            Self::Native(regex) => regex.is_match(text),
            Self::LeadingAssertions { assertions, tail } => {
                assertions
                    .iter()
                    .all(|(positive, regex)| regex.is_match(text) == *positive)
                    && tail.is_match(text)
            }
        }
    }
}

fn assertion_end(pattern: &str) -> Option<usize> {
    let mut depth = 1_usize;
    let mut escaped = false;
    let mut in_class = false;
    for (index, character) in pattern.char_indices().skip(3) {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class => depth += 1,
            ')' if !in_class => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

impl CompiledOutputSchema {
    pub(crate) fn compile(schema: Value) -> Result<Self, String> {
        let mut patterns = HashMap::new();
        compile_node(
            &schema,
            "schema",
            &schema,
            &mut patterns,
            &mut HashSet::new(),
        )?;
        Ok(Self { schema, patterns })
    }

    pub(crate) fn validate(&self, value: &Value) -> Option<String> {
        validate_node(&self.schema, value, "data", &self.schema, &self.patterns, 0)
    }
}

fn compile_node(
    schema: &Value,
    path: &str,
    root: &Value,
    patterns: &mut HashMap<String, CompatiblePattern>,
    resolving_refs: &mut HashSet<String>,
) -> Result<(), String> {
    if schema.is_boolean() {
        return Ok(());
    }
    let node = schema
        .as_object()
        .ok_or_else(|| format!("{path} must be an object or boolean schema"))?;
    if let Some(kind) = node.get("type") {
        let valid = match kind {
            Value::String(kind) => TYPES.contains(&kind.as_str()),
            Value::Array(kinds) => {
                !kinds.is_empty()
                    && kinds
                        .iter()
                        .all(|kind| kind.as_str().is_some_and(|kind| TYPES.contains(&kind)))
            }
            _ => false,
        };
        if !valid {
            return Err(format!("{path}/type must name valid JSON Schema types"));
        }
    }
    if let Some(reference) = node.get("$ref") {
        let reference = reference
            .as_str()
            .ok_or_else(|| format!("{path}/$ref must be string"))?;
        let target = resolve_ref(root, reference)
            .ok_or_else(|| format!("{path}/$ref cannot resolve {reference}"))?;
        if resolving_refs.insert(reference.to_owned()) {
            compile_node(
                target,
                &format!("{path}/$ref({reference})"),
                root,
                patterns,
                resolving_refs,
            )?;
            resolving_refs.remove(reference);
        }
    }
    if let Some(pattern) = node.get("pattern") {
        let pattern = pattern
            .as_str()
            .ok_or_else(|| format!("{path}/pattern must be string"))?;
        let compiled = CompatiblePattern::compile(pattern)
            .map_err(|error| format!("{path}/pattern {error}"))?;
        patterns.insert(pattern.to_owned(), compiled);
    }
    for keyword in ["properties", "patternProperties", "$defs", "definitions"] {
        if let Some(children) = node.get(keyword) {
            let children = children
                .as_object()
                .ok_or_else(|| format!("{path}/{keyword} must be object"))?;
            for (name, child) in children {
                if keyword == "patternProperties" {
                    let compiled = CompatiblePattern::compile(name)
                        .map_err(|error| format!("{path}/{keyword}/{name} {error}"))?;
                    patterns.insert(name.clone(), compiled);
                }
                compile_node(
                    child,
                    &format!("{path}/{keyword}/{name}"),
                    root,
                    patterns,
                    resolving_refs,
                )?;
            }
        }
    }
    for keyword in [
        "additionalProperties",
        "additionalItems",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
    ] {
        if let Some(child) = node.get(keyword) {
            compile_node(
                child,
                &format!("{path}/{keyword}"),
                root,
                patterns,
                resolving_refs,
            )?;
        }
    }
    if let Some(items) = node.get("items") {
        if let Some(items) = items.as_array() {
            for (index, child) in items.iter().enumerate() {
                compile_node(
                    child,
                    &format!("{path}/items/{index}"),
                    root,
                    patterns,
                    resolving_refs,
                )?;
            }
        } else {
            compile_node(
                items,
                &format!("{path}/items"),
                root,
                patterns,
                resolving_refs,
            )?;
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(children) = node.get(keyword) {
            let children = children
                .as_array()
                .filter(|children| !children.is_empty())
                .ok_or_else(|| format!("{path}/{keyword} must be a non-empty array"))?;
            for (index, child) in children.iter().enumerate() {
                compile_node(
                    child,
                    &format!("{path}/{keyword}/{index}"),
                    root,
                    patterns,
                    resolving_refs,
                )?;
            }
        }
    }
    if let Some(required) = node.get("required") {
        string_array(required, &format!("{path}/required"))?;
    }
    if let Some(values) = node.get("enum")
        && values.as_array().is_none_or(Vec::is_empty)
    {
        return Err(format!("{path}/enum must be a non-empty array"));
    }
    for keyword in ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"] {
        if node.get(keyword).is_some_and(|value| !value.is_number()) {
            return Err(format!("{path}/{keyword} must be number"));
        }
    }
    if node
        .get("multipleOf")
        .is_some_and(|value| value.as_f64().is_none_or(|value| value <= 0.0))
    {
        return Err(format!("{path}/multipleOf must be a positive number"));
    }
    for keyword in [
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "minProperties",
        "maxProperties",
    ] {
        if node
            .get(keyword)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(format!("{path}/{keyword} must be a non-negative integer"));
        }
    }
    if node
        .get("uniqueItems")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(format!("{path}/uniqueItems must be boolean"));
    }
    if let Some(dependencies) = node.get("dependencies") {
        let dependencies = dependencies
            .as_object()
            .ok_or_else(|| format!("{path}/dependencies must be object"))?;
        for (name, dependency) in dependencies {
            if dependency.is_array() {
                string_array(dependency, &format!("{path}/dependencies/{name}"))?;
            } else {
                compile_node(
                    dependency,
                    &format!("{path}/dependencies/{name}"),
                    root,
                    patterns,
                    resolving_refs,
                )?;
            }
        }
    }
    Ok(())
}

fn string_array(value: &Value, path: &str) -> Result<(), String> {
    if !value
        .as_array()
        .is_some_and(|values| values.iter().all(Value::is_string))
    {
        return Err(format!("{path} must be an array of strings"));
    }
    Ok(())
}

fn validate_node(
    schema: &Value,
    value: &Value,
    path: &str,
    root: &Value,
    patterns: &HashMap<String, CompatiblePattern>,
    depth: usize,
) -> Option<String> {
    if depth > 128 {
        return Some(format!("{path} exceeded schema reference depth"));
    }
    if let Some(allowed) = schema.as_bool() {
        return (!allowed).then(|| format!("{path} boolean schema is false"));
    }
    let node = schema.as_object()?;
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        return validate_node(
            resolve_ref(root, reference).expect("reference compiled"),
            value,
            path,
            root,
            patterns,
            depth + 1,
        );
    }
    if node.get("const").is_some_and(|constant| constant != value) {
        return Some(format!("{path} must be equal to constant"));
    }
    if let Some(values) = node.get("enum").and_then(Value::as_array)
        && !values.contains(value)
    {
        return Some(format!("{path} must be equal to one of the allowed values"));
    }
    if let Some(branches) = node.get("allOf").and_then(Value::as_array) {
        for branch in branches {
            if let Some(error) = validate_node(branch, value, path, root, patterns, depth + 1) {
                return Some(error);
            }
        }
    }
    if let Some(branches) = node.get("anyOf").and_then(Value::as_array)
        && !branches
            .iter()
            .any(|branch| validate_node(branch, value, path, root, patterns, depth + 1).is_none())
    {
        return Some(format!("{path} must match a schema in anyOf"));
    }
    if let Some(branches) = node.get("oneOf").and_then(Value::as_array)
        && branches
            .iter()
            .filter(|branch| {
                validate_node(branch, value, path, root, patterns, depth + 1).is_none()
            })
            .count()
            != 1
    {
        return Some(format!("{path} must match exactly one schema in oneOf"));
    }
    if let Some(negated) = node.get("not")
        && validate_node(negated, value, path, root, patterns, depth + 1).is_none()
    {
        return Some(format!("{path} must NOT be valid"));
    }
    if let Some(condition) = node.get("if") {
        let branch = if validate_node(condition, value, path, root, patterns, depth + 1).is_none() {
            node.get("then")
        } else {
            node.get("else")
        };
        if let Some(branch) = branch
            && let Some(error) = validate_node(branch, value, path, root, patterns, depth + 1)
        {
            return Some(error);
        }
    }
    if let Some(types) = node.get("type") {
        let matches = match types {
            Value::String(kind) => matches_type(kind, value),
            Value::Array(kinds) => kinds
                .iter()
                .filter_map(Value::as_str)
                .any(|kind| matches_type(kind, value)),
            _ => false,
        };
        if !matches {
            let expected = types.as_str().unwrap_or("the advertised type");
            return Some(format!("{path} must be {expected}"));
        }
    }
    validate_number(node, value, path)
        .or_else(|| validate_string(node, value, path, patterns))
        .or_else(|| validate_array(node, value, path, root, patterns, depth))
        .or_else(|| validate_object(node, value, path, root, patterns, depth))
}

fn validate_number(node: &Map<String, Value>, value: &Value, path: &str) -> Option<String> {
    let number = value.as_f64()?;
    for (keyword, operator) in [
        ("minimum", ">="),
        ("maximum", "<="),
        ("exclusiveMinimum", ">"),
        ("exclusiveMaximum", "<"),
    ] {
        if let Some(limit) = node.get(keyword).and_then(Value::as_f64) {
            let valid = match keyword {
                "minimum" => number >= limit,
                "maximum" => number <= limit,
                "exclusiveMinimum" => number > limit,
                _ => number < limit,
            };
            if !valid {
                return Some(format!("{path} must be {operator} {limit}"));
            }
        }
    }
    if let Some(divisor) = node.get("multipleOf").and_then(Value::as_f64) {
        let quotient = number / divisor;
        if (quotient - quotient.round()).abs() > f64::EPSILON * quotient.abs().max(1.0) {
            return Some(format!("{path} must be multiple of {divisor}"));
        }
    }
    None
}

fn validate_string(
    node: &Map<String, Value>,
    value: &Value,
    path: &str,
    patterns: &HashMap<String, CompatiblePattern>,
) -> Option<String> {
    let text = value.as_str()?;
    if let Some(pattern) = node.get("pattern").and_then(Value::as_str) {
        let Some(compiled) = patterns.get(pattern) else {
            return Some(format!("{path} pattern \"{pattern}\" was not compiled"));
        };
        if !compiled.is_match(text) {
            return Some(format!("{path} must match pattern \"{pattern}\""));
        }
    }
    let length = text.chars().count() as u64;
    if let Some(minimum) = node.get("minLength").and_then(Value::as_u64)
        && length < minimum
    {
        return Some(format!(
            "{path} must NOT have fewer than {minimum} characters"
        ));
    }
    if let Some(maximum) = node.get("maxLength").and_then(Value::as_u64)
        && length > maximum
    {
        return Some(format!(
            "{path} must NOT have more than {maximum} characters"
        ));
    }
    None
}

fn validate_array(
    node: &Map<String, Value>,
    value: &Value,
    path: &str,
    root: &Value,
    patterns: &HashMap<String, CompatiblePattern>,
    depth: usize,
) -> Option<String> {
    let items = value.as_array()?;
    if let Some(minimum) = node.get("minItems").and_then(Value::as_u64)
        && items.len() < minimum as usize
    {
        return Some(format!("{path} must NOT have fewer than {minimum} items"));
    }
    if let Some(maximum) = node.get("maxItems").and_then(Value::as_u64)
        && items.len() > maximum as usize
    {
        return Some(format!("{path} must NOT have more than {maximum} items"));
    }
    if node.get("uniqueItems") == Some(&Value::Bool(true)) {
        for (index, item) in items.iter().enumerate() {
            if items[..index].contains(item) {
                return Some(format!("{path} must NOT have duplicate items"));
            }
        }
    }
    if let Some(item_schema) = node.get("items") {
        for (index, item) in items.iter().enumerate() {
            let schema = item_schema
                .as_array()
                .and_then(|tuple| tuple.get(index))
                .or_else(|| {
                    item_schema
                        .as_array()
                        .and_then(|_| node.get("additionalItems"))
                })
                .unwrap_or(item_schema);
            if let Some(error) = validate_node(
                schema,
                item,
                &format!("{path}/{index}"),
                root,
                patterns,
                depth + 1,
            ) {
                return Some(error);
            }
        }
    }
    if let Some(contains) = node.get("contains")
        && !items
            .iter()
            .any(|item| validate_node(contains, item, path, root, patterns, depth + 1).is_none())
    {
        return Some(format!("{path} must contain a valid item"));
    }
    None
}

fn validate_object(
    node: &Map<String, Value>,
    value: &Value,
    path: &str,
    root: &Value,
    compiled_patterns: &HashMap<String, CompatiblePattern>,
    depth: usize,
) -> Option<String> {
    let object = value.as_object()?;
    if let Some(minimum) = node.get("minProperties").and_then(Value::as_u64)
        && object.len() < minimum as usize
    {
        return Some(format!(
            "{path} must NOT have fewer than {minimum} properties"
        ));
    }
    if let Some(maximum) = node.get("maxProperties").and_then(Value::as_u64)
        && object.len() > maximum as usize
    {
        return Some(format!(
            "{path} must NOT have more than {maximum} properties"
        ));
    }
    let properties = node.get("properties").and_then(Value::as_object);
    if let Some(required) = node.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(name) {
                return Some(format!("{path} must have required property '{name}'"));
            }
        }
    }
    if let Some(properties) = properties {
        for (name, child) in properties {
            if let Some(item) = object.get(name)
                && let Some(error) = validate_node(
                    child,
                    item,
                    &format!("{path}/{name}"),
                    root,
                    compiled_patterns,
                    depth + 1,
                )
            {
                return Some(error);
            }
        }
    }
    let patterns = node.get("patternProperties").and_then(Value::as_object);
    if let Some(patterns) = patterns {
        for (pattern, child) in patterns {
            let Some(pattern) = compiled_patterns.get(pattern) else {
                return Some(format!("{path} patternProperty was not compiled"));
            };
            for (name, item) in object.iter().filter(|(name, _)| pattern.is_match(name)) {
                if let Some(error) = validate_node(
                    child,
                    item,
                    &format!("{path}/{name}"),
                    root,
                    compiled_patterns,
                    depth + 1,
                ) {
                    return Some(error);
                }
            }
        }
    }
    if let Some(additional) = node.get("additionalProperties") {
        for (name, item) in object {
            let declared = properties.is_some_and(|values| values.contains_key(name));
            let patterned = patterns.is_some_and(|values| {
                values.keys().any(|pattern| {
                    compiled_patterns
                        .get(pattern)
                        .is_some_and(|pattern| pattern.is_match(name))
                })
            });
            if !declared
                && !patterned
                && let Some(error) = validate_node(
                    additional,
                    item,
                    &format!("{path}/{name}"),
                    root,
                    compiled_patterns,
                    depth + 1,
                )
            {
                return Some(error);
            }
        }
    }
    if let Some(names) = node.get("propertyNames") {
        for name in object.keys() {
            if let Some(error) = validate_node(
                names,
                &Value::String(name.clone()),
                path,
                root,
                compiled_patterns,
                depth + 1,
            ) {
                return Some(error);
            }
        }
    }
    if let Some(dependencies) = node.get("dependencies").and_then(Value::as_object) {
        for (name, dependency) in dependencies {
            if !object.contains_key(name) {
                continue;
            }
            if let Some(required) = dependency.as_array() {
                for required in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(required) {
                        return Some(format!(
                            "{path} must have property {required} when property {name} is present"
                        ));
                    }
                }
            } else if let Some(error) =
                validate_node(dependency, value, path, root, compiled_patterns, depth + 1)
            {
                return Some(error);
            }
        }
    }
    None
}

fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    if reference == "#" {
        Some(root)
    } else {
        root.pointer(reference.strip_prefix('#')?)
    }
}

fn matches_type(kind: &str, value: &Value) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_number().is_some_and(|number| {
            number.is_i64()
                || number.is_u64()
                || number.as_f64().is_some_and(|value| value.fract() == 0.0)
        }),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pattern_properties_compile_and_validate() {
        let schema = CompiledOutputSchema::compile(json!({
            "type":"object", "patternProperties":{"^x-":{"type":"string"}}
        }))
        .unwrap();
        assert_eq!(schema.validate(&json!({"x-kept":"k"})), None);
        assert_eq!(
            schema.validate(&json!({"x-kept":1})).as_deref(),
            Some("data/x-kept must be string")
        );
    }

    #[test]
    fn invalid_pattern_and_type_are_compile_errors() {
        assert!(CompiledOutputSchema::compile(json!({"type":"string","pattern":"("})).is_err());
        assert!(CompiledOutputSchema::compile(json!({"type":42})).is_err());
    }

    #[test]
    fn leading_javascript_lookahead_is_compiled_and_validated() {
        let schema =
            CompiledOutputSchema::compile(json!({"type":"string","pattern":"^(?!tmp)"})).unwrap();
        assert_eq!(schema.validate(&json!("abc")), None);
        assert_eq!(
            schema.validate(&json!("tmp-file")).as_deref(),
            Some("data must match pattern \"^(?!tmp)\"")
        );
    }

    #[test]
    fn arbitrary_local_ref_targets_are_compiled_before_validation() {
        let schema = CompiledOutputSchema::compile(json!({
            "x":{"type":"string","pattern":"^(?!tmp)"},
            "$ref":"#/x"
        }))
        .unwrap();
        assert_eq!(schema.validate(&json!("abc")), None);
        assert!(schema.validate(&json!("tmp-file")).is_some());
        assert!(CompiledOutputSchema::compile(json!({"x":{"pattern":"("},"$ref":"#/x"})).is_err());
    }

    #[test]
    fn validation_messages_match_ajv_paths() {
        let schema = CompiledOutputSchema::compile(json!({
            "type":"object", "properties":{"answer":{"type":"integer"}}
        }))
        .unwrap();
        assert_eq!(
            schema.validate(&json!({"answer":"no"})).as_deref(),
            Some("data/answer must be integer")
        );
    }

    #[test]
    fn draft_seven_constraints_are_enforced() {
        let schema = CompiledOutputSchema::compile(json!({
            "$defs":{"large":{"type":"number","minimum":10}},
            "type":"object",
            "properties":{
                "amount":{"$ref":"#/$defs/large"},
                "tag":{"type":"string","maxLength":3},
                "ids":{"type":"array","uniqueItems":true}
            },
            "required":["amount"]
        }))
        .unwrap();
        assert!(
            schema
                .validate(&json!({"amount":1}))
                .unwrap()
                .contains(">= 10")
        );
        assert!(
            schema
                .validate(&json!({"amount":10,"tag":"long"}))
                .unwrap()
                .contains("more than 3")
        );
        assert!(
            schema
                .validate(&json!({"amount":10,"ids":[1,1]}))
                .unwrap()
                .contains("duplicate")
        );
    }
}
