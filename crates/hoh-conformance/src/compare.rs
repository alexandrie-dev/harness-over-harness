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
    let mut expected = expected;
    // N-TOOLS-FOREIGN: only the recorded side loses the tools of plugins outside the 118.
    for rows in &mut expected {
        crate::normalize::v1::header_rows(rows);
    }
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
    let mut sets = vec![crate::normalize::v1::tools(&v["initial"])];
    sets.extend(
        v["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(crate::normalize::v1::tools),
    );
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
        let prompts = prompt_parts(&crate::normalize::v1::prompt(&read(&satellite(
            all, s, index, true,
        )?)?))?;
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

/// `restorePinnedToolSchemas` (suite.ts:532-548): a pinned header's `tools` must be the `{{tools}}`
/// token or the ordered names of the sidecar schemas (or be absent for an empty sidecar).
pub fn restore_pinned_tool_schemas(
    header: &mut Value,
    schemas: &Value,
) -> std::result::Result<(), String> {
    if !header.is_object() {
        return Err("acp-snapshot: pinned request header must be an object".into());
    }
    let list = schemas.as_array().map_or(&[][..], Vec::as_slice);
    let tools = header.get("tools");
    if tools.is_none() && list.is_empty() {
        return Ok(());
    }
    let names_match = tools.and_then(Value::as_array).is_some_and(|names| {
        names.len() == list.len()
            && names
                .iter()
                .zip(list)
                .all(|(name, schema)| name.is_string() && schema.get("name") == Some(name))
    });
    if tools != Some(&Value::from("{{tools}}")) && !names_match {
        return Err("acp-snapshot: pinned request header tools must equal {{tools}} or the ordered sidecar tool names".into());
    }
    header["tools"] = schemas.clone();
    Ok(())
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
            // The pin is recorded by the full plugin set: drop the same foreign tools as the sidecar.
            if let Some(names) = h.get_mut("tools") {
                crate::normalize::v1::header_tools(names);
            }
            restore_pinned_tool_schemas(h, tools).map_err(|e| Failure::new("headers", e))?;
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
                    &json!(crate::normalize::v1::prompt(&expected)),
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

/// `strictV4Catalog`: the strict read of one recorded Session artifact under the released format
/// catalog, as `llm-replay`'s `parseSessionFixture` performs it for every fixture a scenario
/// replays (`packages/test-support/llm-replay/src/index.ts:218-288`). The first line is the
/// physical header; `createRestore(header, {recovery: 'strict', validation: 'current'})` decodes
/// every following row and migrates older generations to V4 on the way (`:245-248`, `:279`,
/// `:288`). Projected fixtures first get the fixture-only completions of `normalizeProjectedHeader`
/// (`:389-398`: `delegationDepth: 0` for v0, `{{cwd}}` → `/dsh-snapshot-cwd`) and
/// `normalizeProjectedRow` (`:367-387`: request-tool tokens), and dense `seq`/zero `time` for rows
/// without an envelope (`:260-277`, cardinality `:401-407`).
///
/// A rejected fixture is an `input` failure carrying the TS location prefix
/// `session snapshot line N: `; the line of a late (finish) error is found as `:410-431` does.
pub fn strict_catalog(raw: &str) -> Check {
    migrate_fixture(raw).map(|_| ())
}

/// The artifact `strict_catalog` reads: the fixture strictly decoded and migrated to the current
/// generation (`prepareSessionSnapshotFixtureForComparison`'s input to the comparison).
pub fn migrate_fixture(
    raw: &str,
) -> std::result::Result<hoh_session_format::SessionFormatArtifact, Failure> {
    let located = |line: usize, e: &dyn std::fmt::Display| {
        Failure::new("input", format!("session snapshot line {line}: {e}"))
    };
    let catalog = hoh_session_format_catalog::create_session_format_catalog_with_children(vec![])
        .map_err(|e| Failure::new("input", format!("session format catalog: {e}")))?;
    let mut restore: Option<hoh_session_format::SessionFormatRestore> = None;
    let mut header_line = 1;
    let mut body_complete: Option<bool> = None;
    let mut next = 0_u64;
    let mut event_lines: Vec<usize> = Vec::new();
    // Physical body-row index → source line (`rowLines`), for `released Session row N` errors.
    let mut row_lines: Vec<usize> = Vec::new();
    for (index, line) in raw.split('\n').enumerate() {
        let line = line.trim_end_matches('\r');
        if hoh_util_values::js_trim(line).is_empty() {
            continue;
        }
        let number = index + 1;
        let mut row = hoh_util_values::js_json_parse(line).map_err(|_| {
            Failure::new(
                "input",
                format!("session snapshot line {number} contains invalid JSON"),
            )
        })?;
        if !row.is_object() {
            return Err(Failure::new(
                "input",
                format!("session snapshot line {number} must be a JSON object"),
            ));
        }
        let Some(active) = restore.as_mut() else {
            header_line = number;
            if row["version"] == 0 && row.get("delegationDepth").is_none() {
                row["delegationDepth"] = json!(0);
            }
            if let Some(cwd) = row["cwd"]
                .as_str()
                .filter(|c| *c == "{{cwd}}" || c.starts_with("{{cwd}}/"))
            {
                row["cwd"] = json!(cwd.replacen("{{cwd}}", "/dsh-snapshot-cwd", 1));
            }
            restore = Some(
                catalog
                    .create_restore(
                        &row,
                        hoh_session_format::SessionFormatRestoreOptions {
                            recovery: hoh_session_format::SessionFormatRecovery::Strict,
                            validation: hoh_session_format::SessionFormatValidation::Current,
                        },
                    )
                    .map_err(|e| located(number, &e))?,
            );
            continue;
        };
        if row["type"] == "request/header"
            && let Some(header) = row
                .pointer_mut("/data/header")
                .and_then(Value::as_object_mut)
        {
            if header.get("tools").is_some_and(|v| v == "{{tools}}") {
                header.shift_remove("tools");
            } else if let Some(tools) = header.get_mut("tools").filter(|v| {
                v.as_array().is_some_and(|a| {
                    !a.is_empty() && a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
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
            .contains(&row["type"].as_str().unwrap_or(""));
        let (seq, time) = if packed {
            ("seq0", "time0")
        } else {
            ("seq", "time")
        };
        let complete = row.get(seq).is_some();
        if complete != row.get(time).is_some() {
            return Err(Failure::new(
                "input",
                format!(
                    "session snapshot line {number} must contain both {seq} and {time}, or neither"
                ),
            ));
        }
        if body_complete.is_some_and(|previous| previous != complete) {
            return Err(Failure::new(
                "input",
                format!(
                    "session snapshot line {number} cannot mix projected and complete body rows"
                ),
            ));
        }
        body_complete = Some(complete);
        if !complete {
            row[seq] = json!(next);
            row[time] = json!(0);
        }
        let cardinality = if packed {
            row["data"][if row["type"] == "tool-call-chunks" {
                "args"
            } else {
                "texts"
            }]
            .as_array()
            .map_or(1, |a| a.len().max(1))
        } else {
            1
        };
        next += cardinality as u64;
        row_lines.push(number);
        event_lines.extend(std::iter::repeat_n(number, cardinality));
        active.decode_row(row).map_err(|e| located(number, &e))?;
    }
    let restore = restore.ok_or_else(|| {
        Failure::new("input", "session snapshot must start with a session header")
    })?;
    restore.finish().map_err(|e| {
        let detail = e.to_string();
        // `fixtureFormatError`: the location is read from `error.cause.message` when there is one.
        let location =
            std::error::Error::source(&e).map_or_else(|| detail.clone(), |c| c.to_string());
        let line = finish_error_line(&location, &row_lines, &event_lines, header_line);
        located(line, &detail)
    })
}

/// `normalizeSessionSnapshots([log], ctx)` of one fixture (headless.snapshot.ts:1149-1150): the
/// strictly migrated artifact, rendered as a log and run through the comparison normalizer.
pub fn normalized_session(raw: &str) -> std::result::Result<Value, Failure> {
    let artifact = migrate_fixture(raw)?;
    let mut header = artifact.header;
    header
        .entry("type")
        .or_insert_with(|| Value::String("session".into()));
    let mut text = hoh_util_values::js_json_stringify(&Value::Object(header));
    text.push('\n');
    for event in artifact.events {
        text.push_str(&hoh_util_values::js_json_stringify(&Value::Object(event)));
        text.push('\n');
    }
    let context = crate::normalize::Context {
        cwd: "/snapshot".into(),
        ..Default::default()
    };
    let logs = crate::normalize::Normalizer::new()
        .and_then(|n| n.logs(&[text], &context))
        .map_err(|e| Failure::new("body", e))?;
    Ok(json!(logs))
}

/// `fixtureFormatError`'s line choice for an error raised by `finish`: a `released Session row N`
/// message points at the Nth physical body row, an event/seq/cut index at the line of that event,
/// anything else at the header (`llm-replay/src/index.ts:410-431`).
pub(crate) fn finish_error_line(
    location: &str,
    row_lines: &[usize],
    event_lines: &[usize],
    header_line: usize,
) -> usize {
    let number = |pattern: &str| {
        regex::Regex::new(pattern).ok().and_then(|r| {
            r.captures(location)
                .and_then(|c| c[1].parse::<usize>().ok())
        })
    };
    if let Some(row) = number(r"^released Session row ([0-9]+)") {
        return row_lines.get(row).copied().unwrap_or(header_line);
    }
    // `index.ts:422-424`: the patterns are tried in this order, not by leftmost match.
    [
        r"Session event ([0-9]+)",
        r" at seq ([0-9]+)",
        r"inherited Session cut ([0-9]+)",
    ]
    .iter()
    .find_map(|pattern| number(pattern))
    .and_then(|n| event_lines.get(n).copied())
    .unwrap_or(header_line)
}

#[cfg(test)]
mod finish_error_line_tests {
    use super::finish_error_line;

    #[test]
    fn the_event_patterns_are_tried_in_the_order_of_the_source() {
        let events = [10, 11, 12, 13, 14, 15];
        let at = |text: &str| finish_error_line(text, &[], &events, 1);
        // `at seq 5` is the leftmost, `Session event 3` is tried first.
        assert_eq!(at("bad at seq 5 near Session event 3"), 13);
        assert_eq!(at("Session event 2 and inherited Session cut 4"), 12);
        assert_eq!(at("x at seq 4 and inherited Session cut 1"), 14);
        assert_eq!(at("only inherited Session cut 1"), 11);
        // `seed event at index N` is not a location pattern of the source.
        assert_eq!(at("seed event at index 3"), 1);
    }

    #[test]
    fn a_released_row_error_points_at_its_physical_row_not_its_event() {
        // Rows 0..3 live on lines 2, 3, 5; the packed row on line 5 carries events 2..=4.
        let rows = [2, 3, 5];
        let events = [2, 3, 5, 5, 5];
        let at = |text: &str| finish_error_line(text, &rows, &events, 1);
        assert_eq!(
            at("released Session row 2 has seq gap (expected 4, got 9)"),
            5
        );
        assert_eq!(at("released Session row 1 seq must be an integer"), 3);
        // The same number read as an event index lands elsewhere: the branches must not be mixed up.
        assert_eq!(at("Session event 4 is broken"), 5);
        assert_eq!(
            at("format v1 contains unknown event type \"x\" at seq 1"),
            3
        );
        // Out of range and unrelated messages fall back to the header line.
        assert_eq!(at("released Session row 9 is broken"), 1);
        assert_eq!(at("Session inheritedEventCount exceeds its event count"), 1);
        // The row pattern is anchored at the start, as in the source.
        assert_eq!(at("while reading released Session row 2"), 1);
    }
}
