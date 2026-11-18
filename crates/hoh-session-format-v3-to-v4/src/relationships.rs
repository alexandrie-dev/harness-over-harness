use crate::helpers::*;
use hoh_session_format::{SessionFormatArtifact, SessionFormatEvent, SessionFormatJsonObject};
use hoh_util_values::deep_equal_json;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::collections::HashSet;
const SURFACE_TYPES: &[&str] = &[
    "system/message",
    "user/message",
    "developer/message",
    "assistant/message",
    "tool/result",
];
const STEP_EVENT_TYPES: &[&str] = &["system/message", "developer/message", "assistant/attempt"];
const RELATIONSHIP_TYPES: &[&str] = &[
    "system/message",
    "user/message",
    "developer/message",
    "assistant/message",
    "tool/result",
    "assistant/attempt",
    "turn/start",
    "turn/end",
    "step/start",
    "step/end",
    "tool/call",
    "request/header",
    "request/context",
    "tool/ptc-dispatch-start",
    "tool/ptc-dispatch",
    "llm/retry",
    "llm/retry-started",
    "session/title",
    "session/title-llm-request",
    "command/run",
    "command/done",
    "compaction/start",
    "compaction/summary",
    "compaction/end",
    "compaction/prune",
    "session/end-seed",
];
fn text_required<'a>(value: Option<&'a Value>, subject: &str) -> Result<&'a str> {
    nonempty(value).ok_or_else(|| error(format!("{subject} requires a nonempty string")))
}
struct ToolState<'a> {
    name: Option<&'a Value>,
    arguments: Option<&'a Value>,
    started: bool,
}
struct Compaction<'a> {
    id: &'a str,
    command: Option<&'a Value>,
    turn: Option<u64>,
    seq: u64,
    summarized: bool,
}
struct Dispatch<'a> {
    data: &'a SessionFormatJsonObject,
    settled: bool,
}
struct Relationships<'a> {
    artifact: &'a SessionFormatArtifact,
    known: &'a HashSet<String>,
    turn: Option<u64>,
    step: Option<u64>,
    next_turn: u64,
    next_step: u64,
    provider: Option<&'a Value>,
    surface: Vec<u64>,
    protected_head: Option<u64>,
    compaction: Option<Compaction<'a>>,
    tools: IndexMap<String, ToolState<'a>>,
    dispatches: IndexMap<String, Dispatch<'a>>,
    retries: Vec<&'a SessionFormatJsonObject>,
    started_retries: HashSet<String>,
    commands: HashSet<String>,
    orphan_compactions: HashSet<u64>,
}
impl<'a> Relationships<'a> {
    fn new(artifact: &'a SessionFormatArtifact, known: &'a HashSet<String>) -> Result<Self> {
        let mut start = None;
        let mut orphan_compactions = HashSet::new();
        for event in &artifact.events {
            if !known.contains(kind(event)) {
                continue;
            }
            match kind(event) {
                "compaction/start" => start = Some(count(event.get("seq"), "event seq")?),
                "compaction/end" => start = None,
                "session/end-seed" => {
                    if let Some(s) = start {
                        orphan_compactions.insert(s);
                    }
                    start = None;
                }
                _ => {}
            }
        }
        Ok(Self {
            artifact,
            known,
            turn: None,
            step: None,
            next_turn: 1,
            next_step: 1,
            provider: None,
            surface: vec![],
            protected_head: None,
            compaction: None,
            tools: IndexMap::new(),
            dispatches: IndexMap::new(),
            retries: vec![],
            started_retries: HashSet::new(),
            commands: HashSet::new(),
            orphan_compactions,
        })
    }
    fn require_turn(&self, tag: &str) -> Result {
        if self.turn.is_none() {
            return Err(error(format!("{tag} is outside an open turn")));
        }
        Ok(())
    }
    fn require_step(&self, event: &SessionFormatEvent, data: &SessionFormatJsonObject) -> Result {
        if !self.turn.is_some_and(|n| number_eq(data.get("turn"), n))
            || !self.step.is_some_and(|n| number_eq(data.get("step"), n))
        {
            return Err(error(format!(
                "{} does not match an open turn and step",
                kind(event)
            )));
        }
        Ok(())
    }
    fn close_tools(&mut self, tag: &str) -> Result {
        if let Some((id, _)) = self.tools.first() {
            return Err(error(format!("{tag} leaves unresolved tool call {id}")));
        }
        self.tools.clear();
        Ok(())
    }
    fn developer(&self, event: &SessionFormatEvent, data: &SessionFormatJsonObject) -> Result {
        let Some(header_seq) = data.get("headerSeq") else {
            return Ok(());
        };
        let seq = earlier(
            Some(header_seq),
            count(event.get("seq"), "event seq")?,
            "developer/message headerSeq",
        )?;
        let header_event = event_at(&self.artifact.events, seq)
            .filter(|e| kind(e) == "request/header")
            .ok_or_else(|| {
                error("developer/message headerSeq must reference an earlier request/header")
            })?;
        if !self.known.contains(kind(header_event)) {
            return Err(error(
                "developer/message headerSeq must reference a known request/header",
            ));
        }
        let tools = record(
            record(header_event.get("data"), "request/header data")?.get("header"),
            "request header",
        )?
        .get("tools");
        let content = array(
            record(data.get("message"), "developer message")?.get("content"),
            "developer content",
        )?;
        for value in content {
            let Some(block) = value.as_object().filter(|b| kind(b) == "tool-addition") else {
                continue;
            };
            let name = field(block, "toolName");
            let definitions: Vec<_> = tools
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .filter(|tool| strict_eq(tool.get("name"), Some(name)))
                .collect();
            let name = text(Some(name));
            if definitions.len() != 1 {
                return Err(error(format!(
                    "developer/message tool-addition \"{name}\" must name exactly one tool in headerSeq {seq}"
                )));
            }
            let definition = definitions[0];
            if !field(definition, "description").is_string()
                || !field(definition, "parameters").is_object()
            {
                return Err(error(format!(
                    "developer/message tool-addition \"{name}\" requires a complete tool definition in headerSeq {seq}"
                )));
            }
        }
        Ok(())
    }
    fn fold_surface(&mut self, event: &SessionFormatEvent) -> Result {
        let tag = kind(event);
        if !SURFACE_TYPES.contains(&tag) {
            return Ok(());
        }
        if tag == "system/message" && !self.surface.is_empty() && self.protected_head.is_none() {
            return Err(error(
                "system/message requires a protected first surface head",
            ));
        }
        let seq = count(event.get("seq"), "event seq")?;
        if field(event, "surfaceOp") == "append" {
            if tag == "system/message" && self.surface.is_empty() {
                self.protected_head = Some(seq);
            }
            self.surface.push(seq);
            return Ok(());
        }
        let operation = record(event.get("surfaceOp"), &format!("{tag} surfaceOp"))?;
        let start = earlier(operation.get("startSeq"), seq, "replacement start")?;
        let end = earlier(operation.get("endSeq"), seq, "replacement end")?;
        let first = self.surface.iter().position(|n| *n == start);
        let last = self.surface.iter().position(|n| *n == end);
        let (Some(first), Some(last)) = (first, last) else {
            return Err(error(format!(
                "{tag} replacement range is not on the current surface"
            )));
        };
        if last < first {
            return Err(error(format!(
                "{tag} replacement range is not on the current surface"
            )));
        }
        let removed = &self.surface[first..=last];
        let sources = array(event.get("sourceEventSeqs"), "replacement sourceEventSeqs")?;
        if removed
            .iter()
            .any(|n| !sources.iter().any(|s| number_eq(Some(s), *n)))
        {
            return Err(error(
                "replacement sourceEventSeqs omit a shadowed surface node",
            ));
        }
        if self
            .protected_head
            .is_some_and(|head| removed.contains(&head))
        {
            if tag != "system/message" || removed.len() != 1 {
                return Err(error(
                    "surface replacement cannot shadow the protected system head",
                ));
            }
            self.protected_head = Some(seq);
        }
        self.surface.splice(first..=last, [seq]);
        Ok(())
    }
    fn tool(&mut self, event: &SessionFormatEvent, data: &'a SessionFormatJsonObject) -> Result {
        let tag = kind(event);
        if tag == "tool/result" && field(event, "surfaceOp") != "append" {
            return self.require_turn(tag);
        }
        self.require_step(event, data)?;
        if tag == "assistant/message" {
            let message = record(data.get("message"), "assistant message")?;
            for value in array(message.get("content"), "assistant content")? {
                let block = record(Some(value), "assistant content block")?;
                if kind(block) != "tool-call" {
                    continue;
                }
                let id = text_required(block.get("id"), "tool call id")?;
                if self.tools.contains_key(id) {
                    return Err(error(format!(
                        "assistant/message repeats advertised tool call {id}"
                    )));
                }
                self.tools.insert(
                    id.into(),
                    ToolState {
                        name: block.get("name"),
                        arguments: block.get("arguments"),
                        started: false,
                    },
                );
            }
            return Ok(());
        }
        let message = if tag == "tool/result" {
            Some(record(data.get("message"), "tool result message")?)
        } else {
            None
        };
        let id = text_required(
            message.map_or_else(|| data.get("callId"), |m| m.get("toolCallId")),
            "tool call id",
        )?;
        let pending = self
            .tools
            .get_mut(id)
            .ok_or_else(|| error(format!("{tag} {id} has no advertised tool lifecycle")))?;
        if let Some(message) = message {
            if !pending.started && !not_started_repair(event, data, message, id)? {
                return Err(error(format!(
                    "tool/result {id} is not the exact TOOL_NOT_STARTED repair"
                )));
            }
            self.tools.shift_remove(id);
        } else {
            if pending.started
                || !strict_eq(pending.name, data.get("name"))
                || !strict_eq(pending.arguments, data.get("arguments"))
            {
                return Err(error(format!(
                    "tool/call {id} does not match one advertised tool call"
                )));
            }
            pending.started = true;
        }
        Ok(())
    }
    fn dispatch(
        &mut self,
        event: &SessionFormatEvent,
        data: &'a SessionFormatJsonObject,
    ) -> Result {
        self.require_turn(kind(event))?;
        let id = text_required(data.get("subCallId"), "PTC subCallId")?;
        let root = text_required(data.get("rootCallId"), "PTC rootCallId")?;
        let parent = text_required(data.get("parentCallId"), "PTC parentCallId")?;
        let existing = self.dispatches.get(id);
        if existing.is_some_and(|e| field(e.data, "rootCallId") != root) {
            return Err(error("PTC dispatch changes its rootCallId"));
        }
        if parent != root
            && !self
                .dispatches
                .get(parent)
                .is_some_and(|p| field(p.data, "rootCallId") == root)
        {
            return Err(error("PTC parentCallId does not belong to rootCallId"));
        }
        if kind(event) == "tool/ptc-dispatch-start" {
            if existing.is_some() {
                return Err(error("PTC dispatch repeats subCallId"));
            }
            self.dispatches.insert(
                id.into(),
                Dispatch {
                    data,
                    settled: false,
                },
            );
            return Ok(());
        }
        let existing = self
            .dispatches
            .get_mut(id)
            .filter(|e| !e.settled)
            .ok_or_else(|| error("PTC dispatch has no unique start"))?;
        if ["rootCallId", "parentCallId", "name", "arguments"]
            .iter()
            .any(|key| !deep_optional(existing.data.get(*key), data.get(*key)))
        {
            return Err(error("PTC dispatch does not match its start"));
        }
        existing.settled = true;
        Ok(())
    }
    fn retry(&mut self, event: &SessionFormatEvent, data: &'a SessionFormatJsonObject) -> Result {
        let id = text_required(data.get("retryId"), "retryId")?;
        let attempt = count(data.get("retry"), "retry")?;
        if kind(event) == "llm/retry-started" {
            let scheduled = self
                .retries
                .iter()
                .find(|r| field(r, "retryId") == id && number_eq(r.get("retry"), attempt))
                .ok_or_else(|| error("llm/retry-started pairs no prior scheduled attempt"))?;
            if !strict_eq(scheduled.get("turn"), data.get("turn"))
                || !strict_eq(scheduled.get("step"), data.get("step"))
            {
                return Err(error("llm/retry-started changes scheduled coordinates"));
            }
            let key = hoh_util_values::js_json_stringify(&json!([id, attempt]));
            if !self.started_retries.insert(key) {
                return Err(error("llm/retry-started repeats one scheduled attempt"));
            }
            return Ok(());
        }
        if !self.turn.is_some_and(|n| number_eq(data.get("turn"), n))
            || !number_eq(data.get("step"), self.step.unwrap_or(self.next_step - 1))
        {
            return Err(error("llm/retry does not match the current turn and step"));
        }
        if !strict_eq(data.get("provider"), self.provider) {
            return Err(error(
                "llm/retry provider does not match the open request/header",
            ));
        }
        let prior = self.retries.iter().rev().find(|r| {
            ["turn", "step", "provider", "policyKey"]
                .iter()
                .all(|k| strict_eq(r.get(*k), data.get(*k)))
        });
        let expected = if let Some(prior) = prior {
            count(prior.get("retry"), "prior retry")? + 1
        } else {
            1
        };
        if attempt != expected {
            return Err(error("llm/retry skips its policy attempt sequence"));
        }
        if prior.map_or_else(
            || self.retries.iter().any(|r| field(r, "retryId") == id),
            |r| field(r, "retryId") != id,
        ) {
            return Err(error("llm/retry must keep one retryId per policy chain"));
        }
        self.retries.push(data);
        Ok(())
    }
    fn compaction_owner(
        &self,
        data: &SessionFormatJsonObject,
        subject: &str,
    ) -> Result<&Compaction<'a>> {
        self.compaction
            .as_ref()
            .filter(|c| {
                field(data, "compactionId") == c.id
                    && strict_eq(c.command, data.get("sourceCommandId"))
            })
            .ok_or_else(|| error(format!("{subject} has no matching compaction/start")))
    }
    fn span(&self, event: &SessionFormatEvent, data: &SessionFormatJsonObject) -> Result {
        let range = record(data.get("shadowedRange"), "compaction shadowedRange")?;
        let seq = count(event.get("seq"), "event seq")?;
        let start = earlier(range.get("start"), seq, "compaction range start")?;
        let end = earlier(range.get("end"), seq, "compaction range end")?;
        let first = self.surface.iter().position(|n| *n == start);
        let last = self.surface.iter().position(|n| *n == end);
        let seqs = array(data.get("shadowedSeqs"), "compaction shadowedSeqs")?;
        if !first.zip(last).is_some_and(|(a, b)| {
            b >= a
                && deep_optional(
                    Some(&json!(&self.surface[a..=b])),
                    Some(&Value::Array(seqs.clone())),
                )
        }) {
            return Err(error(format!(
                "{} shadowedSeqs do not name an exact current surface span",
                kind(event)
            )));
        }
        if self
            .protected_head
            .is_some_and(|n| seqs.iter().any(|v| number_eq(Some(v), n)))
        {
            return Err(error("compaction cannot shadow the protected system head"));
        }
        Ok(())
    }
    fn compact(&mut self, event: &SessionFormatEvent, data: &'a SessionFormatJsonObject) -> Result {
        let tag = kind(event);
        if matches!(tag, "compaction/prune" | "compaction/summary") {
            self.span(event, data)?;
        }
        if tag == "compaction/prune" {
            return Ok(());
        }
        if tag == "compaction/start" {
            if self.compaction.is_some() {
                return Err(error("compaction/start overlaps an open compaction"));
            }
            let owner = if data.get("turn") == Some(&Value::Null) {
                None
            } else {
                Some(count(data.get("turn"), "compaction turn")?)
            };
            if owner != self.turn {
                return Err(error("compaction/start does not match the open turn"));
            }
            self.compaction = Some(Compaction {
                id: text_required(data.get("compactionId"), "compactionId")?,
                command: data.get("sourceCommandId"),
                turn: owner,
                seq: count(event.get("seq"), "event seq")?,
                summarized: false,
            });
            return Ok(());
        }
        let current = self.compaction_owner(data, tag)?;
        if current.turn != self.turn {
            return Err(error(format!("{tag} does not match the open turn")));
        }
        if tag == "compaction/summary" {
            if current.summarized {
                return Err(error("compaction/summary repeats"));
            }
            if let Some(current) = self.compaction.as_mut() {
                current.summarized = true;
            }
        } else {
            if !optional_turn_eq(data.get("turn"), current.turn) {
                return Err(error("compaction/end changes its owner turn"));
            }
            if !data.contains_key("error") && !current.summarized {
                return Err(error("successful compaction/end requires one summary"));
            }
            self.compaction = None;
        }
        Ok(())
    }
    fn accept(&mut self, event: &'a SessionFormatEvent) -> Result {
        let tag = kind(event);
        if !self.known.contains(tag) || !RELATIONSHIP_TYPES.contains(&tag) {
            return Ok(());
        }
        let data = record(event.get("data"), tag)?;
        if STEP_EVENT_TYPES.contains(&tag) {
            self.require_step(event, data)?;
        }
        if tag.starts_with("turn/")
            && self
                .compaction
                .as_ref()
                .is_some_and(|c| !self.orphan_compactions.contains(&c.seq))
        {
            return Err(error(format!("{tag} crosses an open compaction")));
        }
        self.fold_surface(event)?;
        match tag {
            "turn/start" => {
                if self.turn.is_some() || !number_eq(data.get("turn"), self.next_turn) {
                    return Err(error("turn/start does not open the expected turn"));
                }
                self.turn = Some(self.next_turn);
                self.next_step = 1;
                self.tools.clear();
            }
            "turn/end" => {
                if !self.turn.is_some_and(|n| number_eq(data.get("turn"), n)) || self.step.is_some()
                {
                    return Err(error(
                        "turn/end does not match the open turn with no open step",
                    ));
                }
                self.close_tools(tag)?;
                self.turn = None;
                self.next_turn += 1;
            }
            "step/start" => {
                if !self.turn.is_some_and(|n| number_eq(data.get("turn"), n))
                    || self.step.is_some()
                    || !number_eq(data.get("step"), self.next_step)
                {
                    return Err(error(
                        "step/start does not match the open turn and next step",
                    ));
                }
                self.step = Some(self.next_step);
            }
            "step/end" => {
                self.require_step(event, data)?;
                self.close_tools(tag)?;
                self.step = None;
                self.next_step += 1;
            }
            "assistant/message" | "tool/call" | "tool/result" => self.tool(event, data)?,
            "developer/message" => self.developer(event, data)?,
            "request/header" => {
                self.require_turn(tag)?;
                self.provider = record(
                    record(data.get("header"), "request header")?.get("config"),
                    "request config",
                )?
                .get("provider");
            }
            "request/context" => self.require_turn(tag)?,
            "tool/ptc-dispatch-start" | "tool/ptc-dispatch" => self.dispatch(event, data)?,
            "llm/retry" | "llm/retry-started" => self.retry(event, data)?,
            "session/title" | "session/title-llm-request" => {
                title_sources(&self.artifact.events, event, data, self.known)?
            }
            "command/run" => {
                let id = text_required(data.get("commandId"), "commandId")?;
                if !self.commands.insert(id.into()) {
                    return Err(error(format!("command/run repeats commandId {id}")));
                }
            }
            "command/done" => {
                if !self
                    .commands
                    .contains(text_required(data.get("commandId"), "commandId")?)
                {
                    return Err(error("command/done has no prior command/run"));
                }
                if let Some(source) = data.get("sourceEventSeq") {
                    let seq = earlier(
                        Some(source),
                        count(event.get("seq"), "event seq")?,
                        "command sourceEventSeq",
                    )?;
                    let tag = event_at(&self.artifact.events, seq).map(kind);
                    if field(data, "kind") != "success"
                        || matches!(tag, Some("command/run" | "command/done"))
                    {
                        return Err(error("command/done has invalid sourceEventSeq"));
                    }
                }
            }
            "compaction/start" | "compaction/summary" | "compaction/end" | "compaction/prune" => {
                self.compact(event, data)?
            }
            "user/message" => {
                let source = record(data.get("source"), "user message source")?;
                if field(event, "surfaceOp") != "append"
                    && field(source, "kind") == "compact-checkpoint"
                {
                    self.compaction_owner(source, "compaction checkpoint")?;
                }
            }
            "session/end-seed" => self.compaction = None,
            _ => {}
        }
        Ok(())
    }
}
fn deep_optional(a: Option<&Value>, b: Option<&Value>) -> bool {
    let (a, b) = match (a, b) {
        (None, None) => return true,
        (Some(a), Some(b)) => (a, b),
        _ => return false,
    };
    if !deep_equal_json(a, b) {
        return false;
    }
    // Node isDeepStrictEqual also distinguishes negative zero, including nested values.
    let mut pending = vec![(a, b)];
    while let Some((a, b)) = pending.pop() {
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => {
                if let (Some(a), Some(b)) = (a.as_f64(), b.as_f64())
                    && a == 0.
                    && b == 0.
                    && a.is_sign_negative() != b.is_sign_negative()
                {
                    return false;
                }
            }
            (Value::Array(a), Value::Array(b)) => pending.extend(a.iter().zip(b)),
            (Value::Object(a), Value::Object(b)) => {
                for (key, a) in a {
                    if let Some(b) = b.get(key) {
                        pending.push((a, b));
                    }
                }
            }
            _ => {}
        }
    }
    true
}
fn optional_turn_eq(value: Option<&Value>, turn: Option<u64>) -> bool {
    turn.map_or(value == Some(&Value::Null), |n| number_eq(value, n))
}
fn not_started_repair(
    event: &SessionFormatEvent,
    data: &SessionFormatJsonObject,
    message: &SessionFormatJsonObject,
    call_id: &str,
) -> Result<bool> {
    let err = record(data.get("error"), "not-started error")?;
    if field(err, "name") != "ToolNotStartedError"
        || field(err, "code") != "TOOL_NOT_STARTED"
        || field(message, "isError") != true
        || event.contains_key("sourceEventSeqs")
    {
        return Ok(false);
    }
    let id = text_required(message.get("id"), "not-started message id")?;
    if id.starts_with(&format!("forked-tool-result-{call_id}-")) {
        return Ok(true);
    }
    let prefix = format!("interrupted-tool-result-{call_id}-");
    let content = array(message.get("content"), "not-started content")?;
    Ok(id.strip_prefix(&prefix).and_then(canonical_suffix).is_some() && content.len()==1 && content[0].get("type").is_some_and(|t|t=="text") && content[0].get("text").is_some_and(|t|t=="The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."))
}
fn title_sources(
    events: &[SessionFormatEvent],
    event: &SessionFormatEvent,
    data: &SessionFormatJsonObject,
    known: &HashSet<String>,
) -> Result {
    let references = array(data.get("messageSeqs"), "title messageSeqs")?;
    if kind(event) == "session/title"
        && references.is_empty()
            != (field(record(data.get("source"), "title source")?, "kind") == "user")
    {
        return Err(error(
            "session/title messageSeqs must be empty exactly for a user title",
        ));
    }
    let mut seen = HashSet::new();
    let seq = count(event.get("seq"), "event seq")?;
    for value in references {
        let n = earlier(Some(value), seq, "title messageSeqs")?;
        let source = event_at(events, n);
        if seen.contains(&n)
            || !source.is_some_and(|s| kind(s) == "user/message" && known.contains(kind(s)))
        {
            return Err(error(format!(
                "{} messageSeqs must cite distinct earlier human user/message events",
                kind(event)
            )));
        }
        // The preceding event-kind check proves the source exists.
        let source = source.expect("known earlier user event");
        if field(
            record(
                record(source.get("data"), "title input")?.get("source"),
                "title input source",
            )?,
            "kind",
        ) != "user"
        {
            return Err(error(format!(
                "{} messageSeqs must cite distinct earlier human user/message events",
                kind(event)
            )));
        }
        seen.insert(n);
    }
    if kind(event) != "session/title-llm-request" {
        return Ok(());
    }
    let messages = array(data.get("messages"), "title messages")?;
    let message = record(messages.first(), "title message")?;
    let content = array(message.get("content"), "title content")?;
    if references.is_empty()
        || messages.len() != 1
        || field(message, "role") != "user"
        || field(record(message.get("source"), "title source")?, "kind") != "dsh-session-title-llm"
        || content.len() != 1
        || !content[0].get("type").is_some_and(|t| t == "text")
    {
        return Err(error(
            "session/title-llm-request messages do not represent messageSeqs",
        ));
    }
    Ok(())
}
pub fn assert_v4_lifecycle_relationships(
    artifact: &SessionFormatArtifact,
    known_event_types: &HashSet<String>,
) -> Result {
    let mut state = Relationships::new(artifact, known_event_types)?;
    for event in &artifact.events {
        state.accept(event)?;
    }
    Ok(())
}
