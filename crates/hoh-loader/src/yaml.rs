use crate::{Entry, Expr, LoadError, Patch, expression};
use indexmap::{IndexMap, IndexSet};
use saphyr_parser::{Event, Marker, Parser, ScalarStyle, Tag};
use serde_json::{Value, json};
use std::{collections::VecDeque, path::Path, rc::Rc};

const YAML_MAX_DEPTH: usize = 128;
const YAML_MAX_EXPANDED_NODES: usize = 100_000;
type NodeRef = Rc<Node>;

#[derive(Clone)]
struct Node {
    kind: NodeKind,
    mark: Marker,
    height: usize,
    size: usize,
}
#[derive(Clone)]
enum NodeKind {
    Scalar(Value),
    Seq(Vec<NodeRef>),
    Map(IndexMap<String, NodeRef>),
}
impl Node {
    fn value(&self) -> Value {
        match &self.kind {
            NodeKind::Scalar(v) => v.clone(),
            NodeKind::Seq(items) => Value::Array(items.iter().map(|n| n.value()).collect()),
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
    fn map(&self, file: &Path) -> Result<&IndexMap<String, NodeRef>, LoadError> {
        match &self.kind {
            NodeKind::Map(map) => Ok(map),
            _ => Err(self.shape(file, "entry must be a mapping")),
        }
    }
    fn seq(&self, file: &Path) -> Result<&[NodeRef], LoadError> {
        match &self.kind {
            NodeKind::Seq(items) => Ok(items),
            _ => Err(self.shape(file, "config file must be a top-level YAML array")),
        }
    }
}
struct Builder<'a> {
    events: VecDeque<(Event<'a>, Marker)>,
    anchors: IndexMap<usize, NodeRef>,
    file: &'a Path,
    expanded_nodes: usize,
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
    fn node(&mut self, depth: usize) -> Result<NodeRef, LoadError> {
        let (event, mark) = self
            .events
            .pop_front()
            .ok_or_else(|| self.yaml(Marker::default(), "expected YAML value"))?;
        if depth >= YAML_MAX_DEPTH {
            return Err(self.yaml(mark, "YAML nesting limit exceeded"));
        }
        let cost = if let Event::Alias(id) = &event {
            let node = self
                .anchors
                .get(id)
                .ok_or_else(|| self.yaml(mark, "unknown or cyclic YAML alias"))?;
            if depth + node.height > YAML_MAX_DEPTH {
                return Err(self.yaml(mark, "expanded YAML nesting limit exceeded"));
            }
            node.size
        } else {
            1
        };
        self.expanded_nodes = self.expanded_nodes.saturating_add(cost);
        if self.expanded_nodes > YAML_MAX_EXPANDED_NODES {
            return Err(self.yaml(mark, "expanded YAML node limit exceeded (100000)"));
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
        let (height, size) = match &kind {
            NodeKind::Scalar(_) => (1, 1),
            NodeKind::Seq(items) => (
                1 + items.iter().map(|n| n.height).max().unwrap_or(0),
                1 + items.iter().map(|n| n.size).sum::<usize>(),
            ),
            NodeKind::Map(items) => (
                1 + items.values().map(|n| n.height).max().unwrap_or(0),
                1 + items.values().map(|n| n.size).sum::<usize>(),
            ),
        };
        let node = Rc::new(Node {
            kind,
            mark,
            height,
            size,
        });
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
fn document<'a>(text: &'a str, file: &'a Path) -> Result<NodeRef, LoadError> {
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
        expanded_nodes: 0,
    };
    builder.events.pop_front(); // StreamStart is guaranteed by saphyr-parser.
    if matches!(builder.events.front(), Some((Event::StreamEnd, _))) {
        return Ok(Rc::new(Node {
            kind: NodeKind::Scalar(Value::Null),
            mark: Marker::new(0, 1, 0),
            height: 1,
            size: 1,
        }));
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
    let source = match &node.kind {
        NodeKind::Scalar(value) => expression(value),
        NodeKind::Map(map) if map.len() == 1 && map.contains_key("__jsExpr") => {
            match &map["__jsExpr"].kind {
                NodeKind::Scalar(Value::String(source)) => Some(source.as_str()),
                _ => return Err(node.shape(file, "__jsExpr must contain expression text")),
            }
        }
        _ => None,
    };
    if let Some(source) = source {
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

// js-yaml 4.3.2 lib/type/int.js:20-116 and float.js:6-51.
fn integer(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned == "0" {
        return Some(0.0);
    } // parseYamlInteger deliberately discards -0's sign.
    let sign = if text.starts_with('-') { -1.0 } else { 1.0 };
    for (prefix, width, radix) in [("0b", 1, 2), ("0o", 3, 8), ("0x", 4, 16)] {
        if let Some(digits) = unsigned.strip_prefix(prefix) {
            if digits.is_empty()
                || !digits
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && char::from(b).is_digit(radix))
            {
                return None;
            }
            let number = sign * radix_number(digits, width);
            return number.is_finite().then_some(number);
        }
    }
    if unsigned.is_empty() || !unsigned.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}
// Round a power-of-two radix integer once, rather than rounding at every digit.
fn radix_number(digits: &str, width: usize) -> f64 {
    let mut significant = 0usize;
    let mut top = 0u64;
    let mut guard = false;
    let mut sticky = false;
    let mut started = false;
    for digit in digits.chars() {
        let value = digit.to_digit(16).unwrap_or(0);
        for bit in (0..width).rev() {
            let one = value & (1 << bit) != 0;
            if !started && !one {
                continue;
            }
            started = true;
            significant += 1;
            if significant <= 53 {
                top = (top << 1) | u64::from(one);
            } else if significant == 54 {
                guard = one;
            } else {
                sticky |= one;
            }
        }
    }
    if significant <= 53 {
        return top as f64;
    }
    if guard && (sticky || top & 1 != 0) {
        top += 1;
    }
    let exponent = i32::try_from(significant - 53).unwrap_or(i32::MAX);
    (top as f64) * 2.0f64.powi(exponent)
}
fn float(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") {
        return Some(if text.starts_with('-') {
            -f64::INFINITY
        } else {
            f64::INFINITY
        });
    }
    if matches!(text, ".nan" | ".NaN" | ".NAN") {
        return Some(f64::NAN);
    }
    let mut exponent = unsigned.split(['e', 'E']);
    let mantissa = exponent.next()?;
    if let Some(power) = exponent.next() {
        let digits = power.strip_prefix(['+', '-']).unwrap_or(power);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    if exponent.next().is_some() {
        return None;
    }
    let mut point = mantissa.split('.');
    let left = point.next()?;
    let right = point.next();
    if point.next().is_some() {
        return None;
    }
    if left.is_empty() {
        if text != unsigned || right.is_none_or(|s| s.is_empty()) {
            return None;
        }
    } else if !left.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if right.is_some_and(|s| !s.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}
fn resolved(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        "true" | "True" | "TRUE" => json!(true),
        "false" | "False" | "FALSE" => json!(false),
        _ => match integer(text).or_else(|| float(text)) {
            Some(n) => finite_number(n),
            None => json!(text),
        },
    }
}
pub(crate) fn finite_number(n: f64) -> Value {
    if n == 0.0 && n.is_sign_negative() {
        json!(-0.0)
    } else {
        let text = hoh_util_values::js_number_string(n);
        if let Ok(integer) = text.parse::<i64>() {
            json!(integer)
        } else if let Ok(integer) = text.parse::<u64>() {
            json!(integer)
        } else {
            json!(n)
        }
    }
}
fn scalar(text: &str, style: ScalarStyle, tag: Option<&Tag>) -> Result<Value, String> {
    let Some(tag) = tag else {
        if style != ScalarStyle::Plain {
            return Ok(json!(text));
        }
        if float(text).is_some_and(|n| !n.is_finite()) {
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
        "null"
            if matches!(text, "~" | "null" | "Null" | "NULL")
                || (text.is_empty() && style == ScalarStyle::Plain) =>
        {
            Ok(Value::Null)
        }
        "bool" if resolved(text).is_boolean() => Ok(resolved(text)),
        "int" | "float" => (if tag.suffix == "int" {
            integer(text)
        } else {
            float(text)
        })
        .filter(|n| n.is_finite())
        .map(finite_number)
        .ok_or_else(|| "invalid numeric scalar".into()),
        _ => Err(format!("unsupported or invalid YAML tag {tag}")),
    }
}

fn validate_row(node: &Node, file: &Path, patch: bool) -> Result<(), LoadError> {
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
    // Do not materialize config at each group ancestor: each subtree is converted once below.
    let mut value = Value::Object(
        map.iter()
            .filter(|(key, _)| key.as_str() != "config")
            .map(|(key, node)| (key.clone(), node.value()))
            .collect(),
    );
    if patch && let Some(map) = value.as_object_mut() {
        map.retain(|_, v| !v.is_null());
        map.entry("name").or_insert(json!(""));
    }
    let row: Entry = serde_json::from_value(value).map_err(|e| node.shape(file, e.to_string()))?;
    if !row.disabled.is_null() && !row.disabled.is_boolean() && expression(&row.disabled).is_none()
    {
        return Err(map
            .get("disabled")
            .map_or(node, |n| n.as_ref())
            .shape(file, "disabled must be bool, null or an expression"));
    }
    if let Some(intercept) = &row.intercept
        && intercept.values().any(|v| !v.is_object())
    {
        return Err(map
            .get("intercept")
            .map_or(node, |n| n.as_ref())
            .shape(file, "intercept values must be objects"));
    }
    if row.group
        && let Some(config) = map.get("config")
        && !matches!(config.kind, NodeKind::Scalar(Value::Null))
    {
        for child in config.seq(file)? {
            validate_row(child, file, false)?;
        }
    }
    Ok(())
}
fn entry(node: &Node, file: &Path) -> Result<Entry, LoadError> {
    validate_row(node, file, false)?;
    serde_json::from_value(node.value()).map_err(|e| node.shape(file, e.to_string()))
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
    for row in rows.iter() {
        if !row.id.is_empty() {
            used.insert(row.id.clone());
        }
        if row.group {
            collect_ids(&row.config, &mut used);
        }
    }
    let mut next = seed.bytes().fold(0x811c9dc5u32, |hash, b| {
        (hash ^ u32::from(b)).wrapping_mul(0x01000193)
    });
    fn fresh(used: &mut IndexSet<String>, next: &mut u32) -> String {
        loop {
            *next = next.wrapping_add(1);
            let id = format!("{next:08x}");
            if used.insert(id.clone()) {
                return id;
            }
        }
    }
    fn walk(value: &mut Value, used: &mut IndexSet<String>, next: &mut u32) {
        if let Some(rows) = value.as_array_mut() {
            for row in rows {
                if row
                    .get("id")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    row["id"] = Value::String(fresh(used, next));
                }
                if row.get("group") == Some(&Value::Bool(true)) {
                    walk(&mut row["config"], used, next);
                }
            }
        }
    }
    for row in rows {
        if row.id.is_empty() {
            row.id = fresh(&mut used, &mut next);
        }
        if row.group {
            walk(&mut row.config, &mut used, &mut next);
        }
    }
}
pub fn parse_entries(text: &str, file: &Path) -> Result<Vec<Entry>, LoadError> {
    let doc = document(text, file)?;
    let mut rows = doc
        .seq(file)?
        .iter()
        .map(|n| entry(n, file))
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
            .filter(|n| !matches!(n.kind, NodeKind::Scalar(Value::Null)))
            .map(|n| {
                n.value()
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| n.shape(file, "patch id must be a string"))
            })
            .transpose()?;
        let name = fields
            .shift_remove("name")
            .filter(|n| !matches!(n.kind, NodeKind::Scalar(Value::Null)))
            .map(|n| {
                n.value()
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| n.shape(file, "patch name must be a string"))
            })
            .transpose()?;
        let insert = fields
            .shift_remove("insert")
            .filter(|n| !matches!(n.kind, NodeKind::Scalar(Value::Null)))
            .map(|n| {
                n.seq(file)?
                    .iter()
                    .map(|n| entry(n, file))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        // Validate patch fields with the same row schema, allowing null to remove a field.
        validate_row(
            &Node {
                kind: NodeKind::Map(fields),
                mark: node.mark,
                height: node.height,
                size: node.size,
            },
            file,
            true,
        )?;
        let set = node
            .map(file)?
            .iter()
            .filter(|(key, _)| !["id", "name", "insert"].contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.value()))
            .collect();
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
    anchor_name(&mut row.name, file)?;
    fn walk(value: &mut Value, file: &Path) -> Result<(), LoadError> {
        if let Some(rows) = value.as_array_mut() {
            for row in rows {
                if let Some(Value::String(name)) = row.get_mut("name") {
                    anchor_name(name, file)?;
                }
                if row.get("group") == Some(&Value::Bool(true)) {
                    walk(&mut row["config"], file)?;
                }
            }
        }
        Ok(())
    }
    if row.group {
        walk(&mut row.config, file)?;
    }
    Ok(())
}
fn anchor_name(name: &mut String, file: &Path) -> Result<(), LoadError> {
    if Path::new(name).is_absolute() || name.starts_with("./") || name.starts_with("../") {
        let base = file.parent().unwrap_or(Path::new("."));
        let path = if Path::new(name).is_absolute() {
            name.clone()
        } else {
            format!("{}/{}", base.display(), name)
        };
        let path = if cfg!(windows) {
            crate::expr::windows_normalize(&path)
        } else {
            crate::expr::normalize(&path)
        };
        *name = url::Url::from_file_path(&path)
            .map_err(|_| LoadError::Shape {
                file: file.into(),
                line: 1,
                column: 1,
                message: "patch file must have an absolute path to anchor plugin names".into(),
            })?
            .into();
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
                // js-yaml int.js:104 drops -0's sign; float.js:76 preserves it as -0.0.
                Some(if number == 0.0 && number.is_sign_negative() {
                    "-0.0".into()
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
    let mut result = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\0' => result.push_str("\\0"),
            '\u{7}' => result.push_str("\\a"),
            '\u{8}' => result.push_str("\\b"),
            '\t' => result.push_str("\\t"),
            '\n' => result.push_str("\\n"),
            '\u{b}' => result.push_str("\\v"),
            '\u{c}' => result.push_str("\\f"),
            '\r' => result.push_str("\\r"),
            '\u{1b}' => result.push_str("\\e"),
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{85}' => result.push_str("\\N"),
            '\u{a0}' => result.push_str("\\_"),
            '\u{2028}' => result.push_str("\\L"),
            '\u{2029}' => result.push_str("\\P"),
            ch if ch < '\u{20}'
                || ('\u{7f}'..='\u{9f}').contains(&ch)
                || matches!(ch, '\u{feff}' | '\u{fffe}' | '\u{ffff}') =>
            {
                let code = u32::from(ch);
                if code <= 0xff {
                    result.push_str(&format!("\\x{code:02X}"));
                } else {
                    result.push_str(&format!("\\u{code:04X}"));
                }
            }
            ch => result.push(ch),
        }
    }
    result.push('"');
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_resolver_boundaries() {
        for (text, expected) in [
            ("01", json!(1)),
            ("+0xF", json!(15)),
            ("0b1_0", json!("0b1_0")),
            ("0o7_1", json!("0o7_1")),
            ("1__2", json!("1__2")),
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
