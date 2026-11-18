//! Human-readable documents, insertion order, identity and stamp validation.
use hoh_storage::{KvSnapshot, KvUnitDescriptor, StorageError, StorageErrorCode};
use hoh_util_values::{js_json_parse, js_json_stringify, js_json_stringify_pretty, js_string};
use indexmap::IndexMap;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct UnitState {
    pub version: u64,
    pub global: Value,
    pub tables: IndexMap<String, IndexMap<String, Value>>,
}
impl UnitState {
    pub(crate) fn empty(descriptor: &KvUnitDescriptor) -> Self {
        Self {
            version: descriptor.version,
            global: Value::Null,
            tables: descriptor
                .tables
                .iter()
                .map(|table| (table.clone(), IndexMap::new()))
                .collect(),
        }
    }
    pub(crate) fn snapshot(&self) -> KvSnapshot {
        // loadAll returns ordinary JS objects, so canonical integer keys precede
        // other keys even when the authoritative Map insertion order differs.
        let tables = self
            .tables
            .iter()
            .map(|(table, records)| {
                let keys = Value::Object(
                    records
                        .keys()
                        .map(|key| (key.clone(), Value::Null))
                        .collect(),
                );
                // A map of null values is always valid JSON and keeps the source keys.
                let ordered =
                    js_json_parse(&js_json_stringify(&keys)).expect("stringified JSON is valid");
                let records = ordered
                    .as_object()
                    .expect("object remains object")
                    .keys()
                    .map(|key| (key.clone(), records[key].clone()))
                    .collect();
                (table.clone(), records)
            })
            .collect();
        KvSnapshot {
            tables,
            global: self.global.clone(),
        }
    }
}
pub fn serialize(name: &str, state: &UnitState) -> String {
    let document = json!({"unit": {"name": name, "version": state.version}, "global": state.global, "tables": state.tables});
    format!("{}\n", js_json_stringify_pretty(&document, 2))
}
pub fn parse(text: &str, descriptor: &KvUnitDescriptor) -> Result<UnitState, StorageError> {
    let malformed = |message: &str| {
        StorageError::new(
            StorageErrorCode::MalformedMedium,
            format!("unit '{}': {message}", descriptor.name),
        )
    };
    let document = js_json_parse(text)
        .map_err(|error| malformed("file is not valid JSON").with_cause(error))?;
    if !document.is_object() && !document.is_array() {
        return Err(malformed("file is not a JSON object"));
    }
    let header = document.get("unit");
    if header
        .and_then(|header| header.get("name"))
        .and_then(Value::as_str)
        != Some(descriptor.name.as_str())
        || !header
            .and_then(|header| header.get("version"))
            .is_some_and(Value::is_number)
    {
        return Err(malformed("missing or foreign unit header"));
    }
    // A successful lookup proves the number field exists.
    let stamped = &document["unit"]["version"];
    if !accepted(stamped, descriptor) {
        return Err(StorageError::new(
            StorageErrorCode::VersionMismatch,
            format!(
                "unit '{}': stored version {} is not accepted by version {}",
                descriptor.name,
                js_string(stamped),
                js_string(&Value::from(descriptor.version))
            ),
        ));
    }
    let tables = document
        .get("tables")
        .and_then(Value::as_object)
        .ok_or_else(|| malformed("tables is not an object"))?;
    let mut state = UnitState::empty(descriptor);
    // accepted() above only succeeds for an exact supported integer stamp.
    state.version = natural(stamped).expect("accepted stamp is an exact supported integer");
    state.global = document.get("global").cloned().unwrap_or(Value::Null);
    for table in &descriptor.tables {
        if let Some(records) = tables.get(table) {
            let records = records
                .as_object()
                .ok_or_else(|| malformed(&format!("table '{table}' is not an object")))?;
            state.tables.insert(
                table.clone(),
                records
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            );
        }
    }
    Ok(state)
}
pub fn serialize_record(version: u64, value: &Value) -> String {
    format!(
        "{}\n",
        js_json_stringify_pretty(&json!({"version":version,"record":value}), 2)
    )
}
pub fn parse_record(text: &str, versions: &[u64]) -> Option<Value> {
    let document = js_json_parse(text).ok()?;
    let version = natural(document.get("version")?)?;
    versions
        .contains(&version)
        .then(|| document.get("record").cloned())
        .flatten()
}
pub(crate) fn natural(value: &Value) -> Option<u64> {
    if let Some(value) = value.as_u64() {
        return (value <= 9_007_199_254_740_992).then_some(value);
    }
    let value = value.as_f64()?;
    (value.is_finite() && value.fract() == 0.0 && (0.0..=9_007_199_254_740_992.0).contains(&value))
        .then_some(value as u64)
}
pub(crate) fn accepted(stamped: &Value, descriptor: &KvUnitDescriptor) -> bool {
    natural(stamped).is_some_and(|version| {
        version == descriptor.version
            || descriptor
                .compatible_versions
                .as_ref()
                .is_some_and(|versions| versions.contains(&version))
    })
}
