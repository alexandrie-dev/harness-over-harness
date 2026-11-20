//! `suite.ts` helpers that carry committed volatile values into freshly harvested logs
//! (`stabilizeFixtureMessageIds`, `refreshFixtureReplacements`, `stabilizeRefreshLog`) and the raw
//! catalog-clock check that a replayed run also performs (`reconcileCatalogCreationTimes`).
use crate::{
    Result,
    normalize::{Context, Normalizer, extract_snapshot_spill_paths},
    records,
    sidecars::fixture_context,
};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

const PACKED_CHUNK_ROW_TYPES: [&str; 3] = ["text-chunks", "reasoning-chunks", "tool-call-chunks"];
const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

fn stringify(value: &Value) -> String {
    hoh_util_values::js_json_stringify(value)
}

/// A literal replacement from a fresh replay-run volatile to its existing fixture value.
pub type Replacement = (String, String);

fn is_surface_type(kind: &str) -> bool {
    matches!(
        kind,
        "user/message"
            | "system/message"
            | "developer/message"
            | "assistant/message"
            | "tool/result"
    )
}

fn is_complete_message(value: &Value) -> bool {
    static UUID: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let uuid = UUID.get_or_init(|| {
        Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap()
    });
    value.is_object()
        && value["id"].as_str().is_some_and(|id| uuid.is_match(id))
        && value["role"].is_string()
        && value["content"].is_array()
        && value["source"].is_object()
}

/// The complete identified messages one durable record structurally owns.
fn messages_mut(record: &mut Value) -> Vec<&mut Value> {
    let kind = record["type"].as_str().unwrap_or_default().to_owned();
    if is_surface_type(&kind) {
        let slot = if kind == "user/message" {
            record.get_mut("data")
        } else {
            record.get_mut("data").and_then(|d| d.get_mut("message"))
        };
        return slot
            .filter(|m| is_complete_message(m))
            .into_iter()
            .collect();
    }
    if kind != "agent/inbox/spliced" {
        return Vec::new();
    }
    record
        .get_mut("data")
        .and_then(|d| d.get_mut("inserted"))
        .and_then(Value::as_array_mut)
        .map(|rows| rows.iter_mut().filter(|m| is_complete_message(m)).collect())
        .unwrap_or_default()
}

fn messages(record: &Value) -> Vec<&Value> {
    let kind = record["type"].as_str().unwrap_or_default();
    if is_surface_type(kind) {
        let slot = if kind == "user/message" {
            record.get("data")
        } else {
            record.get("data").and_then(|d| d.get("message"))
        };
        return slot
            .filter(|m| is_complete_message(m))
            .into_iter()
            .collect();
    }
    if kind != "agent/inbox/spliced" {
        return Vec::new();
    }
    record["data"]["inserted"]
        .as_array()
        .map(|rows| rows.iter().filter(|m| is_complete_message(m)).collect())
        .unwrap_or_default()
}

/// Serialize parsed JSON by value rather than insertion order.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|k| format!(
                        "{}:{}",
                        stringify(&Value::String((*k).clone())),
                        canonical_json(&map[*k])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        other => stringify(other),
    }
}

/// Identity-free message values whose id and fingerprint are mutually unique.
fn unique_message_ids(logs: &[String]) -> Result<HashMap<String, String>> {
    let mut fingerprints_by_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut ids_by_fingerprint: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for log in logs {
        for record in records(log)? {
            for message in messages(&record) {
                let mut without_id = message.clone();
                let id = without_id
                    .as_object_mut()
                    .and_then(|m| m.remove("id"))
                    .and_then(|id| id.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let fingerprint = canonical_json(&without_id);
                fingerprints_by_id
                    .entry(id.clone())
                    .or_default()
                    .insert(fingerprint.clone());
                ids_by_fingerprint
                    .entry(fingerprint)
                    .or_default()
                    .insert(id);
            }
        }
    }
    let mut unique = HashMap::new();
    for (id, fingerprints) in fingerprints_by_id {
        if fingerprints.len() != 1 {
            continue;
        }
        let fingerprint = fingerprints.into_iter().next().unwrap_or_default();
        if ids_by_fingerprint.get(&fingerprint).map(BTreeSet::len) != Some(1) {
            continue;
        }
        unique.insert(fingerprint, id);
    }
    Ok(unique)
}

/// Rewrite only validated durable-message id fields, leaving every other occurrence untouched.
fn apply_fixture_message_ids(
    content: &str,
    replacements: &HashMap<String, String>,
) -> Result<String> {
    let mut out = Vec::new();
    for line in content.split('\n') {
        if hoh_util_values::js_trim(line).is_empty() {
            out.push(line.to_owned());
            continue;
        }
        let mut record = hoh_util_values::js_json_parse(line).map_err(|e| e.to_string())?;
        let mut changed = false;
        for message in messages_mut(&mut record) {
            let Some(replacement) = message["id"].as_str().and_then(|id| replacements.get(id))
            else {
                continue;
            };
            message["id"] = Value::String(replacement.clone());
            changed = true;
        }
        out.push(if changed {
            stringify(&record)
        } else {
            line.to_owned()
        });
    }
    Ok(out.join("\n"))
}

/// Carry committed UUIDs into unchanged, unambiguous messages in fresh session fixtures.
pub fn stabilize_fixture_message_ids(logs: &[String], fixtures: &[String]) -> Result<Vec<String>> {
    let fresh = unique_message_ids(logs)?;
    let existing = unique_message_ids(fixtures)?;
    let mut replacements = HashMap::new();
    for (fingerprint, fresh_id) in &fresh {
        let Some(existing_id) = existing.get(fingerprint) else {
            continue;
        };
        if fresh_id != existing_id {
            replacements.insert(fresh_id.clone(), existing_id.clone());
        }
    }
    logs.iter()
        .map(|log| apply_fixture_message_ids(log, &replacements))
        .collect()
}

/// Refresh write-back replacements for per-log session ids, cwd values and spill paths.
pub fn refresh_fixture_replacements(
    logs: &[String],
    fixtures: &[String],
) -> Result<Vec<Replacement>> {
    let mut replacements = Vec::new();
    for (i, log) in logs.iter().enumerate() {
        let existing = fixtures.get(i).map_or("", String::as_str);
        let fresh_header = records(log)?.into_iter().next();
        let existing_header = records(existing)?.into_iter().next();
        for field in ["id", "cwd"] {
            let from = fresh_header.as_ref().and_then(|h| h[field].as_str());
            let to = existing_header.as_ref().and_then(|h| h[field].as_str());
            if let (Some(from), Some(to)) = (from, to)
                && !from.is_empty()
                && from != to
            {
                replacements.push((from.to_owned(), to.to_owned()));
            }
        }
        // Match moved snapshot spill paths by filename so the fixture does not churn on every refresh.
        let fresh_spills = extract_snapshot_spill_paths(log)?;
        for (name, existing_path) in extract_snapshot_spill_paths(existing)? {
            if let Some(fresh_path) = fresh_spills.get(&name)
                && *fresh_path != existing_path
            {
                replacements.push((fresh_path.clone(), existing_path));
            }
        }
    }
    Ok(replacements)
}

/// How `reconcile_catalog_creation_times` treats a disagreeing child clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClockPolicy {
    /// Fail on disagreement (live output).
    Validate,
    /// Align the catalog to the retained child headers (refresh).
    PreserveHeaders,
}

fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => stringify(other),
    }
}

/// Check raw parent/child clocks before normalization, or preserve their equality during refresh.
pub fn reconcile_catalog_creation_times(
    logs: &[String],
    policy: ClockPolicy,
) -> Result<Vec<String>> {
    let mut headers: HashMap<String, Value> = HashMap::new();
    for log in logs {
        if let Some(header) = records(log)?.into_iter().next()
            && let Some(id) = header["id"].as_str()
        {
            headers.insert(id.to_owned(), header);
        }
    }
    let mut out = Vec::new();
    for log in logs {
        let mut lines = Vec::new();
        for line in log.split('\n') {
            if line.is_empty() {
                lines.push(String::new());
                continue;
            }
            let mut record = hoh_util_values::js_json_parse(line).map_err(|e| e.to_string())?;
            let child = record["data"]["childId"]
                .as_str()
                .and_then(|id| headers.get(id));
            if record["type"] != "subagent/catalog" || !record["data"].is_object() {
                lines.push(line.to_owned());
                continue;
            }
            // Partial harvested corpora need not retain every cataloged child log.
            let Some(child) = child else {
                lines.push(line.to_owned());
                continue;
            };
            let declared = record["data"].get("childCreatedAt");
            if declared == child.get("createdAt") {
                lines.push(line.to_owned());
                continue;
            }
            if policy == ClockPolicy::Validate {
                return Err(format!(
                    "catalog child {} creation time {} disagrees with child header {}",
                    js_string(&record["data"]["childId"]),
                    declared.map_or("undefined".into(), js_string),
                    child.get("createdAt").map_or("undefined".into(), js_string)
                ));
            }
            match child.get("createdAt") {
                Some(created) => record["data"]["childCreatedAt"] = created.clone(),
                None => {
                    record["data"]
                        .as_object_mut()
                        .map(|m| m.remove("childCreatedAt"));
                }
            }
            lines.push(stringify(&record));
        }
        out.push(lines.join("\n"));
    }
    Ok(out)
}

fn packed_times(record: &Value) -> Option<Vec<f64>> {
    if !PACKED_CHUNK_ROW_TYPES.contains(&record["type"].as_str().unwrap_or_default()) {
        return None;
    }
    let mut times = vec![record["time0"].as_f64().unwrap_or(0.0)];
    for gap in record["data"]["dt"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
    {
        let last = *times.last().unwrap_or(&0.0);
        times.push(last + gap.as_f64().unwrap_or(0.0));
    }
    Some(times)
}

/// Expand packed timing envelopes so refresh alignment follows logical events, not physical lines.
fn logical_records(records: Vec<Value>) -> Vec<Value> {
    records
        .into_iter()
        .flat_map(|record| match packed_times(&record) {
            None => vec![record],
            Some(times) => times
                .into_iter()
                .map(|t| serde_json::json!({"type": "assistant/chunk", "time": number(t)}))
                .collect(),
        })
        .collect()
}

fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() <= MAX_SAFE {
        Value::from(value as i64)
    } else {
        serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
    }
}

fn safe_integer(value: Option<&Value>) -> Option<f64> {
    let n = value?.as_f64()?;
    (n.fract() == 0.0 && n.abs() <= MAX_SAFE).then_some(n)
}

/// Carry logical member times into a fresh packed row while leaving its fragment arrays untouched.
fn preserve_packed_member_times(record: &mut Value, existing_members: &[Value]) {
    if !PACKED_CHUNK_ROW_TYPES.contains(&record["type"].as_str().unwrap_or_default()) {
        return;
    }
    let Some(first) = safe_integer(existing_members.first().and_then(|m| m.get("time"))) else {
        return;
    };
    record["time0"] = number(first);
    let gaps_len = record["data"]["dt"].as_array().map_or(0, Vec::len);
    if existing_members.len() != gaps_len + 1 {
        return;
    }
    let times: Option<Vec<f64>> = existing_members
        .iter()
        .map(|m| safe_integer(m.get("time")))
        .collect();
    let Some(times) = times else {
        return;
    };
    let gaps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    if gaps.iter().any(|g| g.fract() != 0.0 || g.abs() > MAX_SAFE) {
        return;
    }
    record["data"]["dt"] = Value::Array(gaps.into_iter().map(number).collect());
}

fn preserve_fixture_volatiles(record: &mut Value, existing: Option<&Value>) {
    let Some(existing) = existing else {
        return;
    };
    if existing["type"] != record["type"] {
        return;
    }
    if record["type"] == "session" {
        for field in ["id", "createdAt", "cwd", "parentSession"] {
            if record.get(field).is_some()
                && let Some(value) = existing.get(field)
            {
                record[field] = value.clone();
            }
        }
        return;
    }
    if record.get("time").is_some()
        && let Some(time) = existing.get("time")
    {
        record["time"] = time.clone();
    }
    if record["type"] != "hook/result" {
        return;
    }
    if record["data"].is_object()
        && existing["data"].is_object()
        && record["data"].get("durationMs").is_some()
        && let Some(duration) = existing["data"].get("durationMs")
    {
        record["data"]["durationMs"] = duration.clone();
    }
}

/// `Object.is` for the leaves JSON can hold.
fn same_leaf(a: &Value, b: &Value) -> bool {
    !matches!(a, Value::Array(_) | Value::Object(_))
        && !matches!(b, Value::Array(_) | Value::Object(_))
        && a == b
}

type Mappings = HashMap<(String, String), String>;

/// Reuse existing leaves whose normalized values equal the fresh values.
fn preserve_normalized_volatiles(
    fresh: &Value,
    existing: &Value,
    normalized_fresh: &Value,
    normalized_existing: &Value,
    mappings: &Mappings,
) -> Value {
    if let (Value::Array(f), Value::Array(e), Value::Array(nf), Value::Array(ne)) =
        (fresh, existing, normalized_fresh, normalized_existing)
    {
        if f.len() != e.len() || f.len() != nf.len() || f.len() != ne.len() {
            return fresh.clone();
        }
        return Value::Array(
            (0..f.len())
                .map(|i| preserve_normalized_volatiles(&f[i], &e[i], &nf[i], &ne[i], mappings))
                .collect(),
        );
    }
    if let (Value::Object(f), Value::Object(e), Value::Object(nf), Value::Object(ne)) =
        (fresh, existing, normalized_fresh, normalized_existing)
    {
        let mut out = Map::new();
        for (key, value) in f {
            let merged = match (e.get(key), nf.get(key), ne.get(key)) {
                (Some(e), Some(nf), Some(ne)) => {
                    preserve_normalized_volatiles(value, e, nf, ne, mappings)
                }
                _ => value.clone(),
            };
            out.insert(key.clone(), merged);
        }
        return Value::Object(out);
    }
    if let (Value::String(f), Value::String(e), Value::String(nf), Value::String(ne)) =
        (fresh, existing, normalized_fresh, normalized_existing)
        && nf == ne
    {
        return if mappings.get(&(nf.clone(), f.clone())).map(String::as_str) == Some(e.as_str()) {
            existing.clone()
        } else {
            fresh.clone()
        };
    }
    if same_leaf(normalized_fresh, normalized_existing) {
        existing.clone()
    } else {
        fresh.clone()
    }
}

/// Add normalized-equivalent string replacements to a bijection; `false` on a conflict.
fn collect_mappings(
    fresh: &Value,
    existing: &Value,
    normalized_fresh: &Value,
    normalized_existing: &Value,
    excluded: &HashSet<String>,
    forward: &mut Mappings,
    reverse: &mut Mappings,
) -> bool {
    if let (Value::Array(f), Value::Array(e), Value::Array(nf), Value::Array(ne)) =
        (fresh, existing, normalized_fresh, normalized_existing)
    {
        if f.len() != e.len() || f.len() != nf.len() || f.len() != ne.len() {
            return true;
        }
        return (0..f.len())
            .all(|i| collect_mappings(&f[i], &e[i], &nf[i], &ne[i], excluded, forward, reverse));
    }
    if let (Value::Object(f), Value::Object(e), Value::Object(nf), Value::Object(ne)) =
        (fresh, existing, normalized_fresh, normalized_existing)
    {
        return f.iter().all(
            |(key, value)| match (e.get(key), nf.get(key), ne.get(key)) {
                (Some(e), Some(nf), Some(ne)) => {
                    collect_mappings(value, e, nf, ne, excluded, forward, reverse)
                }
                _ => true,
            },
        );
    }
    let (Value::String(f), Value::String(e), Value::String(nf), Value::String(ne)) =
        (fresh, existing, normalized_fresh, normalized_existing)
    else {
        return true;
    };
    if nf != ne || f == e || excluded.contains(f) || excluded.contains(e) {
        return true;
    }
    let fresh_key = (nf.clone(), f.clone());
    let existing_key = (nf.clone(), e.clone());
    let mapped_existing = forward.get(&fresh_key);
    let mapped_fresh = reverse.get(&existing_key);
    if mapped_existing.is_some_and(|m| m != e) || mapped_fresh.is_some_and(|m| m != f) {
        return false;
    }
    forward.insert(fresh_key, e.clone());
    reverse.insert(existing_key, f.clone());
    true
}

/// A log-wide bijection for normalized-equivalent strings; any unexplained mismatch disables reuse.
fn normalized_string_mappings(
    records: &[Value],
    fresh_records: &[Value],
    existing_records: &[Value],
    normalized_fresh: &[Value],
    normalized_existing: &[Value],
) -> Option<Mappings> {
    let mut excluded = HashSet::new();
    for record in fresh_records.iter().chain(existing_records) {
        for message in messages(record) {
            if let Some(id) = message["id"].as_str() {
                excluded.insert(id.to_owned());
            }
        }
    }
    let mut forward = Mappings::new();
    let mut reverse = Mappings::new();
    let mut existing_index = 0;
    for (index, record) in records.iter().enumerate() {
        let existing = existing_records.get(existing_index);
        let member_count = packed_times(record).map_or(1, |t| t.len());
        if record["type"] == "session/title"
            && existing.is_none_or(|e| e["type"] != "session/title")
        {
            continue;
        }
        if member_count > 1 {
            let end = (existing_index + member_count).min(existing_records.len());
            let members = &existing_records[existing_index.min(end)..end];
            if members.len() != member_count
                || members.iter().any(|m| m["type"] != "assistant/chunk")
            {
                return None;
            }
        } else {
            let existing = existing?;
            if existing["type"] != record["type"] {
                return None;
            }
            if !collect_mappings(
                record,
                existing,
                normalized_fresh.get(index)?,
                normalized_existing.get(existing_index)?,
                &excluded,
                &mut forward,
                &mut reverse,
            ) {
                return None;
            }
        }
        existing_index += member_count;
    }
    (existing_index == existing_records.len()).then_some(forward)
}

fn normalized_refresh_records(records: &[Value], context: &Context) -> Result<Vec<Value>> {
    let text = records.iter().map(stringify).collect::<Vec<_>>().join("\n") + "\n";
    Normalizer::new()?.legacy_log(&text, context)
}

/// Rewrite a fresh replay-produced log so repeated refreshes do not churn volatile fixture fields.
/// Meaningful payloads come from `fresh`; the existing fixture lends normalized-equivalent values.
pub fn stabilize_refresh_log(
    fresh: &str,
    existing: &str,
    replacements: &[Replacement],
    fresh_context: &Context,
) -> Result<String> {
    let fresh_records = records(fresh)?;
    let mut stable = fresh.to_owned();
    for (from, to) in replacements {
        stable = stable.split(from.as_str()).collect::<Vec<_>>().join(to);
    }
    let existing_records = logical_records(records(existing)?);
    let mut records = records(&stable)?;
    let existing_context = fixture_context(existing)?;
    let normalized_fresh = normalized_refresh_records(&fresh_records, fresh_context)?;
    let normalized_existing = normalized_refresh_records(&existing_records, &existing_context)?;
    let mappings = normalized_string_mappings(
        &records,
        &fresh_records,
        &existing_records,
        &normalized_fresh,
        &normalized_existing,
    );
    let mut existing_index = 0;
    let mut previous_time: Option<Value> = None;
    for i in 0..records.len() {
        let existing_record = existing_records.get(existing_index);
        let member_count = packed_times(&records[i]).map_or(1, |t| t.len());
        let inserted_title = records[i]["type"] == "session/title"
            && existing_record.is_none_or(|e| e["type"] != "session/title");
        if inserted_title {
            let Some(time) = previous_time.clone().filter(Value::is_number) else {
                return Err("acp-snapshot: inserted title has no preceding event time".into());
            };
            records[i]["time"] = time;
        } else {
            if let Some(mappings) = &mappings
                && member_count == 1
                && let Some(existing_record) = existing_record
                && existing_record["type"] == records[i]["type"]
            {
                records[i] = preserve_normalized_volatiles(
                    &records[i],
                    existing_record,
                    &normalized_fresh[i],
                    &normalized_existing[existing_index],
                    mappings,
                );
            }
            let end = (existing_index + member_count).min(existing_records.len());
            let members = &existing_records[existing_index.min(end)..end];
            preserve_packed_member_times(&mut records[i], members);
            preserve_fixture_volatiles(&mut records[i], existing_records.get(existing_index));
            existing_index += member_count;
        }
        if records[i]["time"].is_number() {
            previous_time = Some(records[i]["time"].clone());
        }
    }
    Ok(records.iter().map(stringify).collect::<Vec<_>>().join("\n") + "\n")
}
