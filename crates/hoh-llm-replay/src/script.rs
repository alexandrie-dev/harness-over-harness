use crate::{
    ReplayConfig, ReplayEntry, ReplayOverrideDoc, Result, SessionScript, error,
    fixture::{ParsedFixture, parse_fixture},
    json, read, shim,
};
use hoh_llm::{FinishReason, StreamChunk};
use hoh_session::SessionEvent;
use serde_json::{Value, json};
use std::path::Path;
pub fn derive_replay_script(events: &[SessionEvent]) -> Result<Vec<ReplayEntry>> {
    let mut script = vec![];
    for event in events {
        if event.event_type() == "compaction/summary" {
            let summary: shim::CompactionSummary = serde_json::from_value(event.data().clone())?;
            if summary.llm_stream_call == Some(true) {
                let blocks = summary.raw_output.ok_or_else(|| {
                    error(
                        "llm-replay: compaction/summary marks an LLM stream call without rawOutput",
                    )
                })?;
                let mut chunks = vec![];
                for (index, block) in blocks.into_iter().enumerate() {
                    chunks.push(StreamChunk::BlockStart {
                        index: index as u64,
                        block_type: block.r#type.clone(),
                        wire: Default::default(),
                    });
                    chunks.push(StreamChunk::BlockEnd {
                        index: index as u64,
                        block,
                        wire: Default::default(),
                    });
                }
                if let Some(usage) = summary.usage {
                    chunks.push(StreamChunk::Usage {
                        usage,
                        wire: Default::default(),
                    });
                }
                chunks.push(StreamChunk::finish(FinishReason::new("stop")));
                script.push(ReplayEntry::Chunks { chunks });
            }
            continue;
        }
        if !["assistant/message", "assistant/attempt"].contains(&event.event_type()) {
            continue;
        }
        let streams: Vec<hoh_llm::AssistantStreamRecord> =
            serde_json::from_value(event.data()["stream"].clone())?;
        let chunks: Vec<_> = hoh_llm::expand_assistant_stream(&streams)?
            .into_iter()
            .map(|m| m.chunk)
            .collect();
        if chunks.is_empty() {
            continue;
        }
        if !chunks.last().is_some_and(|c| c.kind() == "finish") {
            let field = |key| {
                event
                    .data()
                    .get(key)
                    .map(hoh_util_values::js_string)
                    .unwrap_or_else(|| "undefined".into())
            };
            return Err(error(format!(
                "llm-replay: model call {}/{} ended without a finish chunk (a thrown stream); this scenario needs a replay.override.json sidecar",
                field("turn"),
                field("step")
            )));
        }
        script.push(ReplayEntry::Chunks { chunks });
    }
    Ok(script)
}
pub fn resolve_scripted_entry<'a>(
    entry: &'a ReplayEntry,
    messages: &[hoh_llm::RequestMessage],
) -> Result<std::borrow::Cow<'a, ReplayEntry>> {
    let mut value = serde_json::to_value(entry)?;
    if !json(&value).contains("{{fromRequest:") {
        return Ok(std::borrow::Cow::Borrowed(entry));
    }
    let mut leaves = vec![];
    // JSON.stringify enumerates integer-named keys before the remaining insertion-ordered keys.
    let request = hoh_util_values::js_json_parse(&json(&serde_json::to_value(messages)?))?;
    collect_strings(&request, &mut leaves);
    let corpus = leaves.join("\n");
    map_strings(&mut value, &mut |text| substitute_string(text, &corpus))?;
    Ok(std::borrow::Cow::Owned(serde_json::from_value(value)?))
}
fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => {
            for v in a {
                collect_strings(v, out);
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                collect_strings(v, out);
            }
        }
        _ => {}
    }
}
pub(crate) fn map_strings(
    value: &mut Value,
    f: &mut impl FnMut(&str) -> Result<String>,
) -> Result<()> {
    match value {
        Value::String(s) => *s = f(s)?,
        Value::Array(a) => {
            for v in a {
                map_strings(v, f)?;
            }
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                map_strings(v, f)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn substitute_string(text: &str, corpus: &str) -> Result<String> {
    const OPEN: &str = "{{fromRequest:";
    let mut output = String::new();
    let mut cursor = 0;
    while let Some(open) = text[cursor..].find(OPEN).map(|n| cursor + n) {
        let start = open + OPEN.len();
        let mut close = text[start..].find("}}").map(|n| start + n).ok_or_else(|| {
            error(format!(
                "llm-replay: fromRequest placeholder is unterminated in {}",
                json(&json!(text))
            ))
        })?;
        while text.as_bytes().get(close + 2) == Some(&b'}') {
            close += 1;
        }
        let pattern = &text[start..close];
        let translated = js_pattern(pattern);
        let regex = regex::Regex::new(&translated).map_err(|e| {
            error(format!(
                "llm-replay: fromRequest has an invalid pattern {}: SyntaxError: {e}",
                json(&json!(pattern))
            ))
        })?;
        let capture = regex.captures_iter(corpus).last().ok_or_else(|| {
            error(format!(
                "llm-replay: fromRequest pattern {} matched nothing in the request",
                json(&json!(pattern))
            ))
        })?;
        // A successful capture always contains the full match.
        let replacement = capture
            .get(1)
            .or_else(|| capture.get(0))
            .expect("capture has full match")
            .as_str();
        output.push_str(&text[cursor..open]);
        output.push_str(replacement);
        cursor = close + 2;
    }
    output.push_str(&text[cursor..]);
    Ok(output)
}
// The permitted regex engine is scalar-based. Translate JS non-u ASCII classes and
// its four line terminators without rewriting escaped backslashes or literal dots.
fn js_pattern(pattern: &str) -> String {
    let mut output = String::new();
    let mut chars = pattern.chars();
    let mut class = false;
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let Some(escaped) = chars.next() else {
                output.push('\\');
                break;
            };
            match escaped {
                'd' => output.push_str("[0-9]"),
                'D' => output.push_str("[^0-9]"),
                'w' => output.push_str("[A-Za-z0-9_]"),
                'W' => output.push_str("[^A-Za-z0-9_]"),
                's' => output.push_str(&format!("[{}]", hoh_util_values::JS_WHITE_SPACE_SET)),
                'S' => output.push_str(&format!("[^{}]", hoh_util_values::JS_WHITE_SPACE_SET)),
                'b' if class => output.push_str(r"\x08"),
                'b' => output.push_str(r"(?-u:\b)"),
                'B' => output.push_str(r"(?-u:\B)"),
                _ => {
                    output.push('\\');
                    output.push(escaped);
                }
            }
        } else {
            match ch {
                '[' => class = true,
                ']' => class = false,
                '.' if !class => {
                    output.push_str(r"[^\n\r\u{2028}\u{2029}]");
                    continue;
                }
                _ => {}
            }
            output.push(ch);
        }
    }
    output
}
pub(crate) fn materialize_session_tokens(
    entry: &ReplayEntry,
    ids: &[Option<String>],
) -> Result<ReplayEntry> {
    let mut value = serde_json::to_value(entry)?;
    let regex = regex::Regex::new(r"\{\{session:([1-9][0-9]*)\}\}")?;
    map_strings(&mut value, &mut |text| {
        let mut result = String::new();
        let mut cursor = 0;
        for capture in regex.captures_iter(text) {
            // Every successful regex capture includes group zero.
            let matched = capture.get(0).expect("capture has full match");
            let live=capture[1].parse::<usize>().ok().and_then(|n|ids.get(n-1)).and_then(Option::as_deref).ok_or_else(||error(format!("llm-replay: session token {{{{session:{}}}}} was used before that recorded session bound",&capture[1])))?;
            result.push_str(&text[cursor..matched.start()]);
            result.push_str(live);
            cursor = matched.end();
        }
        result.push_str(&text[cursor..]);
        Ok(result)
    })?;
    serde_json::from_value(value).map_err(Into::into)
}
fn primary(config: &ReplayConfig) -> Result<Option<ParsedFixture>> {
    if !Path::new(&config.file).exists()
        || config.override_file.as_deref() == Some(config.file.as_str())
    {
        return Ok(None);
    }
    Ok(Some(parse_fixture(&read(&config.file)?)?))
}
fn derive(config: &ReplayConfig, fixture: Option<&ParsedFixture>) -> Result<Vec<ReplayEntry>> {
    let fixture = fixture.ok_or_else(|| {
        error(format!(
            "llm-replay: fixture not found: {} — run `pnpm run test:snapshot:record` first",
            config.file
        ))
    })?;
    derive_replay_script(&fixture.artifact.events)
}
fn resolve(config: &ReplayConfig, fixture: Option<&ParsedFixture>) -> Result<Vec<ReplayEntry>> {
    if let Some(file) = config
        .override_file
        .as_deref()
        .filter(|f| Path::new(f).exists())
    {
        let value = hoh_util_values::js_json_parse(&read(file)?)?;
        let doc = read_override(value, file)?;
        if let ReplayOverrideDoc::Replace(entries) = doc {
            return Ok(entries);
        }
        let ReplayOverrideDoc::Patches { patches } = doc else {
            return Err(error("invalid override document"));
        };
        let mut script = derive(config, fixture)?;
        let length = script.len();
        let mut seen = std::collections::HashSet::new();
        for patch in patches {
            if patch.at > length {
                return Err(error(format!(
                    "llm-replay: override patch index {} out of range (derived script has {length} call(s); == length appends): {file}",
                    patch.at
                )));
            }
            if !seen.insert(patch.at) {
                return Err(error(format!(
                    "llm-replay: duplicate override patch index {}: {file}",
                    patch.at
                )));
            }
            if patch.at == script.len() {
                script.push(patch.entry);
            } else {
                script[patch.at] = patch.entry;
            }
        }
        return Ok(script);
    }
    derive(config, fixture)
}
pub fn load_replay_script(config: &ReplayConfig) -> Result<Vec<ReplayEntry>> {
    let fixture = primary(config)?;
    resolve(config, fixture.as_ref())
}
pub fn load_session_scripts(config: &ReplayConfig) -> Result<Vec<SessionScript>> {
    let fixture = primary(config)?;
    let entries = resolve(config, fixture.as_ref())?;
    let mut result = vec![SessionScript {
        recorded_id: fixture
            .as_ref()
            .and_then(|f| f.artifact.header["id"].as_str())
            .unwrap_or("")
            .into(),
        created_at: fixture
            .as_ref()
            .and_then(|f| f.artifact.header["createdAt"].as_i64())
            .unwrap_or(0),
        entries,
        primary: true,
    }];
    let mut children = vec![];
    for file in config.child_files.iter().flatten() {
        if !Path::new(file).exists() {
            return Err(error(format!(
                "llm-replay: child fixture not found: {file} — re-record the scenario"
            )));
        }
        let fixture = parse_fixture(&read(file)?)?;
        let start = usize::try_from(fixture.artifact.inherited_event_count.get())?;
        children.push(SessionScript {
            recorded_id: fixture.artifact.header["id"].as_str().unwrap_or("").into(),
            created_at: fixture.artifact.header["createdAt"].as_i64().unwrap_or(0),
            entries: derive_replay_script(&fixture.artifact.events[start..])?,
            primary: false,
        });
    }
    // Node's localeCompare under the source en-US locale: CLDR root, non-ignorable.
    let mut collator =
        feruca::Collator::new(feruca::Tailoring::Cldr(feruca::Locale::Root), false, false);
    children.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| collator.collate(a.recorded_id.as_str(), b.recorded_id.as_str()))
    });
    result.extend(children);
    Ok(result)
}
fn read_override(value: Value, file: &str) -> Result<ReplayOverrideDoc> {
    fn exact(v: &Value, keys: &[&str]) -> bool {
        v.as_object()
            .is_some_and(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)))
    }
    fn entry(v: &Value, file: &str, location: &str) -> Result<ReplayEntry> {
        let fail = |detail: &str| {
            error(format!(
                "llm-replay: invalid override {file}: {location} {detail}"
            ))
        };
        if !v.is_object() {
            return Err(fail("must be an object"));
        }
        let kind = v["kind"].as_str().unwrap_or("");
        let keys: Vec<_> = match kind {
            "chunks" => vec!["kind", "chunks"],
            "throw" => {
                let mut k = vec!["kind", "chunks", "message", "code"];
                if v.get("accepted").is_some() {
                    k.push("accepted");
                }
                k
            }
            "hang" => {
                if v.get("readyFile").is_some() {
                    vec!["kind", "readyFile"]
                } else {
                    vec!["kind"]
                }
            }
            _ => {
                return Err(fail(&format!(
                    "has unknown kind {}",
                    v.get("kind")
                        .map(json)
                        .unwrap_or_else(|| "undefined".into())
                )));
            }
        };
        if !exact(v, &keys) {
            return Err(fail(&format!("has invalid {kind}-entry fields")));
        }
        if kind == "hang" {
            if v.get("readyFile")
                .is_some_and(|v| !v.as_str().is_some_and(|s| !s.is_empty()))
            {
                return Err(fail("readyFile must be a non-empty string"));
            }
        } else {
            let chunks = v["chunks"]
                .as_array()
                .ok_or_else(|| fail("chunks must be an array"))?;
            for (i, c) in chunks.iter().enumerate() {
                if ![
                    "block-start",
                    "text-delta",
                    "reasoning-delta",
                    "tool-call-delta",
                    "block-end",
                    "usage",
                    "finish",
                ]
                .contains(&c["type"].as_str().unwrap_or(""))
                {
                    return Err(error(format!(
                        "llm-replay: invalid override {file}: {location}.chunks[{i}] must have a known StreamChunk type"
                    )));
                }
            }
            if kind == "throw" {
                for field in ["message", "code"] {
                    if !v[field].as_str().is_some_and(|s| !s.is_empty()) {
                        return Err(fail(&format!("{field} must be a non-empty string")));
                    }
                }
                if v.get("accepted").is_some_and(|v| !v.is_boolean()) {
                    return Err(fail("accepted must be a boolean"));
                }
            }
        }
        // readReplayEntry rebuilds the envelope in this order, while chunks remain verbatim.
        let value = Value::Object(
            keys.into_iter()
                .map(|key| (key.to_owned(), v[key].clone()))
                .collect(),
        );
        serde_json::from_value(value).map_err(|e| fail(&e.to_string()))
    }
    if let Some(entries) = value.as_array() {
        return Ok(ReplayOverrideDoc::Replace(
            entries
                .iter()
                .enumerate()
                .map(|(i, v)| entry(v, file, &format!("entry {i}")))
                .collect::<Result<_>>()?,
        ));
    }
    if !exact(&value, &["patches"]) || !value["patches"].is_array() {
        return Err(error(format!(
            "llm-replay: invalid override {file}: document must be a ReplayEntry[] or {{ patches: [...] }}"
        )));
    }
    let mut patches = vec![];
    // Array form was checked immediately above.
    for (i, p) in value["patches"]
        .as_array()
        .expect("patch array checked")
        .iter()
        .enumerate()
    {
        if !exact(p, &["at", "entry"]) {
            return Err(error(format!(
                "llm-replay: invalid override {file}: patch {i} must contain exactly at and entry"
            )));
        }
        let at=p["at"].as_u64().filter(|n|*n<=9_007_199_254_740_991).ok_or_else(||error(format!("llm-replay: invalid override {file}: patch {i} at must be a non-negative safe integer")))?;
        patches.push(crate::ReplayOverridePatch {
            at: usize::try_from(at)?,
            entry: entry(&p["entry"], file, &format!("patch {i}.entry"))?,
        });
    }
    Ok(ReplayOverrideDoc::Patches { patches })
}
