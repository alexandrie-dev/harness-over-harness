use crate::dispositions::RELEASED_V0_EVENT_TYPES;
use crate::validation::is_surface;
use crate::validation_helpers::{event_seq, event_type, field, has, js_stringify, record};
use hoh_session_format::{SessionFormatArtifact, SessionFormatError, SessionFormatEvent};
use hoh_util_values::deep_equal_json;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Clone)]
pub struct ReleasedRelationshipExtensions {
    pub step_events: HashSet<String>,
    pub preserved_source_title_request_text: bool,
    pub legacy_interrupted_turn_restart: bool,
}
struct CompactionState {
    id: Value,
    source_command_id: Option<Value>,
    turn: Value,
    start_seq: u64,
    summarized: bool,
}
struct PtcStart {
    root: Value,
    parent: Value,
    name: Value,
    arguments: Value,
    settled: bool,
}
struct ToolLifecycle {
    name: Value,
    arguments: Value,
    started: bool,
}
type Result = std::result::Result<(), SessionFormatError>;
fn err(message: impl Into<String>) -> SessionFormatError {
    SessionFormatError::new(message)
}
fn n(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|v| v as u64))
        .unwrap_or(0)
}
fn s(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
fn equal_count(value: &Value, count: Option<u64>) -> bool {
    match count {
        Some(n) => crate::validation::number_eq(value, n),
        None => value.is_null(),
    }
}

pub fn assert_released_artifact_relationships(
    artifact: &SessionFormatArtifact,
    extensions: &ReleasedRelationshipExtensions,
) -> Result {
    let mut open_turn = None;
    let mut open_step = None;
    let mut provider: Option<Value> = None;
    let mut next_turn = 1;
    let mut next_step = 1;
    let mut surface = Vec::new();
    let mut compaction: Option<CompactionState> = None;
    let stale = inherited_orphan_compaction_starts(&artifact.events);
    let mut retries: Vec<&SessionFormatEvent> = Vec::new();
    let mut retry_starts = HashSet::new();
    let mut ptc_roots = HashMap::new();
    let mut ptc_starts: HashMap<String, PtcStart> = HashMap::new();
    let mut tools: IndexMap<String, ToolLifecycle> = IndexMap::new();
    let mut command_runs = HashSet::new();
    for event in &artifact.events {
        let kind = event_type(event);
        let seq = event_seq(event);
        let extension = extensions.step_events.contains(kind);
        if !RELEASED_V0_EVENT_TYPES.contains(&kind) && !extension {
            continue;
        }
        let d = record(field(event, "data"), &format!("{kind} {seq} data"))?;
        if is_surface(kind) {
            surface = apply_surface(&surface, event)?;
        }
        if matches!(kind, "turn/start" | "turn/end")
            && compaction
                .as_ref()
                .is_some_and(|c| !stale.contains(&c.start_seq))
        {
            return Err(err(format!("{kind} crosses an open compaction")));
        }
        if extension {
            require_open_step(event, d, open_turn, open_step)?;
            continue;
        }
        match kind {
            "turn/start" => {
                if extensions.legacy_interrupted_turn_restart
                    && open_turn.is_some()
                    && open_step.is_none()
                    && open_turn.is_some_and(|t| {
                        crate::validation::number_eq(&d["turn"], t + 1) && next_turn == t
                    })
                    && let Some(previous) = seq
                        .checked_sub(1)
                        .and_then(|i| artifact.events.get(i as usize))
                        .filter(|e| event_type(e) == "agent/inbox/spliced")
                {
                    let splice = record(
                        field(previous, "data"),
                        &format!("agent/inbox/spliced {} data", event_seq(previous)),
                    )?;
                    if splice["target"] == "next-turn"
                        && splice["inserted"].as_array().is_some_and(|a| !a.is_empty())
                    {
                        open_turn = None;
                        next_turn += 1;
                    }
                }
                if open_turn.is_some() || !crate::validation::number_eq(&d["turn"], next_turn) {
                    return Err(err(format!(
                        "turn/start {} does not open expected turn {next_turn}",
                        js_stringify(&d["turn"])
                    )));
                }
                open_turn = Some(n(&d["turn"]));
                open_step = None;
                tools.clear();
                next_step = 1;
            }
            "turn/end" => {
                if !equal_count(&d["turn"], open_turn) {
                    return Err(err(format!(
                        "turn/end {} has no matching open turn",
                        js_stringify(&d["turn"])
                    )));
                }
                no_unresolved(&tools, "turn/end")?;
                if open_step.is_some() {
                    return Err(err(format!(
                        "turn/end {} crosses an open step",
                        js_stringify(&d["turn"])
                    )));
                }
                open_turn = None;
                next_turn += 1;
            }
            "step/start" => {
                if !equal_count(&d["turn"], open_turn)
                    || open_step.is_some()
                    || !crate::validation::number_eq(&d["step"], next_step)
                {
                    return Err(err(format!(
                        "{kind} does not match the open turn and next step"
                    )));
                }
                open_step = Some(n(&d["step"]));
            }
            "step/end" => {
                require_open_step(event, d, open_turn, open_step)?;
                no_unresolved(&tools, "step/end")?;
                tools.clear();
                open_step = None;
                next_step += 1;
            }
            "assistant/chunk" => require_open_step(event, d, open_turn, open_step)?,
            "assistant/message" => {
                require_open_step(event, d, open_turn, open_step)?;
                let message = record(&d["message"], &format!("assistant/message {seq} message"))?;
                if let Some(content) = message["content"].as_array() {
                    for block in content {
                        if block["type"] != "tool-call" {
                            continue;
                        }
                        let id = s(&block["id"]);
                        if tools.contains_key(id) {
                            return Err(err(format!(
                                "assistant/message repeats advertised tool call {id}"
                            )));
                        }
                        tools.insert(
                            id.to_owned(),
                            ToolLifecycle {
                                name: block["name"].clone(),
                                arguments: block["arguments"].clone(),
                                started: false,
                            },
                        );
                    }
                }
            }
            "tool/call" => {
                require_open_step(event, d, open_turn, open_step)?;
                let id = s(&d["callId"]);
                let Some(lifecycle) = tools
                    .get_mut(id)
                    .filter(|t| !t.started && t.name == d["name"] && t.arguments == d["arguments"])
                else {
                    return Err(err(format!(
                        "tool/call {id} does not match one advertised tool call"
                    )));
                };
                lifecycle.started = true;
            }
            "tool/result" => {
                if field(event, "surfaceOp") == "append" {
                    require_open_step(event, d, open_turn, open_step)?;
                    let m = record(&d["message"], &format!("tool/result {seq} message"))?;
                    let source = record(&m["source"], &format!("tool/result {seq} source"))?;
                    let id = s(&source["callId"]);
                    if has(d, "error") {
                        record(&d["error"], &format!("tool/result {seq} error"))?;
                    }
                    let Some(lifecycle) = tools.get(id) else {
                        return Err(err(format!(
                            "tool/result {id} has no advertised tool lifecycle"
                        )));
                    };
                    if !lifecycle.started && !is_exact_tool_not_started_repair(event, d, m, id) {
                        return Err(err(format!(
                            "tool/result {id} is not the exact TOOL_NOT_STARTED repair"
                        )));
                    }
                    tools.shift_remove(id);
                } else if open_turn.is_none() {
                    return Err(err("tool/result replacement is outside an open turn"));
                }
            }
            "request/header" => {
                if open_turn.is_none() {
                    return Err(err(format!("{kind} is outside an open turn")));
                }
                provider = d["header"]["config"].get("provider").cloned();
            }
            "request/context" => {
                if open_turn.is_none() {
                    return Err(err(format!("{kind} is outside an open turn")));
                }
            }
            "tool/code-dispatch-start" | "tool/code-dispatch" => {
                if open_turn.is_none() {
                    return Err(err(format!("{kind} is outside an open turn")));
                }
                let root = &d["rootCallId"];
                let parent = &d["parentCallId"];
                let child = s(&d["subCallId"]);
                if ptc_roots.get(child).is_some_and(|known| known != root) {
                    return Err(err(format!("{kind} changes its rootCallId")));
                }
                if parent != root && ptc_roots.get(s(parent)) != Some(root) {
                    return Err(err(format!(
                        "{kind} parentCallId does not belong to rootCallId"
                    )));
                }
                if kind == "tool/code-dispatch-start" {
                    if ptc_starts.contains_key(child) {
                        return Err(err("tool/code-dispatch-start repeats subCallId"));
                    }
                    ptc_starts.insert(
                        child.to_owned(),
                        PtcStart {
                            root: root.clone(),
                            parent: parent.clone(),
                            name: d["name"].clone(),
                            arguments: d["arguments"].clone(),
                            settled: false,
                        },
                    );
                } else {
                    let Some(start) = ptc_starts.get_mut(child).filter(|start| !start.settled)
                    else {
                        return Err(err("tool/code-dispatch has no unique start"));
                    };
                    if start.root != *root
                        || start.parent != *parent
                        || start.name != d["name"]
                        || !deep_equal_json(&start.arguments, &d["arguments"])
                    {
                        return Err(err("tool/code-dispatch does not match its start"));
                    }
                    start.settled = true;
                }
                ptc_roots.insert(child.to_owned(), root.clone());
            }
            "llm/retry" => {
                if !equal_count(&d["turn"], open_turn)
                    || !crate::validation::number_eq(&d["step"], open_step.unwrap_or(next_step - 1))
                    || open_turn.is_none()
                {
                    return Err(err("llm/retry does not match the current turn and step"));
                }
                if d.get("provider") != provider.as_ref() {
                    return Err(err(
                        "llm/retry provider does not match the open request/header",
                    ));
                }
                assert_retry_chain(&retries, d)?;
                retries.push(event);
            }
            "llm/retry-started" => {
                let scheduled = retries.iter().find(|e| {
                    let p = field(e, "data");
                    p["retryId"] == d["retryId"] && deep_equal_json(&p["retry"], &d["retry"])
                });
                let Some(scheduled) = scheduled else {
                    return Err(err("llm/retry-started pairs no prior scheduled attempt"));
                };
                let prior = field(scheduled, "data");
                if !deep_equal_json(&prior["turn"], &d["turn"])
                    || !deep_equal_json(&prior["step"], &d["step"])
                {
                    return Err(err(
                        "llm/retry-started does not match its scheduled turn and step",
                    ));
                }
                let key = format!(
                    "{}\0{}",
                    js_stringify(&d["retryId"]),
                    js_stringify(&d["retry"])
                );
                if !retry_starts.insert(key) {
                    return Err(err("llm/retry-started repeats one scheduled attempt"));
                }
            }
            "session/title" | "session/title-llm-request" => assert_title_sources(
                &artifact.events,
                event,
                d,
                !extensions.preserved_source_title_request_text,
            )?,
            "command/run" => {
                let id = s(&d["commandId"]);
                if !command_runs.insert(id.to_owned()) {
                    return Err(err(format!("command/run repeats commandId {id}")));
                }
            }
            "command/done" => {
                let id = s(&d["commandId"]);
                if !command_runs.contains(id) {
                    return Err(err(format!("command/done {id} has no prior command/run")));
                }
                if has(d, "sourceEventSeq") {
                    let source = artifact.events.get(n(&d["sourceEventSeq"]) as usize);
                    if d["kind"] != "success"
                        || source.is_some_and(|e| {
                            matches!(event_type(e), "command/run" | "command/done")
                        })
                    {
                        return Err(err(format!("command/done {id} has invalid sourceEventSeq")));
                    }
                }
            }
            "session-log-deepseek/delivery-accepted" => {
                let accepted = d
                    .get("sessionFormatVersion")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or(0.into());
                if deep_equal_json(&accepted, field(&artifact.header, "version")) {
                    let inherited = artifact.header.contains_key("parentSession")
                        && seq < artifact.inherited_event_count;
                    if !inherited && d["sessionId"] != *field(&artifact.header, "id") {
                        return Err(err(
                            "current-generation delivery marker names the wrong Session",
                        ));
                    }
                }
            }
            "compaction/start" => {
                if compaction.is_some() {
                    return Err(err("compaction/start overlaps an open compaction"));
                }
                compaction_turn(&d["turn"], open_turn, "compaction/start")?;
                compaction = Some(CompactionState {
                    id: d["compactionId"].clone(),
                    source_command_id: d.get("sourceCommandId").cloned(),
                    turn: d["turn"].clone(),
                    start_seq: seq,
                    summarized: false,
                });
            }
            "compaction/summary" => {
                compaction_owner(compaction.as_ref(), d, "compaction/summary")?;
                if let Some(c) = compaction.as_mut() {
                    compaction_turn(&c.turn, open_turn, "compaction/summary")?;
                    if c.summarized {
                        return Err(err("compaction/summary repeats"));
                    }
                    current_surface_span(&surface, d, "compaction/summary")?;
                    c.summarized = true;
                }
            }
            "compaction/end" => {
                compaction_owner(compaction.as_ref(), d, "compaction/end")?;
                if let Some(c) = compaction.as_ref() {
                    if !deep_equal_json(&d["turn"], &c.turn) {
                        return Err(err("compaction/end changes its owner turn"));
                    }
                    compaction_turn(&c.turn, open_turn, "compaction/end")?;
                    if !has(d, "error") && !c.summarized {
                        return Err(err("successful compaction/end requires one summary"));
                    }
                }
                compaction = None;
            }
            "compaction/prune" => current_surface_span(&surface, d, "compaction/prune")?,
            "user/message" => {
                let source = record(&d["source"], &format!("user/message {seq} source"))?;
                if field(event, "surfaceOp") != "append"
                    && source["kind"] == "plugin"
                    && source["plugin"] == "compact"
                {
                    compaction_owner(
                        compaction.as_ref(),
                        source,
                        &format!("compaction checkpoint at seq {seq}"),
                    )?;
                }
            }
            "session/end-seed" => compaction = None,
            _ => {}
        }
    }
    Ok(())
}
fn inherited_orphan_compaction_starts(events: &[SessionFormatEvent]) -> HashSet<u64> {
    let mut stale = HashSet::new();
    let mut open = None;
    for e in events {
        match event_type(e) {
            "compaction/start" => open = Some(event_seq(e)),
            "compaction/end" => open = None,
            "session/end-seed" => {
                if let Some(seq) = open.take() {
                    stale.insert(seq);
                }
            }
            _ => {}
        }
    }
    stale
}
fn assert_retry_chain(retries: &[&SessionFormatEvent], d: &Value) -> Result {
    let prior = retries.iter().rev().find(|e| {
        let p = field(e, "data");
        ["turn", "step", "provider", "policyKey"]
            .iter()
            .all(|key| deep_equal_json(&p[*key], &d[*key]))
    });
    let expected = prior.map(|e| n(&field(e, "data")["retry"])).unwrap_or(0) + 1;
    if !crate::validation::number_eq(&d["retry"], expected) {
        return Err(err(format!("llm/retry must use retry {expected}")));
    }
    if prior.is_some_and(|e| field(e, "data")["retryId"] != d["retryId"]) {
        return Err(err(
            "llm/retry must preserve retryId across one policy chain",
        ));
    }
    if prior.is_none()
        && retries
            .iter()
            .any(|e| field(e, "data")["retryId"] == d["retryId"])
    {
        return Err(err(format!(
            "llm/retry reuses retryId {} across policy chains",
            js_stringify(&d["retryId"])
        )));
    }
    Ok(())
}
fn require_open_step(
    event: &SessionFormatEvent,
    d: &Value,
    turn: Option<u64>,
    step: Option<u64>,
) -> Result {
    if !equal_count(&d["turn"], turn)
        || !equal_count(&d["step"], step)
        || turn.is_none()
        || step.is_none()
    {
        return Err(err(format!(
            "{} does not match an open turn and step",
            event_type(event)
        )));
    }
    Ok(())
}
fn no_unresolved(tools: &IndexMap<String, ToolLifecycle>, boundary: &str) -> Result {
    if let Some(id) = tools.keys().next() {
        return Err(err(format!("{boundary} leaves unresolved tool call {id}")));
    }
    Ok(())
}
fn is_exact_tool_not_started_repair(
    event: &SessionFormatEvent,
    d: &Value,
    m: &Value,
    id: &str,
) -> bool {
    let block = &m["content"][0];
    let content = &block["content"];
    d["error"]["name"] == "ToolNotStartedError"
        && d["error"]["code"] == "TOOL_NOT_STARTED"
        && !event.contains_key("sourceEventSeqs")
        && m["id"] == format!("interrupted-tool-result-{id}-{}", event_seq(event))
        && block["isError"] == true
        && content.as_array().is_some_and(|a| a.len() == 1)
        && content[0]["type"] == "text"
        && content[0]["text"]
            == "The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."
}
fn apply_surface(
    surface: &[u64],
    event: &SessionFormatEvent,
) -> std::result::Result<Vec<u64>, SessionFormatError> {
    let kind = event_type(event);
    let seq = event_seq(event);
    let Some(op) = event.get("surfaceOp") else {
        return Err(err(format!("{kind} requires a surfaceOp marker")));
    };
    if op == "append" {
        let mut output = surface.to_vec();
        output.push(seq);
        return Ok(output);
    }
    let start = surface
        .iter()
        .position(|v| crate::validation::number_eq(&op["start"], *v));
    let end = surface
        .iter()
        .position(|v| crate::validation::number_eq(&op["end"], *v));
    let (Some(start), Some(end)) = (start, end) else {
        return Err(err(format!(
            "{kind} replacement range is not on the current surface"
        )));
    };
    if end < start {
        return Err(err(format!(
            "{kind} replacement range is not on the current surface"
        )));
    }
    let sources = field(event, "sourceEventSeqs").as_array();
    if surface[start..=end].iter().any(|seq| {
        !sources.is_some_and(|a| a.iter().any(|v| crate::validation::number_eq(v, *seq)))
    }) {
        return Err(err(format!(
            "{kind} replacement sourceEventSeqs omit a shadowed surface node"
        )));
    }
    Ok(surface[..start]
        .iter()
        .copied()
        .chain([seq])
        .chain(surface[end + 1..].iter().copied())
        .collect())
}
fn assert_title_sources(
    events: &[SessionFormatEvent],
    event: &SessionFormatEvent,
    d: &Value,
    validate_text: bool,
) -> Result {
    let kind = event_type(event);
    let seq = event_seq(event);
    let seqs = d["messageSeqs"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if kind == "session/title" {
        let source = record(&d["source"], &format!("session/title {seq} source"))?;
        if seqs.is_empty() != (source["kind"] == "user") {
            return Err(err(format!(
                "session/title {seq} messageSeqs must be empty exactly for a user title"
            )));
        }
    }
    let mut selected = Vec::new();
    for source_seq in seqs {
        let invalid = || {
            err(format!(
                "{kind} {seq} messageSeqs must cite earlier human user/message events"
            ))
        };
        let Some(source) = events
            .get(n(source_seq) as usize)
            .filter(|e| event_type(e) == "user/message")
        else {
            return Err(invalid());
        };
        let sd = record(
            field(source, "data"),
            &format!("user/message {} data", n(source_seq)),
        )?;
        let ms = record(
            &sd["source"],
            &format!("user/message {} source", n(source_seq)),
        )?;
        if ms["kind"] != "user" {
            return Err(invalid());
        }
        let text = sd["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| {
                if b["type"] == "text" {
                    b["text"].as_str()
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        selected.push(json!({"seq":source_seq,"text":text}));
    }
    if kind == "session/title-llm-request" {
        let expected = format!(
            "Generate the session title from this JSON array of human messages:\n{}",
            js_stringify(&Value::Array(selected))
        );
        let messages = &d["messages"];
        let m = &messages[0];
        let content = &m["content"];
        let source = if messages.as_array().is_some_and(|a| !a.is_empty()) {
            Some(record(
                &m["source"],
                "session/title-llm-request message source",
            )?)
        } else {
            None
        };
        if !messages.as_array().is_some_and(|a| a.len() == 1)
            || m["role"] != "user"
            || !content.as_array().is_some_and(|a| a.len() == 1)
            || !source
                .is_some_and(|s| s["kind"] == "plugin" && s["plugin"] == "dsh-session-title-llm")
        {
            return Err(err(
                "session/title-llm-request messages do not represent messageSeqs",
            ));
        }
        if content[0]["type"] != "text" || (validate_text && content[0]["text"] != expected) {
            return Err(err(
                "session/title-llm-request messages do not represent messageSeqs",
            ));
        }
    }
    Ok(())
}
fn compaction_owner(open: Option<&CompactionState>, d: &Value, kind: &str) -> Result {
    if !open.is_some_and(|c| {
        d["compactionId"] == c.id && d.get("sourceCommandId") == c.source_command_id.as_ref()
    }) {
        return Err(err(format!("{kind} has no matching compaction/start")));
    }
    Ok(())
}
fn compaction_turn(owner: &Value, open_turn: Option<u64>, kind: &str) -> Result {
    if !equal_count(owner, open_turn) {
        return Err(err(format!("{kind} does not match the open turn")));
    }
    Ok(())
}
fn current_surface_span(surface: &[u64], d: &Value, kind: &str) -> Result {
    let start = surface
        .iter()
        .position(|v| crate::validation::number_eq(&d["shadowedRange"]["start"], *v));
    let end = surface
        .iter()
        .position(|v| crate::validation::number_eq(&d["shadowedRange"]["end"], *v));
    let expected = match (start, end) {
        (Some(a), Some(b)) if b >= a => &surface[a..=b],
        _ => &[],
    };
    let seqs = d["shadowedSeqs"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if expected.len() != seqs.len()
        || expected
            .iter()
            .zip(seqs)
            .any(|(a, b)| !crate::validation::number_eq(b, *a))
    {
        return Err(err(format!(
            "{kind} shadowedSeqs do not name an exact current surface span"
        )));
    }
    Ok(())
}
