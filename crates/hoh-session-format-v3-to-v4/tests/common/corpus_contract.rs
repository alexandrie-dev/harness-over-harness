#![allow(dead_code)]
use crate::common::*;
use hoh_session_format_v3_to_v4::{
    facts::historical_child_catalog_source,
    sources::{map_event_messages, rewrite_plugin_source},
    tool_role::lift_tool_result,
};
use std::path::{Path, PathBuf};
pub(crate) fn fixture_records(path: &Path) -> (Value, Vec<Value>) {
    let text = std::fs::read_to_string(path).unwrap();
    let mut records = hoh_util_values::js_trim(&text)
        .split('\n')
        .map(|line| hoh_util_values::js_json_parse(line).unwrap());
    let mut header = records.next().unwrap();
    if header
        .get("cwd")
        .and_then(Value::as_str)
        .is_some_and(|cwd| cwd.starts_with("{{cwd}}"))
    {
        header["cwd"] = "/snapshot-cwd".into();
    }
    let mut rows = vec![];
    for (seq, record) in records.enumerate() {
        let mut record = obj(record);
        if record.get("type").is_some_and(|t| t == "request/header")
            && let Some(request) = record
                .get_mut("data")
                .and_then(|d| d.get_mut("header"))
                .and_then(Value::as_object_mut)
        {
            if request.get("tools").is_some_and(|t| t == "{{tools}}") {
                request.shift_remove("tools");
            } else if let Some(tools) = request
                .get("tools")
                .and_then(Value::as_array)
                .filter(|a| a.iter().all(Value::is_string))
            {
                let tools = tools
                    .iter()
                    .map(|name| json!({"name":name,"description":"","parameters":{}}))
                    .collect::<Vec<_>>();
                request.insert("tools".into(), tools.into());
            }
        }
        if record.get("seq").is_none_or(Value::is_null) {
            record.insert("seq".into(), seq.into());
        }
        if record.get("time").is_none_or(Value::is_null) {
            record.insert("time".into(), 0.into());
        }
        rows.push(Value::Object(record));
    }
    (header, rows)
}
fn fixture(path: &Path) -> SessionFormatArtifact {
    let (header, rows) = fixture_records(path);
    let mut decoder = RELEASED_V3_SESSION_FORMAT_CODEC
        .create_decoder(&header, SessionFormatRecovery::Strict)
        .unwrap();
    let mut output = SessionFormatEventCollector::new();
    for row in rows {
        decoder.decode_row(row, &mut output).unwrap();
    }
    let cut = decoder.finish(&mut output).unwrap();
    let known = RELEASED_V3_EVENT_TYPES
        .iter()
        .map(|t| (*t).into())
        .collect();
    hoh_session_format_v2_to_v3::restore_released_v3_artifact(
        artifact(output.values, decoder.header().clone(), cut),
        &known,
    )
    .unwrap()
}
fn converted(
    parent: &SessionFormatArtifact,
    children: &[SessionFormatArtifact],
) -> R<SessionFormatArtifact> {
    let facts = children
        .iter()
        .map(|c| historical_child_catalog_source(c).map(Value::Object))
        .collect::<R<Vec<_>>>()?;
    let a = migrate(
        parent.events.clone(),
        parent.header.clone(),
        Some(parent.inherited_event_count),
        facts,
    )?;
    let known = RELEASED_V3_EVENT_TYPES
        .iter()
        .map(|t| (*t).into())
        .collect();
    restore_released_v4_artifact(a, &known)
}
fn expected_event(event: &SessionFormatEvent) -> SessionFormatEvent {
    let seq = event["seq"].as_u64().unwrap();
    let rewritten = map_event_messages(event.clone(), |mut message| {
        if let Some(source) = message
            .get("source")
            .and_then(Value::as_object)
            .filter(|s| s.get("kind").is_some_and(|k| k == "plugin"))
        {
            let source = rewrite_plugin_source(source.clone(), seq, message.get("role"))?;
            message.insert("source".into(), source.into());
        }
        Ok(message)
    })
    .unwrap();
    lift_tool_result(rewritten).unwrap()
}
fn sorted_dirs(path: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort_by(|a, b| {
        a.file_name()
            .unwrap()
            .as_encoded_bytes()
            .cmp(b.file_name().unwrap().as_encoded_bytes())
    });
    dirs
}
pub(crate) fn corpus_contract(
    fixture: impl Fn(&Path) -> SessionFormatArtifact,
    converted: impl Fn(&SessionFormatArtifact, &[SessionFormatArtifact]) -> R<SessionFormatArtifact>,
) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("hoh-session-format-v3-to-v4/tests/fixtures");
    let mut conflicts = vec![];
    let mut pairs = 0;
    let mut without_descriptor = 0;
    for profile in ["sdk", "session"] {
        let base = root.join("snapshots").join(profile);
        for dir in sorted_dirs(&base) {
            let mut paths: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| {
                    p.file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .ends_with(".v3.jsonl")
                })
                .collect();
            paths.sort_by(|a, b| {
                a.file_name()
                    .unwrap()
                    .as_encoded_bytes()
                    .cmp(b.file_name().unwrap().as_encoded_bytes())
            });
            if paths.len() < 2 {
                continue;
            }
            let logs: Vec<_> = paths
                .iter()
                .map(|p| {
                    (
                        p.strip_prefix(&root).unwrap().to_str().unwrap().to_owned(),
                        fixture(p),
                    )
                })
                .collect();
            for (parent_path, parent) in &logs {
                let children: Vec<_> = logs
                    .iter()
                    .filter(|(_, child)| {
                        child.header.get("origin").is_some_and(|o| o == "subagent")
                            && child.header.get("parentSession") == parent.header.get("id")
                    })
                    .collect();
                if children.is_empty() {
                    continue;
                }
                pairs += children.len();
                let catalogs: Vec<_> = parent
                    .events
                    .iter()
                    .filter(|e| {
                        e["type"] == "subagent/catalog"
                            && e["seq"].as_u64().unwrap() >= parent.inherited_event_count
                    })
                    .collect();
                let mut parent_has_conflict = false;
                let mut aligned = vec![];
                for (child_path, child) in &children {
                    let catalog = catalogs
                        .iter()
                        .find(|e| e["data"].get("childId") == child.header.get("id"))
                        .unwrap_or_else(|| panic!("missing catalog for {child_path}"));
                    let created_at = catalog["data"]["childCreatedAt"].clone();
                    if Some(&created_at) != child.header.get("createdAt") {
                        conflicts.push(child_path.clone());
                        parent_has_conflict = true;
                    }
                    if historical_child_catalog_source(child).unwrap()["descriptorCount"] == 0 {
                        without_descriptor += 1;
                    }
                    let mut copy = child.clone();
                    copy.header.insert("createdAt".into(), created_at);
                    aligned.push(copy);
                }
                let raw_children: Vec<_> =
                    children.iter().map(|(_, child)| child.clone()).collect();
                if parent_has_conflict {
                    err(
                        converted(parent, &raw_children),
                        "conflicts with its parent catalog",
                    );
                } else {
                    assert_eq!(
                        converted(parent, &raw_children).unwrap().events,
                        parent.events.iter().map(expected_event).collect::<Vec<_>>(),
                        "{parent_path}"
                    );
                }
                let mut expected = parent.clone();
                expected.header.insert("version".into(), 4.into());
                expected.events = parent.events.iter().map(expected_event).collect();
                assert_eq!(
                    converted(parent, &aligned).unwrap(),
                    expected,
                    "{parent_path}"
                );
            }
        }
    }
    assert_eq!(pairs, 25);
    assert_eq!(without_descriptor, 1);
    conflicts.sort();
    assert_eq!(conflicts, EXPECTED_CONFLICTS);
}
const EXPECTED_CONFLICTS: &[&str] = &[
    "snapshots/sdk/subagent-continuable-inheritance/session.1.v3.jsonl",
    "snapshots/sdk/subagent-continuable/session.1.v3.jsonl",
    "snapshots/sdk/subagent-fork-in-process/session.1.v3.jsonl",
    "snapshots/sdk/subagent-list-agents/session.1.v3.jsonl",
    "snapshots/sdk/subagent-mixed/session.1.v3.jsonl",
    "snapshots/sdk/subagent-mixed/session.2.v3.jsonl",
    "snapshots/sdk/subagent-send-message/session.1.v3.jsonl",
    "snapshots/sdk/subagent-spawn-in-process/session.1.v3.jsonl",
    "snapshots/session/advanced-toolchain-runtime/session.1.v3.jsonl",
    "snapshots/session/advanced-toolchain-runtime/session.2.v3.jsonl",
    "snapshots/session/advanced-toolchain/session.1.v3.jsonl",
    "snapshots/session/advanced-toolchain/session.2.v3.jsonl",
    "snapshots/session/ralph-loop/session.1.v3.jsonl",
    "snapshots/session/ralph-loop/session.2.v3.jsonl",
    "snapshots/session/subagent-child-question-rejection/session.1.v3.jsonl",
    "snapshots/session/subagent-depth-two-rejection/session.1.v3.jsonl",
    "snapshots/session/subagent-depth-two-rejection/session.2.v3.jsonl",
    "snapshots/session/subagent-max-tokens-partial/session.1.v3.jsonl",
    "snapshots/session/subagent-parallel/session.1.v3.jsonl",
    "snapshots/session/subagent-parallel/session.2.v3.jsonl",
    "snapshots/session/subagent-published-run-failure/session.1.v3.jsonl",
    "snapshots/session/subagent-spawn-in-process/session.1.v3.jsonl",
    "snapshots/session/subagent-tool-filter/session.1.v3.jsonl",
    "snapshots/session/workflow-run/session.1.v3.jsonl",
];

pub(crate) fn native_contract() {
    corpus_contract(fixture, converted);
}
