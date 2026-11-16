use crate::{Entry, Expr, LoadError, Patch, expression};
use indexmap::{IndexMap, IndexSet};
use saphyr_parser::{Event, Marker, Parser, ScalarStyle, Tag};
use serde_json::{Value, json};
use std::{collections::VecDeque, path::Path};

#[derive(Clone)]
struct Node {
    kind: NodeKind,
    mark: Marker,
}
#[derive(Clone)]
enum NodeKind {
    Scalar(Value),
    Seq(Vec<Node>),
    Map(IndexMap<String, Node>),
}
impl Node {
    fn value(&self) -> Value {
        match &self.kind {
            NodeKind::Scalar(v) => v.clone(),
            NodeKind::Seq(items) => Value::Array(items.iter().map(Self::value).collect()),
            NodeKind::Map(map) => {
                Value::Object(map.iter().map(|(k, v)| (k.clone(), v.value())).collect())
            }
        }
    }
    fn shape(&self, file: &Path, message: impl Into<String>) -> LoadError {
        LoadError::Shape {
            file: file.into(),
            line: self.mark.line().max(1),
            column: self.mark.col() + 1,
            message: message.into(),
        }
    }
    fn map(&self, file: &Path) -> Result<&IndexMap<String, Node>, LoadError> {
        match &self.kind {
            NodeKind::Map(map) => Ok(map),
            _ => Err(self.shape(file, "entry must be a mapping")),
        }
    }
    fn seq(&self, file: &Path) -> Result<&[Node], LoadError> {
        match &self.kind {
            NodeKind::Seq(items) => Ok(items),
            _ => Err(self.shape(file, "config file must be a top-level YAML array")),
        }
    }
}
struct Builder<'a> {
    events: VecDeque<(Event<'a>, Marker)>,
    anchors: IndexMap<usize, Node>,
    file: &'a Path,
}
impl Builder<'_> {
    fn yaml(&self, mark: Marker, message: impl Into<String>) -> LoadError {
        LoadError::Yaml {
            file: self.file.into(),
            line: mark.line().max(1),
            column: mark.col() + 1,
            message: message.into(),
        }
    }
    fn node(&mut self, depth: usize) -> Result<Node, LoadError> {
        let (event, mark) = self
            .events
            .pop_front()
            .ok_or_else(|| self.yaml(Marker::default(), "expected YAML value"))?;
        if depth > 128 {
            return Err(self.yaml(mark, "YAML nesting limit exceeded"));
        }
        let (kind, anchor) = match event {
            Event::Scalar(text, style, anchor, tag) => (
                NodeKind::Scalar(
                    scalar(&text, style, tag.as_deref()).map_err(|m| self.yaml(mark, m))?,
                ),
                anchor,
            ),
            Event::Alias(id) => {
                return self
                    .anchors
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| self.yaml(mark, "unknown or cyclic YAML alias"));
            }
            Event::SequenceStart(anchor, tag) => {
                self.collection_tag(tag.as_deref(), "seq", mark)?;
                let mut items = Vec::new();
                while !matches!(self.events.front(), Some((Event::SequenceEnd, _))) {
                    items.push(self.node(depth + 1)?);
                }
                self.events.pop_front();
                (NodeKind::Seq(items), anchor)
            }
            Event::MappingStart(anchor, tag) => {
                self.collection_tag(tag.as_deref(), "map", mark)?;
                let mut map = IndexMap::new();
                while !matches!(self.events.front(), Some((Event::MappingEnd, _))) {
                    let key = self.node(depth + 1)?;
                    let NodeKind::Scalar(Value::String(name)) = &key.kind else {
                        return Err(self.yaml(key.mark, "mapping key must be a string"));
                    };
                    if map.contains_key(name) {
                        return Err(self.yaml(key.mark, format!("duplicate mapping key {name:?}")));
                    }
                    map.insert(name.clone(), self.node(depth + 1)?);
                }
                self.events.pop_front();
                (NodeKind::Map(map), anchor)
            }
            _ => return Err(self.yaml(mark, "expected YAML value")),
        };
        let node = Node { kind, mark };
        if anchor != 0 {
            self.anchors.insert(anchor, node.clone());
        }
        Ok(node)
    }
    fn collection_tag(
        &self,
        tag: Option<&Tag>,
        expected: &str,
        mark: Marker,
    ) -> Result<(), LoadError> {
        if tag.is_some_and(|tag| !tag.is_yaml_core_schema() || tag.suffix != expected) {
            return Err(self.yaml(mark, "unsupported collection tag"));
        }
        Ok(())
    }
}
fn document<'a>(text: &'a str, file: &'a Path) -> Result<Node, LoadError> {
    let mut events = VecDeque::new();
    for event in Parser::new_from_str(text) {
        let (event, span) = event.map_err(|e| LoadError::Yaml {
            file: file.into(),
            line: e.marker().line(),
            column: e.marker().col() + 1,
            message: e.info().into(),
        })?;
        events.push_back((event, span.start));
    }
    let mut builder = Builder {
        events,
        anchors: IndexMap::new(),
        file,
    };
    builder.events.pop_front(); // StreamStart is guaranteed by saphyr-parser.
    if matches!(builder.events.front(), Some((Event::StreamEnd, _))) {
        return Ok(Node {
            kind: NodeKind::Scalar(Value::Null),
            mark: Marker::new(0, 1, 0),
        });
    }
    builder.events.pop_front(); // DocumentStart is guaranteed by saphyr-parser.
    let node = builder.node(0)?;
    builder.events.pop_front(); // DocumentEnd is guaranteed by saphyr-parser.
    if !matches!(builder.events.front(), Some((Event::StreamEnd, _))) {
        return Err(builder.yaml(
            builder.events.front().map(|(_, m)| *m).unwrap_or_default(),
            "expected a single YAML document",
        ));
    }
    validate_expressions(&node, file)?;
    Ok(node)
}
fn validate_expressions(node: &Node, file: &Path) -> Result<(), LoadError> {
    let value = node.value();
    if value
        .as_object()
        .is_some_and(|map| map.len() == 1 && map.contains_key("__jsExpr"))
        && expression(&value).is_none()
    {
        return Err(node.shape(file, "__jsExpr must contain expression text"));
    }
    if let Some(source) = expression(&value) {
        Expr::parse(source).map_err(|reason| LoadError::Expr {
            file: file.into(),
            line: node.mark.line().max(1),
            column: node.mark.col() + 1,
            expression: source.into(),
            reason,
        })?;
    } else {
        match &node.kind {
            NodeKind::Seq(items) => {
                for item in items {
                    validate_expressions(item, file)?;
                }
            }
            NodeKind::Map(items) => {
                for item in items.values() {
                    validate_expressions(item, file)?;
                }
            }
            NodeKind::Scalar(_) => {}
        }
    }
    Ok(())
}

// js-yaml 4 JSON_SCHEMA includes the core int/float resolvers, including underscores,
// leading zeroes, binary/octal/hex integers, .inf and .nan (not YAML 1.1 sexagesimal).
fn numeric(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let sign = if text.starts_with('-') { -1.0 } else { 1.0 };
    if unsigned.is_empty() || unsigned.starts_with('_') || unsigned.ends_with('_') {
        return None;
    }
    for (prefix, radix) in [("0b", 2), ("0o", 8), ("0x", 16)] {
        if let Some(digits) = unsigned.strip_prefix(prefix) {
            if digits.is_empty() {
                return None;
            }
            return digits
                .chars()
                .filter(|c| *c != '_')
                .try_fold(0.0, |n, c| {
                    c.to_digit(radix)
                        .map(|d| n * f64::from(radix) + f64::from(d))
                })
                .map(|n| sign * n);
        }
    }
    if unsigned.chars().all(|c| c.is_ascii_digit() || c == '_') {
        return unsigned
            .replace('_', "")
            .parse::<f64>()
            .ok()
            .map(|n| sign * n);
    }
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") {
        return Some(sign * f64::INFINITY);
    }
    if matches!(text, ".nan" | ".NaN" | ".NAN") {
        return Some(f64::NAN);
    }
    let mut exponent = unsigned.split(['e', 'E']);
    let mantissa = exponent.next()?;
    let power = exponent.next();
    if exponent.next().is_some() {
        return None;
    }
    if let Some(power) = power {
        let digits = power.strip_prefix(['+', '-']).unwrap_or(power);
        if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    let mut point = mantissa.split('.');
    let left = point.next()?;
    let right = point.next();
    if point.next().is_some() || (right.is_none() && power.is_none()) {
        return None;
    }
    if !left.is_empty()
        && (!left.as_bytes()[0].is_ascii_digit()
            || !left.bytes().all(|c| c.is_ascii_digit() || c == b'_'))
    {
        return None;
    }
    if left.is_empty() && right.is_none() {
        return None;
    }
    if right.is_some_and(|s| !s.bytes().all(|c| c.is_ascii_digit() || c == b'_')) {
        return None;
    }
    // js-yaml's float pattern also admits '.', '+.' and '.e2'; its constructor yields NaN.
    Some(text.replace('_', "").parse().unwrap_or(f64::NAN))
}
fn resolved(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        "true" | "True" | "TRUE" => json!(true),
        "false" | "False" | "FALSE" => json!(false),
        _ => match numeric(text) {
            Some(n) => finite_number(n),
            None => json!(text),
        },
    }
}
fn finite_number(n: f64) -> Value {
    if n == 0.0 && n.is_sign_negative() {
        json!(-0.0)
    } else if n.fract() == 0.0 && n >= i64::MIN as f64 && n < i64::MAX as f64 {
        json!(n as i64)
    } else if n.fract() == 0.0 && n >= 0.0 && n < u64::MAX as f64 {
        json!(n as u64)
    } else {
        json!(n)
    }
}
fn scalar(text: &str, style: ScalarStyle, tag: Option<&Tag>) -> Result<Value, String> {
    let Some(tag) = tag else {
        if style != ScalarStyle::Plain {
            return Ok(json!(text));
        }
        if numeric(text).is_some_and(|n| !n.is_finite()) {
            return Err("non-finite YAML number cannot be represented as JSON".into());
        }
        return Ok(resolved(text));
    };
    if tag.handle == "!" && tag.suffix.is_empty() {
        return Ok(json!(text));
    }
    if !tag.is_yaml_core_schema() {
        return Err(format!("unknown YAML tag {tag}"));
    }
    match tag.suffix.as_str() {
        "js" if !text.trim().is_empty() => Ok(json!({"__jsExpr":text})),
        "js" => Err("!!js requires an expression body".into()),
        "str" => Ok(json!(text)),
        "null" if resolved(text).is_null() && numeric(text).is_none() => Ok(Value::Null),
        "bool" if resolved(text).is_boolean() => Ok(resolved(text)),
        "int" | "float" => numeric(text)
            .filter(|_| {
                let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
                let integer = unsigned.starts_with("0x")
                    || unsigned.starts_with("0o")
                    || unsigned.starts_with("0b")
                    || unsigned.bytes().all(|c| c.is_ascii_digit() || c == b'_');
                integer == (tag.suffix == "int")
            })
            .filter(|n| n.is_finite())
            .map(finite_number)
            .ok_or_else(|| "invalid numeric scalar".into()),
        _ => Err(format!("unsupported or invalid YAML tag {tag}")),
    }
}

fn entry(node: &Node, file: &Path, patch: bool) -> Result<Entry, LoadError> {
    let map = node.map(file)?;
    for (key, value) in map {
        if ![
            "id",
            "name",
            "config",
            "disabled",
            "group",
            "inject",
            "isolate",
            "intercept",
        ]
        .contains(&key.as_str())
        {
            return Err(value.shape(file, format!("unknown row key {key:?}")));
        }
    }
    let mut value = node.value();
    if patch && let Some(map) = value.as_object_mut() {
        map.retain(|_, v| !v.is_null());
        map.entry("name").or_insert(json!(""));
    }
    let mut row: Entry =
        serde_json::from_value(value).map_err(|e| node.shape(file, e.to_string()))?;
    if !row.disabled.is_null() && !row.disabled.is_boolean() && expression(&row.disabled).is_none()
    {
        return Err(map
            .get("disabled")
            .unwrap_or(node)
            .shape(file, "disabled must be bool, null or an expression"));
    }
    if let Some(intercept) = &row.intercept
        && intercept.values().any(|v| !v.is_object())
    {
        return Err(map
            .get("intercept")
            .unwrap_or(node)
            .shape(file, "intercept values must be objects"));
    }
    if row.group && !row.config.is_null() {
        let config = map.get("config").unwrap_or(node);
        let rows = config
            .seq(file)?
            .iter()
            .map(|n| entry(n, file, false))
            .collect::<Result<Vec<_>, _>>()?;
        row.config = entries_value(&rows);
    }
    Ok(row)
}
pub(crate) fn entries_value(rows: &[Entry]) -> Value {
    Value::Array(rows.iter().map(entry_value).collect())
}
pub(crate) fn entry_value(row: &Entry) -> Value {
    // Entry's fields are all infallibly JSON serializable.
    serde_json::to_value(row).unwrap_or(Value::Null)
}
fn collect_ids(value: &Value, used: &mut IndexSet<String>) {
    if let Some(rows) = value.as_array() {
        for row in rows {
            if let Some(id) = row
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                used.insert(id.into());
            }
            if row.get("group") == Some(&Value::Bool(true)) {
                collect_ids(&row["config"], used);
            }
        }
    }
}
pub(crate) fn assign_ids(rows: &mut [Entry], seed: &str) {
    let mut used = IndexSet::new();
    collect_ids(&entries_value(rows), &mut used);
    let mut next = seed.bytes().fold(0x811c9dc5u32, |hash, b| {
        (hash ^ u32::from(b)).wrapping_mul(0x01000193)
    });
    fn walk(rows: &mut [Entry], used: &mut IndexSet<String>, next: &mut u32) {
        for row in rows {
            if row.id.is_empty() {
                loop {
                    *next = next.wrapping_add(1);
                    let id = format!("{next:08x}");
                    if used.insert(id.clone()) {
                        row.id = id;
                        break;
                    }
                }
            }
            if row.group
                && let Ok(mut children) = serde_json::from_value::<Vec<Entry>>(row.config.clone())
            {
                walk(&mut children, used, next);
                row.config = entries_value(&children);
            }
        }
    }
    walk(rows, &mut used, &mut next);
}
pub fn parse_entries(text: &str, file: &Path) -> Result<Vec<Entry>, LoadError> {
    let doc = document(text, file)?;
    let mut rows = doc
        .seq(file)?
        .iter()
        .map(|n| entry(n, file, false))
        .collect::<Result<Vec<_>, _>>()?;
    assign_ids(&mut rows, &format!("entries:{}", file.display()));
    Ok(rows)
}
pub fn parse_patches(text: &str, file: &Path) -> Result<Vec<Patch>, LoadError> {
    let doc = document(text, file)?;
    let mut patches = Vec::new();
    for node in doc.seq(file)? {
        let mut fields = node.map(file)?.clone();
        let id = fields
            .shift_remove("id")
            .map(|n| {
                n.value()
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| n.shape(file, "patch id must be a string"))
            })
            .transpose()?;
        let name = fields
            .shift_remove("name")
            .map(|n| {
                n.value()
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| n.shape(file, "patch name must be a string"))
            })
            .transpose()?;
        let insert = fields
            .shift_remove("insert")
            .map(|n| {
                n.seq(file)?
                    .iter()
                    .map(|n| entry(n, file, false))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let set = fields.iter().map(|(k, v)| (k.clone(), v.value())).collect();
        // Validate patch fields with the same row schema, allowing null to remove a field.
        entry(
            &Node {
                kind: NodeKind::Map(fields),
                mark: node.mark,
            },
            file,
            true,
        )?;
        patches.push(Patch {
            id,
            name,
            set,
            insert,
        });
    }
    // Assign once for all insert blocks, so anonymous rows share one uniqueness domain.
    let mut rows = patches
        .iter()
        .flat_map(|p| p.insert.iter().flatten().cloned())
        .collect::<Vec<_>>();
    assign_ids(&mut rows, &format!("patches:{}", file.display()));
    let mut rows = rows.into_iter();
    for patch in &mut patches {
        if let Some(insert) = &mut patch.insert {
            for row in insert {
                if let Some(assigned) = rows.next() {
                    *row = assigned;
                    anchor(row, file)?;
                }
            }
        }
    }
    Ok(patches)
}
fn anchor(row: &mut Entry, file: &Path) -> Result<(), LoadError> {
    if Path::new(&row.name).is_absolute()
        || row.name.starts_with("./")
        || row.name.starts_with("../")
    {
        let base = file.parent().unwrap_or(Path::new("."));
        let path = if Path::new(&row.name).is_absolute() {
            row.name.clone()
        } else {
            format!("{}/{}", base.display(), row.name)
        };
        let path = if cfg!(windows) {
            crate::expr::windows_normalize(&path)
        } else {
            crate::expr::normalize(&path)
        };
        row.name = url::Url::from_file_path(&path)
            .map_err(|_| LoadError::Shape {
                file: file.into(),
                line: 1,
                column: 1,
                message: "patch file must have an absolute path to anchor plugin names".into(),
            })?
            .into();
    }
    if row.group
        && let Ok(mut children) = serde_json::from_value::<Vec<Entry>>(row.config.clone())
    {
        for child in &mut children {
            anchor(child, file)?;
        }
        row.config = entries_value(&children);
    }
    Ok(())
}

pub(crate) fn write_yaml(value: &Value) -> String {
    fn atom(value: &Value) -> Option<String> {
        if let Some(expr) = expression(value) {
            return Some(format!("!!js {}", quote(expr)));
        }
        match value {
            Value::Null => Some("null".into()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Number(n) => {
                let number = n.as_f64().unwrap_or(f64::NAN);
                // YAML's integer resolver preserves the sign of -0, unlike String(-0).
                Some(if number == 0.0 && number.is_sign_negative() {
                    "-0".into()
                } else {
                    hoh_util_values::js_number_string(number)
                })
            }
            Value::String(s) => Some(if identifier(s) && resolved(s).is_string() {
                s.clone()
            } else {
                quote(s)
            }),
            Value::Array(a) if a.is_empty() => Some("[]".into()),
            Value::Object(o) if o.is_empty() => Some("{}".into()),
            _ => None,
        }
    }
    fn emit(value: &Value, indent: usize, out: &mut String) {
        let pad = " ".repeat(indent);
        if let Some(text) = atom(value) {
            out.push_str(&format!("{pad}{text}\n"));
            return;
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    if let Some(text) = atom(item) {
                        out.push_str(&format!("{pad}- {text}\n"));
                    } else if item.is_object() {
                        let mut child = String::new();
                        emit(item, indent + 2, &mut child);
                        out.push_str(&format!(
                            "{pad}- {}",
                            child.get(indent + 2..).unwrap_or(&child)
                        ));
                    } else {
                        out.push_str(&format!("{pad}-\n"));
                        emit(item, indent + 2, out);
                    }
                }
            }
            Value::Object(map) => {
                for (key, value) in map {
                    let key = if identifier(key) && resolved(key).is_string() {
                        key.clone()
                    } else {
                        quote(key)
                    };
                    if let Some(text) = atom(value) {
                        out.push_str(&format!("{pad}{key}: {text}\n"));
                    } else {
                        out.push_str(&format!("{pad}{key}:\n"));
                        emit(value, indent + 2, out);
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    emit(value, 0, &mut out);
    out
}
fn identifier(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}
fn quote(s: &str) -> String {
    Value::String(s.into()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_resolver_boundaries() {
        for (text, expected) in [
            ("01", json!(1)),
            ("+0xF", json!(15)),
            ("0b1_0", json!(2)),
            ("0o7_1", json!(57)),
            ("1__2", json!(12)),
            ("1.e2", json!(100)),
            (".2", json!(0.2)),
            ("1e2", json!(100)),
            ("1e+2", json!(100)),
            ("1:20", json!("1:20")),
            ("0XFF", json!("0XFF")),
            ("_1", json!("_1")),
            ("1_", json!("1_")),
            ("1e_2", json!("1e_2")),
            ("yes", json!("yes")),
            ("2001-01-01", json!("2001-01-01")),
        ] {
            assert_eq!(resolved(text), expected, "{text}");
        }
    }
    #[test]
    fn snapshot_documents_validate_all_expressions() {
        fn walk(dir: &Path) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path);
                } else if path.extension().is_some_and(|e| e == "yml") {
                    document(&std::fs::read_to_string(&path).unwrap(), &path)
                        .unwrap_or_else(|e| panic!("{e}"));
                }
            }
        }
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dsh/snapshots"));
    }
}
