use crate::yaml::{entries_value, entry_value, write_yaml};
use crate::{Composition, Entry, Layer, LayerSource, Origin, Patch};
use indexmap::{IndexMap, IndexSet};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};

type Row = Rc<RefCell<TreeRow>>;
struct TreeRow {
    value: Value,
    children: Option<Vec<Row>>,
}
fn build(value: Value, index: &mut IndexMap<String, Row>, indexed: bool) -> Row {
    let row = Rc::new(RefCell::new(TreeRow {
        value: value.clone(),
        children: None,
    }));
    if indexed
        && let Some(id) = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
    {
        index.insert(id.into(), row.clone());
    }
    let children = if value.get("group") == Some(&json!(true)) {
        value.get("config").and_then(Value::as_array).map(|rows| {
            rows.iter()
                .map(|v| build(v.clone(), index, indexed))
                .collect()
        })
    } else {
        None
    };
    row.borrow_mut().children = children;
    row
}
fn snapshot(row: &Row) -> Value {
    let row = row.borrow();
    let mut value = row.value.clone();
    if let Some(children) = &row.children {
        value["config"] = Value::Array(children.iter().map(snapshot).collect());
    }
    value
}
fn argument(text: &str, quoted: bool) -> String {
    if quoted {
        hoh_util_values::js_json_stringify(&Value::String(text.into()))
    } else {
        text.into()
    }
}
fn run(
    base: &[Entry],
    patches: &[Patch],
    warn: &mut dyn FnMut(String),
    quoted: bool,
    reserved: &IndexSet<String>,
) -> Vec<Entry> {
    let mut index = IndexMap::new();
    let mut roots = base
        .iter()
        .map(|row| build(entry_value(row), &mut index, true))
        .collect::<Vec<_>>();
    for patch in patches {
        let id = patch.id.as_deref().filter(|id| !id.is_empty());
        if let Some(insert) = &patch.insert {
            let target = if let Some(id) = id {
                let Some(target) = index.get(id).cloned() else {
                    warn(format!(
                        "patch insert: entry {} not found",
                        argument(id, quoted)
                    ));
                    continue;
                };
                if target.borrow().value.get("group") != Some(&json!(true)) {
                    warn(format!(
                        "patch insert: entry {} is not a group",
                        argument(id, quoted)
                    ));
                    continue;
                }
                Some(target)
            } else {
                None
            };
            let inserted = insert
                .iter()
                .map(|row| build(entry_value(row), &mut index, true))
                .collect::<Vec<_>>();
            if let Some(target) = target {
                let mut target = target.borrow_mut();
                if target.children.is_none() {
                    target.children = Some(
                        target
                            .value
                            .get("config")
                            .and_then(Value::as_array)
                            .map(|rows| {
                                rows.iter()
                                    .map(|v| build(v.clone(), &mut index, false))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    );
                }
                if let Some(children) = &mut target.children {
                    children.extend(inserted);
                }
            } else {
                roots.extend(inserted);
            }
            continue;
        }
        let Some(id) = id else {
            warn("patch: id is required for non-insert patches".into());
            continue;
        };
        let Some(target) = index.get(id) else {
            warn(format!("patch: entry {} not found", argument(id, quoted)));
            continue;
        };
        let mut target = target.borrow_mut();
        let name = target
            .value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(assertion) = &patch.name
            && !assertion.is_empty()
            && assertion != name
        {
            warn(format!(
                "patch: name mismatch for {} (expected {}, got {}), skipping",
                argument(id, quoted),
                argument(name, quoted),
                argument(assertion, quoted)
            ));
            continue;
        }
        let mut candidate = target.value.clone();
        for (key, value) in &patch.set {
            if key == "id" {
                continue;
            }
            if let Some(map) = candidate.as_object_mut() {
                if value.is_null() {
                    map.remove(key);
                } else {
                    map.insert(key.clone(), value.clone());
                }
            }
        }
        // Parsed patches already have valid types; check programmatically constructed patches too.
        if let Err(error) = serde_json::from_value::<Entry>(candidate.clone()) {
            warn(format!("patch: invalid fields for {id}: {error}, skipping"));
            continue;
        }
        target.value = candidate;
        if patch.set.contains_key("config") {
            target.children = None;
        }
    }
    let mut result = roots
        .iter()
        .map(|row| {
            // Roots start as typed Entry; validated updates and snapshots only replace arbitrary config.
            serde_json::from_value(snapshot(row)).expect("patch engine preserves Entry field types")
        })
        .collect::<Vec<Entry>>();
    let mut pool = IdPool {
        reserved: reserved.clone(),
        seen: IndexSet::new(),
        next: 0,
    };
    pool.rows(&mut result);
    result
}
/// Apply all patches against one initial index; inserted descendants join that index.
pub fn apply_patches(
    base: &[Entry],
    patches: &[Patch],
    warn: &mut dyn FnMut(String),
) -> Vec<Entry> {
    let reserved = IdPool::new(base, patches).reserved;
    run(base, patches, warn, false, &reserved)
}

// One allocation domain for the base and every inserted subtree, including all later layers.
struct IdPool {
    reserved: IndexSet<String>,
    seen: IndexSet<String>,
    next: u32,
}
impl IdPool {
    fn new(base: &[Entry], patches: &[Patch]) -> Self {
        fn collect(value: &Value, ids: &mut IndexSet<String>) {
            match value {
                Value::Object(map) => {
                    if let Some(id) = map.get("id").and_then(Value::as_str) {
                        ids.insert(id.into());
                    }
                    for value in map.values() {
                        collect(value, ids);
                    }
                }
                Value::Array(rows) => {
                    for row in rows {
                        collect(row, ids);
                    }
                }
                _ => {}
            }
        }
        let mut reserved = IndexSet::new();
        for row in base {
            reserved.insert(row.id.clone());
            collect(&row.config, &mut reserved);
        }
        for patch in patches {
            if let Some(rows) = &patch.insert {
                for row in rows {
                    reserved.insert(row.id.clone());
                    collect(&row.config, &mut reserved);
                }
            }
            for value in patch.set.values() {
                collect(value, &mut reserved);
            }
        }
        Self {
            reserved,
            seen: IndexSet::new(),
            next: 0,
        }
    }
    fn id(&mut self, id: &mut String) {
        if !id.is_empty() && self.seen.insert(id.clone()) {
            return;
        }
        loop {
            self.next = self.next.wrapping_add(1);
            let fresh = format!("{:08x}", self.next);
            if self.reserved.insert(fresh.clone()) {
                self.seen.insert(fresh.clone());
                *id = fresh;
                return;
            }
        }
    }
    fn value(&mut self, value: &mut Value) {
        if let Some(rows) = value.as_array_mut() {
            for row in rows {
                if let Some(map) = row.as_object_mut() {
                    let mut id = map
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    self.id(&mut id);
                    map.insert("id".into(), Value::String(id));
                    if map.get("group") == Some(&Value::Bool(true))
                        && let Some(config) = map.get_mut("config")
                    {
                        self.value(config);
                    }
                }
            }
        }
    }
    fn rows(&mut self, rows: &mut [Entry]) {
        for row in rows {
            self.id(&mut row.id);
            if row.group {
                self.value(&mut row.config);
            }
        }
    }
}
fn label(layer: &Layer) -> String {
    match &layer.source {
        LayerSource::Bundle(name) => name.clone(),
        LayerSource::Profile => "profile".into(),
        LayerSource::Home => "home".into(),
        LayerSource::CommandLine(file) => file.to_string_lossy().into(),
        LayerSource::Launcher(name) => (*name).into(),
    }
}
fn flatten(rows: &[Entry]) -> IndexMap<String, Value> {
    fn walk(value: &Value, out: &mut IndexMap<String, Value>) {
        if let Some(rows) = value.as_array() {
            for row in rows {
                if let Some(id) = row.get("id").and_then(Value::as_str) {
                    out.insert(id.into(), row.clone());
                }
                if row.get("group") == Some(&json!(true)) {
                    walk(&row["config"], out);
                }
            }
        }
    }
    let mut out = IndexMap::new();
    walk(&entries_value(rows), &mut out);
    out
}
/// Prefix snapshots always apply ONE flattened list to the original base (§L9).
pub fn compose(base: &[Entry], layers: &[Layer], warn: &mut dyn FnMut(String)) -> Composition {
    let patches = layers
        .iter()
        .flat_map(|l| l.patches.iter().cloned())
        .collect::<Vec<_>>();
    let mut pool = IdPool::new(base, &patches);
    let reserved = pool.reserved.clone();
    let mut entries = base.to_vec();
    pool.rows(&mut entries);
    let mut previous = flatten(&entries);
    let mut origins: IndexMap<String, Origin> = previous
        .keys()
        .map(|id| {
            (
                id.clone(),
                Origin {
                    created_by: "base".into(),
                    patched_by: Vec::new(),
                },
            )
        })
        .collect();
    let mut count = 0;
    let mut warnings_seen = 0;
    for layer in layers {
        count += layer.patches.len();
        let label = label(layer);
        let mut warnings = Vec::new();
        entries = run(
            base,
            &patches[..count],
            &mut |w| warnings.push(w),
            true,
            &reserved,
        );
        for warning in &warnings[warnings_seen..] {
            warn(format!("[{label}] {warning}"));
        }
        warnings_seen = warnings.len();
        let current = flatten(&entries);
        for (id, value) in &current {
            match previous.get(id) {
                None => {
                    origins.insert(
                        id.clone(),
                        Origin {
                            created_by: label.clone(),
                            patched_by: Vec::new(),
                        },
                    );
                }
                Some(old) if old != value => {
                    if let Some(origin) = origins.get_mut(id) {
                        origin.patched_by.push(label.clone());
                    }
                }
                Some(_) => {}
            }
        }
        previous = current;
    }
    origins.retain(|id, _| previous.contains_key(id));
    Composition { entries, origins }
}
/// Write a loadable YAML document, retaining literal expression source.
pub fn render_dump(composition: &Composition) -> String {
    if composition.entries.is_empty() {
        return "[]\n".into();
    }
    let mut out = String::new();
    for entry in &composition.entries {
        let origin = composition.origins.get(&entry.id);
        out.push_str(&format!(
            "# == {} · {}",
            comment(&entry.id),
            comment(origin.map(|o| o.created_by.as_str()).unwrap_or("base"))
        ));
        if let Some(origin) = origin
            && !origin.patched_by.is_empty()
        {
            out.push_str(&format!(
                " · patched by {}",
                origin
                    .patched_by
                    .iter()
                    .map(|s| comment(s))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push('\n');
        out.push_str(&write_yaml(&entries_value(std::slice::from_ref(entry))));
    }
    out
}
fn comment(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}
