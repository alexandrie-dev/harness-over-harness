use hoh_session::SessionEvent;
use hoh_util_values::{is_js_white_space, js_json_stringify_pretty, js_trim};
use serde_json::Value;
use std::{borrow::Borrow, collections::HashMap};

const UNTITLED_SESSION_MARKDOWN_TITLE: &str = "Untitled Session";

#[derive(Clone)]
enum Segment {
    User(String),
    Assistant { text: String, tool_calls: bool },
    Tool(ToolEntry),
}

#[derive(Clone)]
struct ToolEntry {
    name: String,
    arguments: String,
    result: Option<ToolResult>,
}

#[derive(Clone)]
struct ToolResult {
    text: String,
    is_error: bool,
    reason: Option<String>,
}

#[derive(Default)]
struct Turn {
    segments: Vec<Segment>,
    calls: HashMap<String, usize>,
}

/// Render a complete committed logical event snapshot as Markdown.
pub fn format_session_markdown(
    events: &[impl Borrow<SessionEvent>],
    source_path: Option<&str>,
) -> String {
    let title = events
        .iter()
        .rev()
        .find(|event| <_ as Borrow<SessionEvent>>::borrow(*event).event_type() == "session/title")
        .and_then(|event| {
            <_ as Borrow<SessionEvent>>::borrow(event)
                .data()
                .get("title")
        })
        .and_then(Value::as_str);
    let mut sections = vec![format!("# {}", normalized_title(title))];
    if let Some(path) = source_path.filter(|path| !path.is_empty()) {
        sections.push(format!("Session file:\n\n{}", fenced(path, "text", false)));
    }
    for turn in collect_turns(events) {
        let final_index = final_reply_index(&turn.segments);
        let end = final_index.unwrap_or(turn.segments.len());
        let mut pending = Vec::new();
        for segment in &turn.segments[..end] {
            match segment {
                Segment::User(text) => {
                    if !pending.is_empty() {
                        sections.push(collapsible_section(&pending));
                        pending.clear();
                    }
                    sections.push(quote(text));
                }
                Segment::Assistant { .. } | Segment::Tool(_) => pending.push(segment.clone()),
            }
        }
        if !pending.is_empty() {
            sections.push(collapsible_section(&pending));
        }
        if let Some(index) = final_index
            && let Segment::Assistant { text, .. } = &turn.segments[index]
        {
            let text = close_unfinished_fence(&trim_blank_edge_lines(text));
            if !text.is_empty() {
                sections.push(text);
            }
        }
    }
    format!("{}\n", sections.join("\n\n").trim_end())
}

fn normalized_title(title: Option<&str>) -> String {
    let title = title
        .unwrap_or("")
        .replace("\r\n", " ")
        .replace(['\r', '\n'], " ");
    let title = js_trim(&title);
    if title.is_empty() {
        UNTITLED_SESSION_MARKDOWN_TITLE.to_owned()
    } else {
        title.to_owned()
    }
}

fn collect_turns(events: &[impl Borrow<SessionEvent>]) -> Vec<Turn> {
    let mut turns = Vec::<Turn>::new();
    let mut current = None;
    for event in events {
        let event = <_ as Borrow<SessionEvent>>::borrow(event);
        match event.event_type() {
            "turn/start" => {
                turns.push(Turn::default());
                current = Some(turns.len() - 1);
            }
            "turn/end" => current = None,
            "user/message" => {
                if event.surface_op().and_then(Value::as_str) != Some("append")
                    || event.data().pointer("/source/kind").and_then(Value::as_str) != Some("user")
                {
                    continue;
                }
                let text = visible_content(event.data().get("content"));
                if !text.is_empty() {
                    let index = active_turn(&mut turns, &mut current);
                    turns[index].segments.push(Segment::User(text));
                }
            }
            "assistant/message" => {
                if event.surface_op().and_then(Value::as_str) != Some("append") {
                    continue;
                }
                let content = event.data().pointer("/message/content");
                let text = visible_content(content);
                let tool_calls = content.and_then(Value::as_array).is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool-call"))
                });
                if !trim_blank_edge_lines(&text).is_empty() {
                    let index = active_turn(&mut turns, &mut current);
                    turns[index]
                        .segments
                        .push(Segment::Assistant { text, tool_calls });
                }
            }
            "tool/call" => {
                let index = active_turn(&mut turns, &mut current);
                let call_id = js_value_string(event.data().get("callId"));
                let position = turns[index].segments.len();
                turns[index].segments.push(Segment::Tool(ToolEntry {
                    name: event
                        .data()
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    arguments: event
                        .data()
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    result: None,
                }));
                turns[index].calls.insert(call_id, position);
            }
            "tool/result" => {
                if event.surface_op().and_then(Value::as_str) != Some("append") {
                    continue;
                }
                let index = active_turn(&mut turns, &mut current);
                let call_id = js_value_string(event.data().pointer("/message/toolCallId"));
                let position = turns[index]
                    .calls
                    .get(&call_id)
                    .copied()
                    .unwrap_or_else(|| {
                        let position = turns[index].segments.len();
                        turns[index].segments.push(Segment::Tool(ToolEntry {
                            name: "unknown".to_owned(),
                            arguments: String::new(),
                            result: None,
                        }));
                        turns[index].calls.insert(call_id, position);
                        position
                    });
                if let Some(Segment::Tool(entry)) = turns[index].segments.get_mut(position) {
                    entry.result = Some(ToolResult {
                        text: visible_content(event.data().pointer("/message/content")),
                        is_error: event
                            .data()
                            .pointer("/message/isError")
                            .and_then(Value::as_bool)
                            == Some(true),
                        reason: event
                            .data()
                            .pointer("/error/reason")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    });
                }
            }
            _ => {}
        }
    }
    turns.retain(|turn| !turn.segments.is_empty());
    turns
}

fn active_turn(turns: &mut Vec<Turn>, current: &mut Option<usize>) -> usize {
    if let Some(index) = *current {
        index
    } else {
        turns.push(Turn::default());
        let index = turns.len() - 1;
        *current = Some(index);
        index
    }
}

fn final_reply_index(segments: &[Segment]) -> Option<usize> {
    let index = segments.len().checked_sub(1)?;
    matches!(
        segments[index],
        Segment::Assistant {
            tool_calls: false,
            ..
        }
    )
    .then_some(index)
}

fn collapsible_section(entries: &[Segment]) -> String {
    let summary = if entries.len() == 1 {
        "1 previous message".to_owned()
    } else {
        format!("{} previous messages", entries.len())
    };
    let body = entries
        .iter()
        .map(render_entry)
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("<details><summary>{summary}</summary>\n\n{body}\n\n</details>")
}

fn render_entry(entry: &Segment) -> String {
    match entry {
        Segment::Assistant { text, .. } => {
            let text = escape_wrapper_tags_outside_fences(&close_unfinished_fence(
                &trim_blank_edge_lines(text),
            ));
            format!("**Assistant**\n\n{text}")
        }
        Segment::Tool(entry) => {
            let heading = format!("**Tool: `{}`**", escape_wrapper_tags(&entry.name));
            let arguments = if entry.arguments.is_empty() {
                "(no arguments)".to_owned()
            } else {
                fenced(&json_arguments(&entry.arguments), "json", true)
            };
            let result_heading = if entry.result.as_ref().is_some_and(|result| result.is_error) {
                "**Result (error)**"
            } else {
                "**Result**"
            };
            let result_body = match &entry.result {
                None => "(no result recorded)".to_owned(),
                Some(result)
                    if result
                        .reason
                        .as_deref()
                        .is_some_and(|reason| !reason.is_empty()) =>
                {
                    let reason = escape_wrapper_tags(result.reason.as_deref().unwrap_or_default());
                    if result.text.is_empty() {
                        format!("Error: {reason}")
                    } else {
                        format!("Error: {reason}\n\n{}", fenced(&result.text, "", true))
                    }
                }
                Some(result) if result.text.is_empty() => "(no output)".to_owned(),
                Some(result) => fenced(&result.text, "", true),
            };
            format!("{heading}\n\n{arguments}\n\n{result_heading}\n\n{result_body}")
        }
        Segment::User(_) => unreachable!("user entries are never rendered as commentary"),
    }
}

fn visible_content(content: Option<&Value>) -> String {
    let Some(blocks) = content.and_then(Value::as_array) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    parts.push(text.to_owned());
                }
            }
            Some("image") => parts.push(image_label(block.get("attachment"))),
            Some("file") => parts.push(file_label(block.get("attachment"))),
            _ => {}
        }
    }
    parts.join("\n\n")
}

fn image_label(reference: Option<&Value>) -> String {
    let name = label_name(
        reference
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str),
    );
    let width = js_value_string(reference.and_then(|value| value.get("width")));
    let height = js_value_string(reference.and_then(|value| value.get("height")));
    let bytes = js_value_string(reference.and_then(|value| value.get("bytes")));
    format!("[Image: {name} ({width}×{height}, {bytes} bytes)]")
}

fn file_label(reference: Option<&Value>) -> String {
    let name = label_name(
        reference
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str),
    );
    let bytes = js_value_string(reference.and_then(|value| value.get("bytes")));
    format!("[File: {name} ({bytes} bytes)]")
}

fn label_name(name: Option<&str>) -> String {
    let mut result = String::new();
    let mut separating = false;
    for character in name.unwrap_or("").chars() {
        if is_js_white_space(character) {
            separating = !result.is_empty();
        } else {
            if separating {
                result.push(' ');
                separating = false;
            }
            if character == ']' {
                result.push('\\');
            }
            result.push(character);
        }
    }
    if result.is_empty() {
        "attachment".to_owned()
    } else {
        result
    }
}

fn quote(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .trim_end_matches('\n')
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_arguments(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .map(|value| js_json_stringify_pretty(&value, 2))
        .unwrap_or_else(|| text.to_owned())
}

fn fenced(text: &str, language: &str, trim_trailing: bool) -> String {
    let content = if trim_trailing {
        text.trim_end_matches('\n')
    } else {
        text
    };
    let longest = content
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(3.max(longest + 1));
    format!("{fence}{language}\n{content}\n{fence}")
}

fn trim_blank_edge_lines(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let start = lines
        .iter()
        .position(|line| !js_trim(line).is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !js_trim(line).is_empty())
        .map_or(start, |index| index + 1);
    lines[start..end].join("\n")
}

#[derive(Clone, Copy)]
struct Fence {
    character: char,
    length: usize,
}

fn opening_fence(line: &str) -> Option<Fence> {
    let rest = line
        .strip_prefix("   ")
        .or_else(|| line.strip_prefix("  "))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line);
    let character = rest.chars().next()?;
    if character != '`' && character != '~' {
        return None;
    }
    let length = rest.chars().take_while(|value| *value == character).count();
    if length < 3 {
        return None;
    }
    let info = &rest[character.len_utf8() * length..];
    if info
        .chars()
        .any(|value| matches!(value, '\u{2028}' | '\u{2029}'))
    {
        return None;
    }
    if character == '`' && info.contains('`') {
        return None;
    }
    Some(Fence { character, length })
}

fn closes_fence(line: &str, fence: Fence) -> bool {
    let rest = line
        .strip_prefix("   ")
        .or_else(|| line.strip_prefix("  "))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line);
    let length = rest
        .chars()
        .take_while(|value| *value == fence.character)
        .count();
    length >= fence.length && rest.chars().skip(length).all(is_js_white_space)
}

fn scan_markdown(markdown: &str) -> (Vec<(String, bool)>, Option<Fence>) {
    let normalized = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let mut scanned = Vec::new();
    let mut fence = None;
    let mut indented = false;
    for line in normalized.split('\n') {
        if let Some(open) = fence {
            scanned.push((line.to_owned(), true));
            if closes_fence(line, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = opening_fence(line) {
            fence = Some(open);
            indented = false;
            scanned.push((line.to_owned(), true));
            continue;
        }
        if line.starts_with("    ") || line.starts_with('\t') {
            indented = true;
            scanned.push((line.to_owned(), true));
            continue;
        }
        let blank = js_trim(line).is_empty();
        scanned.push((line.to_owned(), indented && blank));
        if !blank {
            indented = false;
        }
    }
    (scanned, fence)
}

fn close_unfinished_fence(markdown: &str) -> String {
    let (_, open) = scan_markdown(markdown);
    let Some(open) = open else {
        return markdown.to_owned();
    };
    format!(
        "{}\n{}",
        markdown.trim_end_matches('\n'),
        open.character.to_string().repeat(open.length)
    )
}

fn escape_wrapper_tags_outside_fences(markdown: &str) -> String {
    scan_markdown(markdown)
        .0
        .into_iter()
        .map(|(line, code)| {
            if code {
                line
            } else {
                escape_wrapper_tags(&line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_wrapper_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut offset = 0;
    while let Some(relative) = text[offset..].find('<') {
        let index = offset + relative;
        result.push_str(&text[offset..index]);
        let tail = &text[index + 1..];
        let name = tail.strip_prefix('/').unwrap_or(tail);
        let matched = ["details", "summary"].iter().find(|candidate| {
            name.get(..candidate.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(candidate))
                && name[candidate.len()..]
                    .chars()
                    .next()
                    .is_some_and(|character| {
                        is_js_white_space(character) || matches!(character, '/' | '>')
                    })
        });
        if matched.is_some() {
            result.push_str("&lt;");
        } else {
            result.push('<');
        }
        offset = index + 1;
    }
    result.push_str(&text[offset..]);
    result
}

fn js_value_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Null) | None => "undefined".to_owned(),
        Some(value) => value.to_string(),
    }
}
