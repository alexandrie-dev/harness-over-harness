use crate::{
    Result,
    corpus::{self, Scenario},
    normalize::{Context, Logs, Normalizer},
    read, records,
};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct Failure {
    pub phase: &'static str,
    pub reason: String,
    pub diff: String,
}
pub type Check = std::result::Result<(), Failure>;
impl Failure {
    pub fn new(phase: &'static str, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            phase,
            diff: reason.clone(),
            reason,
        }
    }
}
pub fn equal(phase: &'static str, label: &str, expected: &Value, actual: &Value) -> Check {
    if hoh_util_values::deep_equal_json(expected, actual) {
        return Ok(());
    }
    let e = hoh_util_values::js_json_stringify_pretty(expected, 2) + "\n";
    let a = hoh_util_values::js_json_stringify_pretty(actual, 2) + "\n";
    let options = hoh_jsdiff::PatchOptions {
        timeout_ms: Some(2000),
        ..Default::default()
    };
    let diff =
        hoh_jsdiff::create_two_files_patch("expected", "actual", &e, &a, None, None, &options)
            .unwrap_or_else(|| format!("EXPECTED\n{e}\nACTUAL\n{a}"));
    Err(Failure {
        phase,
        reason: format!("{label} mismatch"),
        diff,
    })
}
pub fn logs(n: &Normalizer, expected: &[String], actual: &[String], ctx: &Context) -> Check {
    if expected.len() != actual.len() {
        return Err(Failure::new(
            "log",
            format!(
                "persisted session count: expected {}, got {}",
                expected.len(),
                actual.len()
            ),
        ));
    }
    let expected = n
        .logs(
            expected,
            &Context {
                cwd: "{{cwd}}".into(),
                ..Default::default()
            },
        )
        .map_err(|e| Failure::new("log", e))?;
    let actual = n.logs(actual, ctx).map_err(|e| Failure::new("log", e))?;
    for (i, (e, a)) in expected.into_iter().zip(actual).enumerate() {
        if let Err(mut failure) = equal("log", &format!("session role {i}"), &json!(e), &json!(a)) {
            failure.reason = if e.len() != a.len() {
                let extra = a
                    .get(e.len())
                    .map(|v| format!("; extra type {}", v["type"]))
                    .unwrap_or_default();
                format!(
                    "session role {i} row count: expected {}, got {}{extra}",
                    e.len(),
                    a.len()
                )
            } else {
                let row = e
                    .iter()
                    .zip(&a)
                    .position(|(e, a)| !hoh_util_values::deep_equal_json(e, a))
                    .expect("unequal logs contain an unequal row");
                let types = format!(
                    "session role {i} row {row}: expected type {}, got {}",
                    e[row]["type"], a[row]["type"]
                );
                if e[row]["type"] != a[row]["type"] {
                    format!("{types}; event type/order mismatch")
                } else {
                    let (path, before, after) = first_difference(&e[row], &a[row], String::new())
                        .expect("unequal row contains a difference");
                    format!(
                        "{types}; value at {path}: expected {}, got {}",
                        hoh_util_values::js_json_stringify(&before),
                        hoh_util_values::js_json_stringify(&after)
                    )
                }
            };
            return Err(failure);
        }
    }
    Ok(())
}

fn first_difference(
    expected: &Value,
    actual: &Value,
    path: String,
) -> Option<(String, Value, Value)> {
    if hoh_util_values::deep_equal_json(expected, actual) {
        return None;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for key in e.keys().chain(a.keys().filter(|k| !e.contains_key(*k))) {
                let child = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                if let Some(difference) = first_difference(
                    e.get(key).unwrap_or(&Value::Null),
                    a.get(key).unwrap_or(&Value::Null),
                    child.clone(),
                ) {
                    return Some(difference);
                }
                if e.contains_key(key) != a.contains_key(key) {
                    return Some((
                        child,
                        e.get(key).cloned().unwrap_or(Value::Null),
                        a.get(key).cloned().unwrap_or(Value::Null),
                    ));
                }
            }
        }
        (Value::Array(e), Value::Array(a)) if e.len() == a.len() => {
            for (i, (e, a)) in e.iter().zip(a).enumerate() {
                if let Some(difference) = first_difference(e, a, format!("{path}/{i}")) {
                    return Some(difference);
                }
            }
        }
        _ => {}
    }
    Some((path, expected.clone(), actual.clone()))
}

pub fn catalog_creation_times(raw: &[String]) -> Check {
    let rows = raw
        .iter()
        .map(|s| records(s).map_err(|e| Failure::new("log", e)))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let headers: std::collections::HashMap<_, _> = rows
        .iter()
        .filter_map(|r| r.first())
        .filter_map(|h| Some((h["id"].as_str()?, &h["createdAt"])))
        .collect();
    for row in rows
        .iter()
        .flat_map(|r| r.iter())
        .filter(|r| r["type"] == "subagent/catalog")
    {
        if let Some(created) = row["data"]["childId"]
            .as_str()
            .and_then(|id| headers.get(id))
            && row["data"]["childCreatedAt"] != **created
        {
            return Err(Failure::new(
                "log",
                "catalog child creation time disagrees with child header",
            ));
        }
    }
    Ok(())
}

/// headless.snapshot.ts:199–205; sdk.snapshot.ts:362–367. Check before comparison removes version.
pub fn current_writer_generation(raw: &[String]) -> Check {
    for (role, log) in raw.iter().enumerate() {
        let rows = records(log).map_err(|e| Failure::new("log", e))?;
        let version = rows.first().and_then(|h| h["version"].as_u64());
        if version != Some(4) {
            return Err(Failure::new(
                "log",
                format!("current writer generation: role {role} wrote {version:?}; expected v4"),
            ));
        }
    }
    Ok(())
}

/// sdk.snapshot.ts:533–541 applies independently to actual and expected role sets.
pub fn sdk_wire_generation(raw: &[String], side: &str) -> std::result::Result<u64, Failure> {
    let mut versions = std::collections::BTreeSet::new();
    for log in raw {
        let rows = records(log).map_err(|e| Failure::new("frames", e))?;
        let version = rows
            .first()
            .and_then(|h| h["version"].as_u64())
            .ok_or_else(|| Failure::new("frames", "SDK wire role lacks a declared generation"))?;
        versions.insert(version);
    }
    if versions.len() != 1 {
        return Err(Failure::new(
            "frames",
            format!("SDK wire goldens require one common writer generation ({side}): {versions:?}"),
        ));
    }
    // A singleton set was checked above.
    Ok(*versions.first().expect("one generation"))
}

pub fn context(logs: &[String], cwd: &str) -> Result<Context> {
    let mut ids = Vec::new();
    for log in logs {
        let rows = records(log)?;
        if let Some(id) = rows.first().and_then(|h| h["id"].as_str()) {
            ids.push(id.into());
        }
    }
    let mut aliases = Vec::new();
    if let Ok(real) = std::fs::canonicalize(cwd) {
        aliases.push(real.to_string_lossy().into_owned());
    }
    Ok(Context {
        cwd: cwd.into(),
        aliases,
        session_ids: ids,
    })
}

fn schema_sets(file: &std::path::Path) -> Result<Vec<Value>> {
    let v = hoh_util_values::js_json_parse(&read(file)?).map_err(|e| e.to_string())?;
    if !v["initial"].is_array()
        || !v["changes"].is_array()
        || v["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|s| !s.is_array())
    {
        return Err("invalid tool-schema satellite".into());
    }
    let mut sets = vec![v["initial"].clone()];
    sets.extend(v["changes"].as_array().into_iter().flatten().cloned());
    Ok(sets)
}
pub fn prompt_parts(snapshot: &str) -> Result<Vec<String>> {
    let re = regex::Regex::new(r"\n<!-- system/message change [1-9][0-9]* -->\n\n")
        .map_err(|e| e.to_string())?;
    Ok(re.split(snapshot).map(str::to_owned).collect())
}
pub fn format_prompts(prompts: &[String]) -> String {
    let mut out = String::new();
    for (i, prompt) in prompts.iter().enumerate() {
        if i > 0 {
            out.push_str(&format!("\n<!-- system/message change {i} -->\n\n"));
        }
        out.push_str(prompt);
        if !prompt.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}
fn header_values(n: &Normalizer, log: &str, ctx: &Context) -> Result<Vec<Value>> {
    let rows = n.legacy_log(log, ctx)?;
    Ok(rows
        .into_iter()
        .filter(|r| r["type"] == "request/header")
        .map(|r| r["data"]["header"].clone())
        .collect())
}
fn satellite(
    all: &[Scenario],
    s: &Scenario,
    index: usize,
    prompt: bool,
) -> Result<std::path::PathBuf> {
    let field = if prompt {
        "childSystemPrompts"
    } else {
        "childToolSchemas"
    };
    let child = s.manifest["header"][field]
        .as_array()
        .is_some_and(|a| a.contains(&json!(index)));
    let base = if prompt {
        "system-prompt"
    } else {
        "tool-schemas"
    };
    let extension = if prompt { "md" } else { "json" };
    if index > 0 && child {
        return Ok(s.dir.join(format!("{base}.{index}.expected.{extension}")));
    }
    let pin = corpus::pin(all, s)?;
    let owner = corpus::source(
        all,
        pin,
        if prompt {
            "systemPromptSource"
        } else {
            "toolSchemasSource"
        },
    )?;
    Ok(owner.dir.join(format!("{base}.expected.{extension}")))
}

/// Restore satellite bulk into recorded rows for the fake writer, never used as real execution.
pub fn hydrate(all: &[Scenario], s: &Scenario, raw: &[String]) -> Result<Logs> {
    let mut result = raw.iter().map(|s| records(s)).collect::<Result<Logs>>()?;
    for (index, rows) in result.iter_mut().enumerate() {
        if !rows
            .iter()
            .any(|r| r["type"] == "request/header" || r["type"] == "system/message")
        {
            continue;
        }
        let prompts = prompt_parts(&read(&satellite(all, s, index, true)?)?)?;
        let schemas = schema_sets(&satellite(all, s, index, false)?)?;
        let mut pi = 0;
        let mut hi = 0;
        for row in rows {
            if row["type"] == "system/message"
                && let Some(text) = row.pointer_mut("/data/message/content/0/text")
            {
                if text == "{{system}}" {
                    *text = json!(
                        prompts
                            .get(pi)
                            .or_else(|| prompts.first())
                            .ok_or("empty prompt satellite")?
                    );
                }
                pi += 1;
            }
            if row["type"] == "request/header" {
                if let Some(tools) = row.pointer_mut("/data/header/tools")
                    && (tools == "{{tools}}"
                        || tools
                            .as_array()
                            .is_some_and(|a| a.iter().all(Value::is_string)))
                {
                    *tools = schemas
                        .get(hi)
                        .or_else(|| schemas.first())
                        .ok_or("empty tool satellite")?
                        .clone();
                }
                hi += 1;
            }
        }
    }
    Ok(result)
}

pub fn headers(
    n: &Normalizer,
    all: &[Scenario],
    s: &Scenario,
    actual: &[String],
    ctx: &Context,
) -> Check {
    let perform = || -> std::result::Result<(), Failure> {
        let pin = corpus::pin(all, s).map_err(|e| Failure::new("headers", e))?;
        let pin_files = pin.fixtures().map_err(|e| Failure::new("headers", e))?;
        let pinned = read(&pin_files[0]).map_err(|e| Failure::new("headers", e))?;
        let pin_context = context(std::slice::from_ref(&pinned), "{{cwd}}")
            .map_err(|e| Failure::new("headers", e))?;
        let mut base = if s.suite == "acp" {
            n.legacy_log(&pinned, &pin_context)
                .map_err(|e| Failure::new("headers", e))?
                .into_iter()
                .filter(|r| r["type"] == "request/header" && r["data"]["reason"] != "series")
                .map(|r| r["data"]["header"].clone())
                .collect()
        } else {
            header_values(n, &pinned, &pin_context).map_err(|e| Failure::new("headers", e))?
        };
        if base.is_empty() && actual.is_empty() {
            return Ok(());
        }
        let schema_file = satellite(all, s, 0, false).map_err(|e| Failure::new("headers", e))?;
        let schemas = schema_sets(&schema_file).map_err(|e| Failure::new("headers", e))?;
        for (i, h) in base.iter_mut().enumerate() {
            let tools = schemas
                .get(i)
                .ok_or_else(|| Failure::new("headers", "pin tool-schema count mismatch"))?;
            if h.get("tools").is_some() {
                h["tools"] = tools.clone();
            }
        }
        for (index, log) in actual.iter().enumerate() {
            let rows = n
                .legacy_log(log, ctx)
                .map_err(|e| Failure::new("headers", e))?;
            let first_header = rows.iter().position(|r| r["type"] == "request/header");
            let first_prompt = rows.iter().position(|r| r["type"] == "system/message");
            if first_header.is_some_and(|h| first_prompt.is_none_or(|p| p >= h)) {
                return Err(Failure::new("headers", "systemPromptPrecedesRequests"));
            }
            let prompts: Vec<String> = rows
                .iter()
                .filter(|r| r["type"] == "system/message")
                .filter_map(|r| {
                    let blocks = r.pointer("/data/message/content")?.as_array()?;
                    if blocks.is_empty() {
                        Some(String::new())
                    } else {
                        blocks[0]["text"].as_str().map(str::to_owned)
                    }
                })
                .collect();
            let headers: Vec<Value> = rows
                .iter()
                .filter(|r| r["type"] == "request/header")
                .map(|r| r["data"]["header"].clone())
                .collect();
            if !headers.is_empty()
                && prompts.len()
                    != 1 + if index == 0 {
                        pin.manifest["header"]["promptChanges"]
                            .as_u64()
                            .unwrap_or(0) as usize
                    } else {
                        0
                    }
            {
                return Err(Failure::new("headers", "system/message count mismatch"));
            }
            if s.suite == "acp" {
                let expected_changes = if index == 0 && s.manifest["header"]["pin"] == true {
                    s.manifest["header"]["changes"].as_u64().unwrap_or(0)
                } else {
                    0
                };
                let changed = rows
                    .iter()
                    .filter(|r| r["type"] == "request/header" && r["data"]["reason"] == "change")
                    .count() as u64;
                if changed != expected_changes {
                    return Err(Failure::new(
                        "headers",
                        "changed request/header count mismatch",
                    ));
                }
                if headers.iter().any(|h| !h["tools"].is_array()) {
                    return Err(Failure::new(
                        "headers",
                        "every ACP request/header must carry array-valued tools",
                    ));
                }
            }
            if !prompts.is_empty() {
                let expected =
                    read(&satellite(all, s, index, true).map_err(|e| Failure::new("headers", e))?)
                        .map_err(|e| Failure::new("headers", e))?;
                equal(
                    "headers",
                    "system prompt satellite",
                    &json!(expected),
                    &json!(format_prompts(&prompts)),
                )?;
            }
            let child = s.manifest["header"]["childToolSchemas"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(index)));
            let child_schemas = if index > 0 && child {
                Some(
                    schema_sets(
                        &satellite(all, s, index, false).map_err(|e| Failure::new("headers", e))?,
                    )
                    .map_err(|e| Failure::new("headers", e))?,
                )
            } else {
                None
            };
            let reasons: Vec<_> = rows
                .iter()
                .filter(|r| r["type"] == "request/header")
                .map(|r| r["data"]["reason"].clone())
                .collect();
            let mut revision = 0;
            for (i, h) in headers.iter().enumerate() {
                if s.suite == "acp" && reasons[i] == "change" {
                    revision += 1;
                }
                let selected = if s.suite == "acp" { revision } else { i };
                let mut expected = base
                    .get(selected)
                    .or_else(|| base.first())
                    .ok_or_else(|| Failure::new("headers", "unexpected request/header"))?
                    .clone();
                if let Some(schemas) = child_schemas.as_ref().and_then(|a| a.get(i)) {
                    expected["tools"] = schemas.clone();
                }
                if s.name == "subagent-dsh-sdk-dynamic-route" && index == 1 {
                    expected["config"] = json!({"provider":"mock","model":"mock-routed","reasoningEffort":"max","maxTokens":777});
                }
                equal(
                    "headers",
                    &format!("role {index} request header {i}"),
                    &expected,
                    h,
                )?;
            }
        }
        Ok(())
    };
    perform()
}

pub fn final_text(rows: &[Value]) -> String {
    rows.iter()
        .rev()
        .find(|r| r["type"] == "assistant/message")
        .and_then(|r| r.pointer("/data/message/content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect()
}
pub fn expected_exit(rows: &[Value]) -> i32 {
    if rows
        .iter()
        .rev()
        .find(|r| r["type"] == "turn/end")
        .is_some_and(|r| r["data"]["reason"]["kind"] == "completed")
    {
        0
    } else {
        1
    }
}
pub fn task(rows: &[Value]) -> Option<String> {
    fn text(message: &Value) -> Option<String> {
        let content = message["content"].as_array()?;
        (message["source"]["kind"] == "user" && content.len() == 1 && content[0]["type"] == "text")
            .then(|| content[0]["text"].as_str().map(str::to_owned))
            .flatten()
    }
    for row in rows.iter().filter(|r| r["type"] == "agent/inbox/spliced") {
        for message in row["data"]["inserted"].as_array().into_iter().flatten() {
            if let Some(s) = text(message) {
                return Some(s);
            }
        }
    }
    rows.iter()
        .filter(|r| r["type"] == "user/message")
        .find_map(|r| text(&r["data"]))
}

pub fn stderr(rows: &[Value]) -> Result<String> {
    struct State {
        out: String,
        open: bool,
        newline: bool,
    }
    impl State {
        fn close(&mut self) {
            if self.open && !self.newline {
                self.out.push('\n');
            }
            self.open = false;
            self.newline = true;
        }
        fn append(&mut self, text: &str) {
            if text.is_empty() {
                return;
            }
            if !self.open {
                self.out.push_str("dsh: reasoning:\n");
                self.open = true;
            }
            self.out.push_str(text);
            self.newline = text.ends_with('\n');
        }
        fn consume(&mut self, kind: &str, data: &Value) -> Result<()> {
            match kind {
                "reasoning-chunks" => {
                    for text in data["texts"].as_array().ok_or("invalid reasoning texts")? {
                        self.append(text.as_str().ok_or("invalid reasoning text")?);
                    }
                }
                "text-chunks" | "tool-call-chunks" => self.close(),
                "assistant/chunk" | "chunk" => {
                    let c = &data["chunk"];
                    match c["type"].as_str().unwrap_or("") {
                        "reasoning-delta" => {
                            self.append(c["text"].as_str().ok_or("invalid reasoning delta")?)
                        }
                        "block-start" => {
                            if c["blockType"] != "reasoning" {
                                self.close();
                            }
                        }
                        "block-end" => {
                            if c["block"]["type"] != "reasoning" {
                                self.close();
                            }
                        }
                        "text-delta" | "tool-call-delta" | "finish" => self.close(),
                        _ => {}
                    }
                }
                _ => {}
            }
            Ok(())
        }
    }
    let mut state = State {
        out: String::new(),
        open: false,
        newline: true,
    };
    let mut started = false;
    for r in rows {
        let kind = r["type"].as_str().unwrap_or("");
        if kind == "turn/start" {
            state.close();
            started = true;
            continue;
        }
        if !started {
            continue;
        }
        if ["assistant/message", "assistant/attempt"].contains(&kind)
            && let Some(stream) = r["data"]["stream"].as_array()
        {
            for entry in stream {
                state.consume(entry["type"].as_str().unwrap_or(""), entry)?;
            }
            state.close();
        } else {
            state.consume(kind, &r["data"])?;
        }
    }
    state.close();
    if let Some(reason) = rows
        .iter()
        .rev()
        .find(|r| r["type"] == "turn/end")
        .map(|r| &r["data"]["reason"])
        && reason["kind"] == "error"
    {
        state.out.push_str(&format!(
            "dsh: {}: {}\n",
            reason["error"]["code"]
                .as_str()
                .ok_or("error reason has no code")?,
            reason["error"]["message"]
                .as_str()
                .ok_or("error reason has no message")?
        ));
    }
    Ok(state.out)
}

pub fn headless_stdout(fixture: &str, stdout: &str, stderr_actual: &str) -> Check {
    let rows = records(fixture).map_err(|e| Failure::new("stdout", e))?;
    equal(
        "stdout",
        "stdout",
        &json!(final_text(&rows) + "\n"),
        &json!(stdout),
    )?;
    equal(
        "stdout",
        "stderr",
        &json!(stderr(&rows).map_err(|e| Failure::new("stdout", e))?),
        &json!(stderr_actual),
    )
}

/// Adjacent migration actually available today; the next missing edge is always reported.
pub fn migration_gate(raw: &str) -> Check {
    let mut rows = records(raw).map_err(|e| Failure::new("input", e))?;
    let header = rows
        .first_mut()
        .ok_or_else(|| Failure::new("input", "empty fixture"))?;
    let version = header.get("version").and_then(Value::as_u64).unwrap_or(0);
    match version {
        0 => {
            // Projected fixtures must be hydrated before the released codec validates cwd/coordinates.
            crate::normalize::map_strings(header, &mut |s, _| {
                s.replace("{{cwd}}", "/dsh-snapshot-cwd")
            });
            header["version"] = json!(0);
            if header.get("delegationDepth").is_none() {
                header["delegationDepth"] = json!(0);
            }
            let mut envelope = 0;
            let mut complete = None;
            for r in rows.iter_mut().skip(1) {
                if r["type"] == "request/header"
                    && let Some(header) =
                        r.pointer_mut("/data/header").and_then(Value::as_object_mut)
                {
                    if header.get("tools").is_some_and(|v| v == "{{tools}}") {
                        header.shift_remove("tools");
                    } else if let Some(tools) = header.get_mut("tools").filter(|v| {
                        v.as_array().is_some_and(|a| {
                            !a.is_empty()
                                && a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                        })
                    }) {
                        *tools = Value::Array(
                            tools
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|name| json!({"name":name,"description":"","parameters":{}}))
                                .collect(),
                        );
                    }
                }
                let packed = ["text-chunks", "reasoning-chunks", "tool-call-chunks"]
                    .contains(&r["type"].as_str().unwrap_or(""));
                let (seq, time) = if packed {
                    ("seq0", "time0")
                } else {
                    ("seq", "time")
                };
                let has_seq = r.get(seq).is_some();
                let has_time = r.get(time).is_some();
                if has_seq != has_time {
                    return Err(Failure::new(
                        "input",
                        "fixture row must contain both sequence and time or neither",
                    ));
                }
                if complete.is_some_and(|c| c != has_seq) {
                    return Err(Failure::new(
                        "input",
                        "fixture cannot mix projected and complete rows",
                    ));
                }
                complete = Some(has_seq);
                if !has_seq {
                    r[seq] = json!(envelope);
                    r[time] = json!(0);
                }
                envelope += if packed {
                    r["data"][if r["type"] == "tool-call-chunks" {
                        "args"
                    } else {
                        "texts"
                    }]
                    .as_array()
                    .map_or(1, |a| a.len().max(1))
                } else {
                    1
                };
            }
            let _artifact = hoh_session_format_v0_to_v1::testing::restore::restore_v0_to_v1(
                &rows[0],
                &rows[1..],
                hoh_session_format::SessionFormatRecovery::Strict,
            )
            .map_err(|e| Failure::new("input", format!("migration v0->v1: {e}")))?;
            Err(Failure::new("input", "migration v1->v2 not ported"))
        }
        1..=3 => Err(Failure::new(
            "input",
            format!("migration v{version}->v{} not ported", version + 1),
        )),
        4 => Ok(()),
        _ => Err(Failure::new(
            "input",
            format!("unsupported Session format v{version}"),
        )),
    }
}
