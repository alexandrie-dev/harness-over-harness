use crate::yaml::{entries_value, entry_value, write_yaml};
use crate::{Composition, Entry, Layer, LayerSource, Origin, Patch};
use indexmap::IndexMap;
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
fn run(base: &[Entry], patches: &[Patch], warn: &mut dyn FnMut(String)) -> Vec<Entry> {
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
                    warn(format!("patch insert: entry {id} not found"));
                    continue;
                };
                if target.borrow().value.get("group") != Some(&json!(true)) {
                    warn(format!("patch insert: entry {id} is not a group"));
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
            warn(format!("patch: entry {id} not found"));
            continue;
        };
        let mut target = target.borrow_mut();
        let name = target
            .value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(assertion) = &patch.name
            && assertion != name
        {
            warn(format!(
                "patch: name mismatch for {id} (expected {name}, got {assertion}), skipping"
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
    roots
        .iter()
        .map(|row| {
            // Roots start as typed Entry; validated updates and snapshots only replace arbitrary config.
            serde_json::from_value(snapshot(row)).expect("patch engine preserves Entry field types")
        })
        .collect()
}
/// Apply all patches against one initial index; inserted descendants join that index.
pub fn apply_patches(
    base: &[Entry],
    patches: &[Patch],
    warn: &mut dyn FnMut(String),
) -> Vec<Entry> {
    run(base, patches, warn)
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
    let mut entries = base.to_vec();
    let mut previous = flatten(base);
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
    let mut patches = Vec::new();
    let mut warnings_seen = 0;
    for layer in layers {
        patches.extend(layer.patches.iter().cloned());
        let label = label(layer);
        let mut warnings = Vec::new();
        entries = run(base, &patches, &mut |w| warnings.push(w));
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
