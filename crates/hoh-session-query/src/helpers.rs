use crate::error::{Result, SessionQueryError, SessionQueryErrorCode as Code};
use crate::types::*;
use feruca::{Collator, Locale, Tailoring};
use hoh_error::BoxError;
use hoh_session::{SessionEvent, SessionHeader, SessionId, SessionSeq, fold_surface};
use hoh_util_values::{JS_WHITE_SPACE_SET, js_trim};
use regex::{Regex, RegexBuilder};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn string_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str()
}

fn content_text(content: &Value) -> Vec<String> {
    let mut result = Vec::new();
    for block in content.as_array().into_iter().flatten() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    result.push(text.to_owned());
                }
            }
            Some("tool-call") => {
                if let Some(name) = block.get("name").and_then(Value::as_str) {
                    result.push(name.to_owned());
                }
                if let Some(arguments) = block.get("arguments").and_then(Value::as_str) {
                    result.push(arguments.to_owned());
                }
            }
            _ => {}
        }
    }
    result
}

fn semantic_parts(event: &SessionEvent) -> Vec<String> {
    let data = event.data();
    match event.event_type() {
        "user/message" => data.get("content").map(content_text).unwrap_or_default(),
        "assistant/message" => data
            .get("message")
            .and_then(|message| message.get("content"))
            .map(content_text)
            .unwrap_or_default(),
        "tool/call" => [data.get("name"), data.get("arguments")]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        "tool/result" => {
            let mut result = data
                .get("message")
                .and_then(|message| message.get("content"))
                .map(content_text)
                .unwrap_or_default();
            for key in ["name", "code"] {
                if let Some(value) = data
                    .get("error")
                    .and_then(|error| error.get(key))
                    .and_then(Value::as_str)
                {
                    result.push(value.to_owned());
                }
            }
            result
        }
        "todo/write" => data
            .get("todos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|todo| [todo.get("status"), todo.get("content")])
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        "turn/end" => match string_at(data, &["reason", "kind"]) {
            Some("error") => {
                let mut parts = vec!["error".to_owned()];
                if let Some(message) = string_at(data, &["reason", "error", "message"]) {
                    parts.push(message.to_owned());
                }
                parts
            }
            Some("aborted") => vec!["aborted".to_owned()],
            Some(kind @ ("max-tokens" | "interrupted")) => vec![kind.to_owned()],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

pub fn extract_session_event_text(event: &SessionEvent) -> String {
    semantic_parts(event)
        .into_iter()
        .map(|part| js_trim(&part).to_owned())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn projections() -> Vec<Arc<dyn hoh_session::SessionMessageProjection>> {
    let projections =
        hoh_session_format_catalog::message_projections::current_session_message_projections();
    projections
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn surfaces(events: &[Arc<SessionEvent>]) -> Result<HashMap<SessionSeq, SessionEventSurface>> {
    let fold = fold_surface(events, &projections()).map_err(|cause| {
        Box::new(SessionQueryError::with_cause(
            format!("invalid session surface: {cause}"),
            Code::InvalidSurface,
            cause,
        )) as BoxError
    })?;
    let current = fold.nodes.into_iter().collect::<HashSet<_>>();
    let shadowed = fold
        .replacements
        .iter()
        .flat_map(|replacement| replacement.shadowed_seqs.iter().copied())
        .collect::<HashSet<_>>();
    Ok(events
        .iter()
        .map(|event| {
            let surface = if current.contains(&event.seq()) {
                SessionEventSurface::Current
            } else if shadowed.contains(&event.seq()) {
                SessionEventSurface::Shadowed
            } else {
                SessionEventSurface::LogOnly
            };
            (event.seq(), surface)
        })
        .collect())
}

pub fn build_session_event_records(
    session_id: &SessionId,
    events: &[Arc<SessionEvent>],
) -> Result<Vec<SessionEventRecord>> {
    let surfaces = surfaces(events)?;
    Ok(events
        .iter()
        .map(|event| SessionEventRecord {
            session_id: session_id.clone(),
            seq: event.seq(),
            r#type: event.event_type().to_owned(),
            time: event.time(),
            surface: surfaces[&event.seq()],
        })
        .collect())
}

pub fn build_session_event_search_documents(
    session_id: &SessionId,
    events: &[Arc<SessionEvent>],
) -> Result<Vec<SessionEventSearchDocument>> {
    let records = build_session_event_records(session_id, events)?;
    Ok(events
        .iter()
        .zip(records)
        .filter_map(|(event, record)| {
            let text = extract_session_event_text(event);
            (!text.is_empty()).then_some(SessionEventSearchDocument { record, text })
        })
        .collect())
}

fn validate_range(name: &str, range: SessionResultRange) -> Result<()> {
    let detail = if range.from.is_some_and(|value| !value.is_finite()) {
        Some("from must be finite")
    } else if range.to.is_some_and(|value| !value.is_finite()) {
        Some("to must be finite")
    } else if range.from.zip(range.to).is_some_and(|(from, to)| from > to) {
        Some("from must be less than or equal to to")
    } else {
        None
    };
    if let Some(detail) = detail {
        return Err(SessionQueryError::boxed(
            format!("session {name} filter {detail}"),
            Code::InvalidFilter,
        ));
    }
    Ok(())
}

pub fn materialize_session_result_filters(
    filters: &[SessionResultFilter],
) -> Result<Vec<SessionResultFilter>> {
    for filter in filters {
        if let SessionResultFilter::CreatedAt(range) = filter {
            validate_range("created-at", *range)?;
        }
    }
    Ok(filters.to_vec())
}

pub fn materialize_session_event_result_filters(
    filters: &[SessionEventResultFilter],
) -> Result<Vec<SessionEventResultFilter>> {
    for filter in filters {
        match filter {
            SessionEventResultFilter::Seq(range) => validate_range("seq", *range)?,
            SessionEventResultFilter::Time(range) => validate_range("time", *range)?,
            _ => {}
        }
    }
    Ok(filters.to_vec())
}

fn in_range(value: f64, range: SessionResultRange) -> bool {
    range.from.is_none_or(|from| value >= from) && range.to.is_none_or(|to| value <= to)
}

pub fn filter_session_results<T: SessionRecordLike + Clone>(
    records: &[T],
    filters: &[SessionResultFilter],
) -> Result<Vec<T>> {
    let filters = materialize_session_result_filters(filters)?;
    Ok(records
        .iter()
        .filter(|value| {
            let record = value.session_record();
            filters.iter().all(|filter| match filter {
                SessionResultFilter::Id(values) => values.contains(&record.header.id),
                SessionResultFilter::Cwd(values) => values.contains(&record.header.cwd),
                SessionResultFilter::CreatedAt(range) => {
                    in_range(record.header.created_at as f64, *range)
                }
                SessionResultFilter::Parent(values) => {
                    values.contains(&record.header.parent_session)
                }
                SessionResultFilter::Availability(values) => {
                    values.iter().any(|value| match value {
                        SessionAvailability::Live => record.live,
                        SessionAvailability::Persisted => record.persisted,
                    })
                }
            })
        })
        .cloned()
        .collect())
}

pub fn compile_session_text_filter(text: &str) -> Result<Regex> {
    let text = js_trim(text);
    if text.is_empty() {
        return Err(SessionQueryError::boxed(
            "session text filter must contain non-whitespace text",
            Code::InvalidFilter,
        ));
    }
    let mut parts = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if hoh_util_values::is_js_white_space(ch) {
            if let Some(begin) = start.take() {
                parts.push(regex::escape(&text[begin..index]));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(begin) = start {
        parts.push(regex::escape(&text[begin..]));
    }
    let pattern = parts.join(&format!("[{JS_WHITE_SPACE_SET}]+"));
    RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .unicode(true)
        .build()
        .map_err(|cause| {
            Box::new(SessionQueryError::with_cause(
                "session-query: invalid text filter",
                Code::InvalidFilter,
                cause,
            )) as BoxError
        })
}

pub fn filter_session_event_documents(
    documents: &[SessionEventSearchDocument],
    filters: &[SessionEventResultFilter],
) -> Result<Vec<SessionEventSearchDocument>> {
    let filters = materialize_session_event_result_filters(filters)?;
    let compiled = filters
        .iter()
        .map(|filter| match filter {
            SessionEventResultFilter::Text(text) => compile_session_text_filter(text).map(Some),
            _ => Ok(None),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(documents
        .iter()
        .filter(|document| {
            filters
                .iter()
                .zip(&compiled)
                .all(|(filter, regex)| match filter {
                    SessionEventResultFilter::Seq(range) => {
                        in_range(document.record.seq.get() as f64, *range)
                    }
                    SessionEventResultFilter::Time(range) => {
                        in_range(document.record.time as f64, *range)
                    }
                    SessionEventResultFilter::Type(values) => {
                        values.contains(&document.record.r#type)
                    }
                    SessionEventResultFilter::Surface(values) => {
                        values.contains(&document.record.surface)
                    }
                    SessionEventResultFilter::Text(_) => regex
                        .as_ref()
                        .is_some_and(|regex| regex.is_match(&document.text)),
                })
        })
        .cloned()
        .collect())
}

pub fn assert_session_headers_compatible(
    live: &SessionHeader,
    persisted: &SessionHeader,
) -> Result<()> {
    let same = live.id == persisted.id
        && live.created_at == persisted.created_at
        && live.cwd == persisted.cwd
        && live.parent_session == persisted.parent_session
        && live.is_seeded == persisted.is_seeded
        && live.delegation_depth.unwrap_or(0) == persisted.delegation_depth.unwrap_or(0);
    if same {
        Ok(())
    } else {
        Err(SessionQueryError::boxed(
            format!(
                "session source headers conflict for session \"{}\"",
                live.id
            ),
            Code::SourceConflict,
        ))
    }
}

fn source_seqs(event: &SessionEvent) -> Vec<SessionSeq> {
    event
        .source_event_seqs()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .filter_map(|value| SessionSeq::new(value).ok())
        .collect()
}

pub fn trace_event(
    session_id: &SessionId,
    events: &[Arc<SessionEvent>],
    target_seq: SessionSeq,
) -> Result<SessionEventTrace> {
    let Some(target) = events.get(target_seq.get() as usize) else {
        return Err(SessionQueryError::boxed(
            format!("session \"{session_id}\" has no event at seq {target_seq}"),
            Code::EventNotFound,
        ));
    };
    if target.seq() != target_seq {
        return Err(SessionQueryError::boxed(
            format!("session \"{session_id}\" has no event at seq {target_seq}"),
            Code::EventNotFound,
        ));
    }
    let fold = fold_surface(events, &projections()).map_err(|cause| {
        Box::new(SessionQueryError::with_cause(
            format!("invalid session surface: {cause}"),
            Code::InvalidSurface,
            cause,
        )) as BoxError
    })?;
    let records = build_session_event_records(session_id, events)?;
    let mut replacer = HashMap::new();
    let mut replaced = HashMap::new();
    for replacement in &fold.replacements {
        replaced.insert(replacement.seq, replacement.shadowed_seqs.clone());
        for seq in &replacement.shadowed_seqs {
            replacer.insert(*seq, replacement.seq);
        }
    }
    let replaced_by = replacer.get(&target_seq).copied();
    let mut replacement_chain = Vec::new();
    let mut current = replaced_by;
    while let Some(seq) = current {
        replacement_chain.push(seq);
        current = replacer.get(&seq).copied();
    }
    Ok(SessionEventTrace {
        target: records[target_seq.get() as usize].clone(),
        replaced_by,
        replacement_chain,
        replaced_event_seqs: replaced.remove(&target_seq).unwrap_or_default(),
        source_event_seqs: source_seqs(target),
        derived_event_seqs: events
            .iter()
            .filter(|event| event.seq() > target_seq && source_seqs(event).contains(&target_seq))
            .map(|event| event.seq())
            .collect(),
    })
}

fn locale_sort(records: &mut [SessionRecord]) {
    let mut collator = Collator::new(Tailoring::Cldr(Locale::Root), false, false);
    records.sort_by(|a, b| {
        a.header
            .created_at
            .cmp(&b.header.created_at)
            .then_with(|| collator.collate(a.header.id.as_ref(), b.header.id.as_ref()))
    });
}

pub(crate) fn sort_corpus(records: &mut [SessionRecord]) {
    let mut collator = Collator::new(Tailoring::Cldr(Locale::Root), false, false);
    records.sort_by(|a, b| {
        b.header
            .created_at
            .cmp(&a.header.created_at)
            .then_with(|| collator.collate(a.header.id.as_ref(), b.header.id.as_ref()))
    });
}

pub fn trace_session(
    records: &[SessionRecord],
    target_id: &SessionId,
) -> Result<SessionLineageTrace> {
    let by_id = records
        .iter()
        .cloned()
        .map(|record| (record.header.id.clone(), record))
        .collect::<HashMap<_, _>>();
    let Some(target) = by_id.get(target_id).cloned() else {
        return Err(SessionQueryError::boxed(
            format!("session \"{target_id}\" not found"),
            Code::SessionNotFound,
        ));
    };
    let mut ancestors = Vec::new();
    let mut seen = HashSet::from([target_id.clone()]);
    let mut parent = target.header.parent_session.clone();
    let completion = loop {
        let Some(parent_id) = parent else {
            let root = ancestors.last().cloned().unwrap_or_else(|| target.clone());
            break SessionLineageCompletion::Complete {
                root: Box::new(root),
            };
        };
        if !seen.insert(parent_id.clone()) {
            return Err(SessionQueryError::boxed(
                format!("session lineage contains a cycle at \"{parent_id}\""),
                Code::InvalidLineage,
            ));
        }
        let Some(record) = by_id.get(&parent_id).cloned() else {
            break SessionLineageCompletion::Incomplete {
                unresolved_parent_id: parent_id,
            };
        };
        parent = record.header.parent_session.clone();
        ancestors.push(record);
    };

    let mut children: HashMap<SessionId, Vec<SessionRecord>> = HashMap::new();
    for record in records {
        if let Some(parent_id) = &record.header.parent_session {
            children
                .entry(parent_id.clone())
                .or_default()
                .push(record.clone());
        }
    }
    for entries in children.values_mut() {
        locale_sort(entries);
    }

    struct Frame {
        record: SessionRecord,
        expanded: bool,
    }
    let mut stack = children
        .get(target_id)
        .into_iter()
        .flatten()
        .rev()
        .cloned()
        .map(|record| Frame {
            record,
            expanded: false,
        })
        .collect::<Vec<_>>();
    let mut built: HashMap<SessionId, SessionLineageNode> = HashMap::new();
    while let Some(mut frame) = stack.pop() {
        if !frame.expanded {
            frame.expanded = true;
            let id = frame.record.header.id.clone();
            stack.push(frame);
            if let Some(kids) = children.get(&id) {
                stack.extend(kids.iter().rev().cloned().map(|record| Frame {
                    record,
                    expanded: false,
                }));
            }
        } else {
            let descendants = children
                .get(&frame.record.header.id)
                .into_iter()
                .flatten()
                .filter_map(|child| built.remove(&child.header.id))
                .collect();
            built.insert(
                frame.record.header.id.clone(),
                SessionLineageNode {
                    session: frame.record,
                    descendants,
                },
            );
        }
    }
    let descendants = children
        .get(target_id)
        .into_iter()
        .flatten()
        .filter_map(|child| built.remove(&child.header.id))
        .collect();
    Ok(SessionLineageTrace {
        target,
        ancestors,
        descendants,
        completion,
    })
}
