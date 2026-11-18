//! Include-local identities: a file's unchanged anonymous rows keep their first ids.
use crate::{Entry, Layer, Patch};
use indexmap::IndexSet;
use serde_json::Value;

#[derive(Clone)]
struct Anonymous {
    scope: &'static str,
    parent: Option<String>,
    raw: Entry,
    id: String,
}
#[derive(Default)]
pub(crate) struct Identities {
    next: u32,
    previous: Vec<Anonymous>,
}
impl Identities {
    pub(crate) fn assign(&mut self, base: &mut [Entry], layers: &mut [Layer]) {
        let mut patches: Vec<_> = layers
            .iter()
            .flat_map(|layer| layer.patches.iter().cloned())
            .collect();
        self.assign_patches(base, &mut patches);
        let mut patches = patches.into_iter();
        for layer in layers {
            for patch in &mut layer.patches {
                if let Some(next) = patches.next() {
                    *patch = next;
                }
            }
        }
    }
    pub(crate) fn assign_patches(&mut self, base: &mut [Entry], patches: &mut [Patch]) {
        self.assign_reserved(base, patches, &IndexSet::new());
    }
    pub(crate) fn assign_reserved(
        &mut self,
        base: &mut [Entry],
        patches: &mut [Patch],
        existing: &IndexSet<String>,
    ) {
        let mut explicit = IndexSet::new();
        fn reserve(value: &Value, used: &mut IndexSet<String>) {
            match value {
                Value::Object(map) => {
                    if let Some(id) = map.get("id").and_then(Value::as_str)
                        && !id.is_empty()
                    {
                        used.insert(id.into());
                    }
                    for value in map.values() {
                        reserve(value, used);
                    }
                }
                Value::Array(array) => {
                    for value in array {
                        reserve(value, used);
                    }
                }
                _ => {}
            }
        }
        for row in base.iter() {
            if !row.id.is_empty() {
                explicit.insert(row.id.clone());
            }
            reserve(&row.config, &mut explicit);
        }
        for patch in patches.iter() {
            for row in patch.insert.iter().flatten() {
                if !row.id.is_empty() {
                    explicit.insert(row.id.clone());
                }
                reserve(&row.config, &mut explicit);
            }
            for value in patch.set.values() {
                reserve(value, &mut explicit);
            }
        }
        let mut old = std::mem::take(&mut self.previous);
        let mut reserved = explicit.clone();
        reserved.extend(existing.iter().cloned());
        reserved.extend(old.iter().map(|entry| entry.id.clone()));
        self.rows(base, None, "base", &explicit, &mut reserved, &mut old);
        for patch in patches {
            if let Some(insert) = &mut patch.insert {
                self.rows(
                    insert,
                    patch.id.as_ref().and_then(Value::as_str).map(str::to_owned),
                    "insert",
                    &explicit,
                    &mut reserved,
                    &mut old,
                );
            }
        }
    }
    fn rows(
        &mut self,
        rows: &mut [Entry],
        parent: Option<String>,
        scope: &'static str,
        explicit: &IndexSet<String>,
        reserved: &mut IndexSet<String>,
        old: &mut Vec<Anonymous>,
    ) {
        for row in rows {
            let raw = row.clone();
            let child_parent = (!raw.id.is_empty()).then(|| raw.id.clone());
            if row.id.is_empty() {
                let matched = old.iter().position(|entry| {
                    entry.scope == scope
                        && entry.parent == parent
                        && entry.raw == raw
                        && !explicit.contains(&entry.id)
                });
                let id = if let Some(index) = matched {
                    old.remove(index).id
                } else {
                    loop {
                        self.next = self.next.wrapping_add(1);
                        let fresh = format!("{:08x}", self.next);
                        if reserved.insert(fresh.clone()) {
                            break fresh;
                        }
                    }
                };
                self.previous.push(Anonymous {
                    scope,
                    parent: parent.clone(),
                    raw,
                    id: id.clone(),
                });
                row.id = id;
            } else if let Some(index) = old
                .iter()
                .position(|entry| entry.scope == scope && entry.id == row.id)
            {
                self.previous.push(old.remove(index));
            }
            if row.group
                && let Some(array) = row.config.as_array_mut()
            {
                // Entries were validated at the YAML boundary; only the ids below change.
                if let Ok(mut children) =
                    serde_json::from_value::<Vec<Entry>>(Value::Array(array.clone()))
                {
                    self.rows(&mut children, child_parent, scope, explicit, reserved, old);
                    *array = children.iter().map(crate::yaml::entry_value).collect();
                }
            }
        }
    }
}
