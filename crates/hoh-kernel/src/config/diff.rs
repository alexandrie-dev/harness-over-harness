use super::schema::{children, nullable_choice, target};
use hoh_util_values::deep_equal_json;
use serde_json::Value;
fn equal(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => deep_equal_json(a, b),
        (None, None) => true,
        _ => false,
    }
}
fn defaulted<'a>(s: &'a Value, v: Option<&'a Value>) -> Option<&'a Value> {
    v.filter(|v| !v.is_null()).or_else(|| s.get("default"))
}
fn record(v: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    v.and_then(Value::as_object)
        .filter(|o| !o.contains_key("__jsExpr"))
}
fn recursive_reference(root: &Value, reference: &str) -> bool {
    let Some(start) = target(root, reference) else {
        return false;
    };
    let mut pending = vec![start];
    let mut seen = std::collections::HashSet::new();
    while let Some(schema) = pending.pop() {
        if !seen.insert(std::ptr::from_ref(schema)) {
            continue;
        }
        if let Some(r) = schema["$ref"].as_str()
            && let Some(next) = target(root, r)
        {
            if std::ptr::eq(next, start) {
                return true;
            }
            pending.push(next);
        }
        pending.extend(children(schema));
    }
    false
}
pub fn equal_except_volatile(s: &Value, a: &Value, b: &Value) -> bool {
    compare(s, s, Some(a), Some(b), &mut vec!["#".into()])
}
fn compare(
    root: &Value,
    s: &Value,
    a: Option<&Value>,
    b: Option<&Value>,
    refs: &mut Vec<String>,
) -> bool {
    if s["x-volatile"] == true {
        return true;
    }
    if let Some(child) = nullable_choice(s) {
        return compare(root, child, a, b, refs);
    }
    if let Some(r) = s["$ref"].as_str() {
        if refs.iter().any(|p| p == r) || recursive_reference(root, r) {
            return equal(a, b);
        }
        if let Some(t) = target(root, r) {
            refs.push(r.into());
            let answer = compare(root, t, a, b, refs);
            refs.pop();
            return answer;
        }
        return equal(a, b);
    }
    let object = s.get("type").is_none_or(|t| {
        t == "object"
            || t.as_array()
                .is_some_and(|a| a.iter().any(|t| t == "object"))
    });
    let Some(props) = s["properties"].as_object().filter(|_| object) else {
        return equal(a, b);
    };
    let (a, b) = (defaulted(s, a), defaulted(s, b));
    let (Some(left), Some(right)) = (record(a), record(b)) else {
        return equal(a, b);
    };
    let keys: indexmap::IndexSet<&str> = left
        .keys()
        .chain(right.keys())
        .map(String::as_str)
        .collect();
    keys.into_iter().all(|key| {
        compare(
            root,
            props.get(key).unwrap_or(&Value::Null),
            left.get(key),
            right.get(key),
            refs,
        )
    })
}
pub fn volatile_paths(s: &Value, a: &Value, b: &Value) -> Vec<String> {
    let mut out = vec![];
    visit(
        s,
        s,
        Some(a),
        Some(b),
        String::new(),
        &mut vec!["#".into()],
        &mut out,
    );
    out
}
fn visit(
    root: &Value,
    s: &Value,
    a: Option<&Value>,
    b: Option<&Value>,
    path: String,
    refs: &mut Vec<String>,
    out: &mut Vec<String>,
) {
    if equal(a, b) {
        return;
    }
    if s["x-volatile"] == true {
        out.push(path);
        return;
    }
    if let Some(child) = nullable_choice(s) {
        visit(root, child, a, b, path, refs, out);
        return;
    }
    if let Some(r) = s["$ref"].as_str() {
        if !refs.contains(&r.to_owned())
            && !recursive_reference(root, r)
            && let Some(t) = target(root, r)
        {
            refs.push(r.into());
            visit(root, t, a, b, path, refs, out);
            refs.pop();
        }
        return;
    }
    if let Some(props) = s["properties"].as_object() {
        for (k, child) in props {
            visit(
                root,
                child,
                a.and_then(|v| v.get(k)),
                b.and_then(|v| v.get(k)),
                if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                },
                refs,
                out,
            );
        }
    }
    let len = [a, b]
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .map(Vec::len)
        .max()
        .unwrap_or(0);
    for i in 0..len {
        if let Some(child) = s["prefixItems"].get(i).or_else(|| s.get("items")) {
            visit(
                root,
                child,
                a.and_then(|v| v.get(i)),
                b.and_then(|v| v.get(i)),
                if path.is_empty() {
                    i.to_string()
                } else {
                    format!("{path}.{i}")
                },
                refs,
                out,
            );
        }
    }
}
