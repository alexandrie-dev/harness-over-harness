//! Pure comparison projection from session-snapshot/{identity,normalize}.ts.
//! This layer does not claim to be the (as yet unported) V4 schema/catalog.
use crate::Result;
use regex::{Captures, Regex};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub type Logs = Vec<Vec<Value>>;

#[derive(Clone, Default)]
pub struct Context {
    pub cwd: String,
    pub aliases: Vec<String>,
    pub session_ids: Vec<String>,
}

pub struct Normalizer {
    uuid: Regex,
    short_job: Regex,
    reserved_id: Regex,
    canonical: Regex,
    mentions: Regex,
    anonymous: Regex,
    cwd_path: Regex,
    path_tag: Regex,
    instructions: Regex,
    spill: Regex,
    event_region: Regex,
    event_end: Regex,
    event_time: Regex,
    omitted: Regex,
    /// `cwdPathMode: "native"`: keep the captured separators of cwd-rooted paths.
    native_cwd_paths: bool,
}

impl Normalizer {
    /// The same normalizer with `cwdPathMode: "native"` (a platform-specific golden).
    pub fn with_native_cwd_paths(mut self) -> Self {
        self.native_cwd_paths = true;
        self
    }

    pub fn new() -> Result<Self> {
        fn re(s: &str) -> Result<Regex> {
            Regex::new(s).map_err(|e| e.to_string())
        }
        Ok(Self {
            uuid: re(r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")?,
            short_job: re(r"(?i)-[0-9a-f]{16}$")?,
            reserved_id: re(r"\{\{id:[1-9][0-9]*\}\}")?,
            canonical: re(
                r"^\{\{(session|message|approval|workflow|command|rpc|retry|id):([1-9][0-9]*)\}\}$",
            )?,
            mentions: re(r"(?i)(?-u:\b)as message ([0-9a-f-]{36})(?-u:\b)")?,
            anonymous: re(r"(?i)(?-u:\b)Anonymous user: ([0-9a-f-]{36})(?-u:\b)")?,
            cwd_path: re(&format!(
                r#"\{{\{{cwd\}}\}}(?:[\\/][^{}<>"'`]+)+"#,
                hoh_util_values::JS_WHITE_SPACE_SET
            ))?,
            path_tag: re(r"(<path>)([^<]*)(</path>)")?,
            instructions: re(r"(Additional instructions from: )([^\r\n]+)")?,
            // The following filename delimiter is checked in code: Rust regex has no lookahead.
            spill: re(
                r#"(?:\{\{cwd\}\}[\\/]+\.spill|(?:[A-Za-z]:)?[\\/]+(?:tmp|t)[\\/]+(?:dsh-acp-snap-[0-9a-f]{9}|dsh-acp-snapshot-spill))[\\/]+session-[0-9a-f]{12}[\\/]+[0-9a-f]{12}-([A-Za-z0-9._~-]+)"#,
            )?,
            event_region: re(
                r"^Session [^\r\n]+ — [^\r\n]+\r?\nTarget event seq [0-9]+:\r?\n```json\r?\n\{\r?\n",
            )?,
            event_end: re(r"\r?\n```(?:\r?\n|$)|\r?\n\r?\n\(Omitted ")?,
            event_time: re(r#"(?m)^(  "time": )[0-9]+(,\r?$)"#)?,
            omitted: re(r"(\r?\n\r?\n\(Omitted )[0-9]+( bytes\.)")?,
            native_cwd_paths: false,
        })
    }

    pub fn identities(&self, logs: &mut Logs) {
        let mut tokens = BTreeMap::<String, String>::new();
        let mut counters = HashMap::<String, u64>::new();
        let mut claim = |value: &Value, kind: &str, always: bool| {
            let Some(value) = value.as_str() else { return };
            if value.is_empty() || tokens.contains_key(value) {
                return;
            }
            let canonical = self.canonical.captures(value);
            if !always
                && canonical.is_none()
                && value != "{{sessionId}}"
                && value != "{{messageId}}"
                && !self.uuid.is_match(value)
                && !self.short_job.is_match(value)
            {
                return;
            }
            if let Some(c) = canonical {
                let n = c[2].parse::<u64>().unwrap_or(0);
                let counter = counters.entry(c[1].into()).or_default();
                *counter = (*counter).max(n);
                tokens.insert(value.into(), value.into());
            } else {
                let n = counters.entry(kind.into()).or_default();
                *n += 1;
                tokens.insert(value.into(), format!("{{{{{kind}:{n}}}}}"));
            }
        };
        for log in logs.iter() {
            if let Some(header) = log.first().filter(|h| h["type"] == "session") {
                claim(&header["id"], "session", true);
            }
        }
        fn collect(
            value: &Value,
            kind: &str,
            n: &Normalizer,
            claim: &mut impl FnMut(&Value, &str, bool),
        ) {
            match value {
                Value::String(text) => {
                    for c in n.mentions.captures_iter(text) {
                        claim(&json!(&c[1]), "message", false);
                    }
                    for c in n.anonymous.captures_iter(text) {
                        claim(&json!(&c[1]), "id", false);
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        collect(item, kind, n, claim);
                    }
                }
                Value::Object(map) => {
                    if value["id"].is_string()
                        && value["role"].is_string()
                        && value["content"].is_array()
                        && value["source"].is_object()
                    {
                        claim(&value["id"], "message", false);
                    }
                    for (key, item) in map {
                        if kind == "approval/asked" || kind == "approval/decided" {
                            if key == "id" {
                                claim(item, "approval", false);
                            }
                        } else {
                            match key.as_str() {
                                "commandId" => claim(item, "command", true),
                                "rpcId" => claim(item, "rpc", true),
                                "retryId" => claim(item, "retry", false),
                                "runId" => claim(item, "workflow", false),
                                key if key == "id"
                                    || key.ends_with("Id")
                                    || key.ends_with("Ids") =>
                                {
                                    claim(item, "id", false)
                                }
                                _ => {}
                            }
                        }
                        collect(item, kind, n, claim);
                    }
                }
                _ => {}
            }
        }
        for log in logs.iter() {
            for record in log {
                let kind = record["type"].as_str().unwrap_or("");
                if kind == "feedback/message-put" {
                    claim(&record["data"]["item"]["version"], "id", false);
                }
                collect(record, kind, self, &mut claim);
            }
        }
        let mut replacements: Vec<_> = tokens.into_iter().collect();
        replacements.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        for log in logs {
            for record in log {
                map_strings(record, &mut |text, _| replace_all(text, &replacements));
            }
        }
    }

    /// Every known spelling of the cwd (and its macOS `/private` forms), longest first.
    fn replace_known_cwd(text: &str, ctx: &Context) -> String {
        let mut spellings = vec![ctx.cwd.clone()];
        spellings.extend(ctx.aliases.iter().cloned());
        spellings.extend(
            spellings
                .clone()
                .into_iter()
                .filter(|s| s.starts_with('/') && !s.starts_with("/private/"))
                .map(|s| format!("/private{s}")),
        );
        spellings.retain(|s| !s.is_empty());
        spellings.sort_by_key(|s| std::cmp::Reverse(s.len()));
        spellings.dedup();
        let mut out = text.to_owned();
        for spelling in spellings {
            out = replace_cwd(&out, &spelling);
        }
        out.replace("/private{{cwd}}", "{{cwd}}")
    }

    pub fn string(&self, text: &str, ctx: &Context, legacy: bool) -> String {
        let mut out = Self::replace_known_cwd(text, ctx);
        if !self.native_cwd_paths {
            out = self
                .cwd_path
                .replace_all(&out, |c: &Captures<'_>| c[0].replace('\\', "/"))
                .into_owned();
            out = self
                .path_tag
                .replace_all(&out, |c: &Captures<'_>| {
                    format!("{}{}{}", &c[1], c[2].replace('\\', "/"), &c[3])
                })
                .into_owned();
            out = self
                .instructions
                .replace_all(&out, |c: &Captures<'_>| {
                    format!("{}{}", &c[1], c[2].replace('\\', "/"))
                })
                .into_owned();
        }
        out = self
            .spill
            .replace_all(&out, |c: &Captures<'_>| {
                let matched = c.get(0).map_or(0, |m| m.end());
                let mut name = c[1].to_owned();
                if name.ends_with('.') && out[matched..].starts_with(" Use read with offset/limit")
                {
                    name.pop();
                    return format!("{{{{spillLocator:{name}}}}}.");
                }
                let rest = &out[matched..];
                if rest.is_empty()
                    || rest.starts_with(|ch: char| {
                        hoh_util_values::is_js_white_space(ch) || matches!(ch, ')' | '"')
                    })
                    || rest.trim_start_matches('\\').starts_with('"')
                {
                    format!("{{{{spillLocator:{name}}}}}")
                } else {
                    c[0].to_owned()
                }
            })
            .into_owned();
        if let Some(region) = self.event_region.find(&out) {
            let offset = region.end();
            if let Some(end) = self
                .event_end
                .find(&out[offset..])
                .map(|m| offset + m.start())
            {
                let prefix = self
                    .event_time
                    .replace_all(&out[..end], "${1}{{eventTime}}${2}");
                out = format!("{prefix}{}", &out[end..]);
                out = self
                    .omitted
                    .replace_all(&out, "${1}{{eventOmittedBytes}}${2}")
                    .into_owned();
            }
        }
        if legacy {
            for id in &ctx.session_ids {
                if !id.is_empty() {
                    out = out.replace(id, "{{sessionId}}");
                }
            }
            out = self.uuid.replace_all(&out, "{{sessionId}}").into_owned();
        }
        out
    }

    pub fn value(&self, v: &mut Value, ctx: &Context, legacy: bool) {
        map_strings(v, &mut |s, key| {
            if legacy && key == Some("messageId") {
                "{{messageId}}".into()
            } else {
                let out = self.string(s, ctx, legacy);
                if key == Some("path") && !self.native_cwd_paths {
                    out.replace('\\', "/")
                } else {
                    out
                }
            }
        });
        fn usage(v: &mut Value) {
            match v {
                Value::Array(a) => {
                    for v in a {
                        usage(v);
                    }
                }
                Value::Object(o) => {
                    if o.get("sessionUpdate").is_some_and(|v| v == "usage_update")
                        && o.get("used").is_some_and(Value::is_number)
                    {
                        o.insert("used".into(), json!("{{usedTokens}}"));
                    }
                    for v in o.values_mut() {
                        usage(v);
                    }
                }
                _ => {}
            }
        }
        usage(v);
    }

    /// normalize.ts:359–387: reserve existing tokens and replace jobs before legacy UUID scrubbing.
    pub fn legacy_log(&self, raw: &str, ctx: &Context) -> Result<Vec<Value>> {
        let mut rows = crate::records(raw)?;
        let reserved: std::collections::HashSet<&str> = self
            .reserved_id
            .find_iter(raw)
            .map(|m| m.as_str())
            .collect();
        let mut jobs = Vec::<(String, String)>::new();
        let mut next = 1;
        for row in &rows {
            if !["job/registered", "job/settled"].contains(&row["type"].as_str().unwrap_or("")) {
                continue;
            }
            let Some(id) = row["data"]["jobId"].as_str() else {
                continue;
            };
            if (!self.uuid.is_match(id) && !self.short_job.is_match(id))
                || jobs.iter().any(|(known, _)| known == id)
            {
                continue;
            }
            let token = loop {
                let token = format!("{{{{id:{next}}}}}");
                next += 1;
                if !reserved.contains(token.as_str()) {
                    break token;
                }
            };
            jobs.push((id.into(), token));
        }
        for row in &mut rows {
            map_strings(row, &mut |s, _| {
                let mut out = s.to_owned();
                for (id, token) in &jobs {
                    out = out.replace(id, token);
                }
                out
            });
            self.clocks(row)?;
            self.value(row, ctx, true);
        }
        Ok(rows)
    }

    /// `normalizeSessionSnapshots(raw, ctx, {nativeWriterOutput: true})`: the logs are writer output,
    /// so a delivery marker of the log's own generation is tokenized.
    pub fn logs(&self, raw: &[String], ctx: &Context) -> Result<Logs> {
        self.logs_with(raw, ctx, true)
    }

    /// `normalizeSessionSnapshots`; without `native_writer_output` delivery markers keep their
    /// generation (the source-versus-migrated-artifact comparison is generation-exact).
    pub fn logs_with(
        &self,
        raw: &[String],
        ctx: &Context,
        native_writer_output: bool,
    ) -> Result<Logs> {
        let mut logs = raw
            .iter()
            .map(|s| crate::records(s))
            .collect::<Result<Logs>>()?;
        for log in &mut logs {
            let header = log
                .first_mut()
                .ok_or("session snapshot must start with a session header")?;
            if header["type"] != "session" {
                return Err("session snapshot must start with a session header".into());
            }
            let version = header.get("version").cloned();
            // The adjacent migrations refuse a delivery marker that already claims their target
            // generation (`session-format-v2-to-v3`, `-v3-to-v4`); the source reaches them through
            // `prepareSessionSnapshotFixtureForComparison`.
            if let Some(source) = version
                .as_ref()
                .and_then(Value::as_u64)
                .filter(|v| (2..=3).contains(v))
            {
                let claims = log.iter().skip(1).any(|e| {
                    e["type"] == "session-log-deepseek/delivery-accepted"
                        && e["data"]["sessionFormatVersion"].as_u64() == Some(source + 1)
                });
                if claims {
                    return Err(format!(
                        "format v{source} delivery marker claims target format v{}",
                        source + 1
                    ));
                }
            }
            let header = &mut log[0];
            if let Some(o) = header.as_object_mut() {
                o.shift_remove("version");
            }
            for event in log.iter_mut().skip(1) {
                if native_writer_output
                    && event["type"] == "session-log-deepseek/delivery-accepted"
                    && version.as_ref() == event["data"].get("sessionFormatVersion")
                    && version.is_some()
                {
                    event["data"]["sessionFormatVersion"] = json!("{{sourceSessionFormatVersion}}");
                }
            }
        }
        self.identities(&mut logs);
        for log in &mut logs {
            let retain_names = log.iter().any(|r| r["type"] == "developer/message");
            for (i, record) in log.iter_mut().enumerate() {
                self.clocks(record)?;
                self.value(record, ctx, false);
                if i > 0 {
                    scrub_bulk(record, retain_names);
                    if let Some(o) = record.as_object_mut() {
                        for key in ["seq", "time", "seq0", "time0"] {
                            o.shift_remove(key);
                        }
                    }
                }
            }
        }
        Ok(logs)
    }

    pub fn clocks(&self, record: &mut Value) -> Result<()> {
        let kind = record["type"].as_str().unwrap_or("").to_owned();
        if kind == "session" {
            zero(record, &["createdAt"], false);
        } else if ["text-chunks", "reasoning-chunks", "tool-call-chunks"].contains(&kind.as_str()) {
            zero(record, &["time0"], false);
            zeros(record.pointer_mut("/data/dt"));
        } else {
            zero(record, &["time"], false);
        }
        if ["assistant/message", "assistant/attempt"].contains(&kind.as_str())
            && let Some(stream) = record
                .pointer_mut("/data/stream")
                .and_then(Value::as_array_mut)
        {
            for v in stream {
                zero(v, &["time", "time0"], true);
                zeros(v.get_mut("dt"));
            }
        }
        // `record["data"]` would insert `data: null` into a row that has none; only touch an existing one.
        let mut data = |keys: &[&str]| {
            if let Some(data) = record.get_mut("data") {
                zero(data, keys, false);
            }
        };
        match kind.as_str() {
            "hook/result" => data(&["durationMs"]),
            "job/registered" | "job/settled" => data(&["startedAt", "finishedAt"]),
            "goal/change" => data(&["createdAt", "updatedAt"]),
            "subagent/catalog" => data(&["childCreatedAt"]),
            "feedback/message-put" | "plugin:timeline/tree" | "tool/result" => {
                service_clocks(record, &kind)
            }
            _ => {}
        }
        if let Some(ranges) = record.get_mut("sourceEventSeqs") {
            *ranges = decode_ranges(ranges)?;
        }
        Ok(())
    }

    pub fn frames(&self, raw: &str, ctx: &Context) -> Result<Vec<Value>> {
        self.frames_mode(raw, ctx, true)
    }

    fn frames_mode(&self, raw: &str, ctx: &Context, legacy: bool) -> Result<Vec<Value>> {
        let mut frames = crate::records(raw)?;
        let mut ids = HashMap::new();
        for frame in &mut frames {
            if !frame.is_object() {
                return Err("protocol frame must be an object".into());
            }
            if let Some(id) = frame.get_mut("id").filter(|v| !v.is_null()) {
                let key = hoh_util_values::js_json_stringify(id);
                let next = ids.len() + 1;
                *id = json!(*ids.entry(key).or_insert(next));
            }
            self.value(frame, ctx, legacy);
        }
        Ok(frames)
    }

    /// SDK notification projection, including the separate shared identity pass for feedback/jobs.
    pub fn notifications(
        &self,
        notifications: &[Value],
        ctx: &Context,
        source_version: Option<u64>,
    ) -> Result<Vec<Value>> {
        let mut events: Vec<Value> = notifications
            .iter()
            .filter(|n| n["method"] == "session.event")
            .map(|n| n["params"]["event"].clone())
            .collect();
        let typed = events.iter().any(|e| {
            ["feedback/message-put", "job/registered", "job/settled"]
                .contains(&e["type"].as_str().unwrap_or(""))
        });
        if typed {
            let mut logs = Vec::new();
            for (i, id) in ctx.session_ids.iter().enumerate() {
                let mut log = vec![json!({"type":"session","id":id})];
                if i == 0 {
                    log.extend(events.clone());
                }
                logs.push(log);
            }
            if logs.is_empty() {
                return Err("typed notification normalization requires root session id".into());
            }
            self.identities(&mut logs);
            events = logs.remove(0).into_iter().skip(1).collect();
        }
        let retain_names = events.iter().any(|r| r["type"] == "developer/message");
        for event in &mut events {
            if event["type"] == "session-log-deepseek/delivery-accepted"
                && source_version.is_some()
                && event["data"]["sessionFormatVersion"].as_u64() == source_version
            {
                event["data"]["sessionFormatVersion"] = json!("{{sourceSessionFormatVersion}}");
            }
            self.clocks(event)?;
            self.value(event, ctx, !typed);
            scrub_bulk(event, retain_names);
        }
        let mut output = notifications.to_vec();
        let mut event_index = 0;
        for notification in &mut output {
            if notification["method"] == "session.event" {
                notification["params"]["event"] = events[event_index].clone();
                event_index += 1;
            }
            // dsh intentionally drops transport fields from SDK notifications.
            *notification =
                json!({"method":notification["method"],"params":notification["params"]});
        }
        if typed {
            for notification in &mut output {
                map_strings(notification, &mut |s, _| {
                    let mut result = s.to_owned();
                    for (i, id) in ctx.session_ids.iter().enumerate() {
                        result = result.replace(id, &format!("{{{{session:{}}}}}", i + 1));
                    }
                    result
                });
            }
        }
        self.frames_mode(&crate::jsonl(&output), ctx, !typed)
    }
}

/// `scrubSystemPrompts` of one record: the prompt text of a `system/message` becomes `{{system}}`.
pub fn scrub_system(record: &mut Value) {
    if record["type"] == "system/message"
        && let Some(text) = record
            .pointer_mut("/data/message/content/0/text")
            .filter(|v| v.is_string())
    {
        *text = json!("{{system}}");
    }
}

/// `scrubToolSchemas` of one record: the tools of a `request/header` become `{{tools}}`
/// (or their names, when the log carries developer messages).
pub fn scrub_tools(record: &mut Value, retain_names: bool) {
    if record["type"] == "request/header"
        && let Some(tools) = record.pointer_mut("/data/header/tools")
    {
        *tools = if retain_names && tools.is_array() {
            Value::Array(
                tools
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|t| {
                        if t.is_string() {
                            t.clone()
                        } else {
                            t["name"].clone()
                        }
                    })
                    .collect(),
            )
        } else {
            json!("{{tools}}")
        };
    }
}

pub fn scrub_bulk(record: &mut Value, retain_names: bool) {
    scrub_system(record);
    scrub_tools(record, retain_names);
}

fn zeros(v: Option<&mut Value>) {
    if let Some(Value::Array(a)) = v {
        for v in a {
            *v = json!(0);
        }
    }
}
fn zero(v: &mut Value, keys: &[&str], numeric: bool) {
    for key in keys {
        if let Some(v) = v.get_mut(*key)
            && (!numeric || v.is_number())
        {
            *v = json!(0);
        }
    }
}

pub fn decode_ranges(v: &Value) -> Result<Value> {
    let mut result = Vec::new();
    let mut has_range = false;
    for item in v.as_array().ok_or("sourceEventSeqs must be an array")? {
        if let Some(n) = item.as_u64().filter(|n| *n <= 9_007_199_254_740_991) {
            result.push(json!(n));
        } else if let Some(pair) = item.as_array().filter(|p| p.len() == 2) {
            let start = pair[0].as_u64().ok_or("invalid seq range")?;
            let end = pair[1].as_u64().ok_or("invalid seq range")?;
            if end < start || end > 9_007_199_254_740_991 || end - start > 1_000_000 {
                return Err("invalid seq range".into());
            }
            for n in start..=end {
                result.push(json!(n));
            }
            has_range = true;
        } else {
            return Err("invalid seq range".into());
        }
    }
    if has_range && result.windows(2).any(|p| p[0].as_u64() >= p[1].as_u64()) {
        return Err("sourceEventSeqs ranges must be strictly increasing".into());
    }
    Ok(Value::Array(result))
}

pub fn map_strings(v: &mut Value, f: &mut impl FnMut(&str, Option<&str>) -> String) {
    fn visit(v: &mut Value, key: Option<&str>, f: &mut impl FnMut(&str, Option<&str>) -> String) {
        match v {
            Value::String(s) => *s = f(s, key),
            Value::Array(a) => {
                for v in a {
                    visit(v, None, f);
                }
            }
            Value::Object(o) => {
                for (k, v) in o {
                    visit(v, Some(k), f);
                }
            }
            _ => {}
        }
    }
    visit(v, None, f);
}
fn replace_all(s: &str, replacements: &[(String, String)]) -> String {
    if let Some((_, token)) = replacements.iter().find(|(source, _)| source == s) {
        return token.clone();
    }
    let mut out = s.to_owned();
    for (source, token) in replacements {
        out = out.replace(source, token);
    }
    out
}
fn boundary(c: char) -> bool {
    hoh_util_values::is_js_white_space(c) || "<>'\"`()[]{},;:!?=".contains(c)
}
fn replace_cwd(text: &str, cwd: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(cwd) {
        let start = cursor + offset;
        let end = start + cwd.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let prefix = text[..start].to_ascii_lowercase();
        let file_prefix = prefix
            .strip_suffix("file://")
            .or_else(|| prefix.strip_suffix("file:///"));
        let uri = file_prefix.is_some_and(|s| {
            s.chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && !"+.-".contains(c))
        });
        let starts = before.is_none_or(boundary) || uri;
        let ends = after.is_none_or(|c| c == '/' || c == '\\' || boundary(c))
            || after == Some('.') && text[end + 1..].chars().next().is_none_or(boundary);
        out.push_str(&text[cursor..start]);
        out.push_str(if starts && ends { "{{cwd}}" } else { cwd });
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Normalization of the first version (see NORMALIZATION.md, which lists every rule with its test).
///
/// The recordings were made by the full set of dsh plugins. The first version has 118 entries;
/// everything below removes only what a plugin outside them contributes to the *recorded* side.
/// The actual side is never rewritten, so anything our own plugins add or change stays a difference.
pub mod v1 {
    use serde_json::Value;

    /// Rule `N-PROMPT-FOREIGN`: system-prompt paragraphs written by plugins outside the 118.
    /// `(paragraph prefix, owner package)`; a paragraph is the text between blank lines.
    pub const FOREIGN_PROMPT_PARAGRAPHS: &[(&str, &str)] = &[
        (
            "Use the glob tool — not shell find — ",
            "@deepseek-ai/dsh-tool-fs-search",
        ),
        (
            "Use the grep tool — not shell grep or rg — ",
            "@deepseek-ai/dsh-tool-fs-search",
        ),
        (
            "Use the workflow tool ONLY when the user explicitly asks for a workflow",
            "@deepseek-ai/dsh-tool-workflow",
        ),
    ];

    /// Rule `N-TOOLS-FOREIGN`: tools registered by plugins outside the 118.
    pub const FOREIGN_TOOLS: &[(&str, &str)] = &[
        ("glob", "@deepseek-ai/dsh-tool-fs-search"),
        ("grep", "@deepseek-ai/dsh-tool-fs-search"),
        ("workflow", "@deepseek-ai/dsh-tool-workflow"),
    ];

    /// Rule `N-BOOT-FOREIGN-PLUGINS`: plugin rows of the full dsh profiles whose package is outside
    /// the 118 and has no Rust implementation. The overlay disables them, as an operator would, so
    /// the profile boots; `(row id, package)`.
    pub const FOREIGN_PLUGINS: &[(&str, &str)] = &[
        ("connection", "@deepseek-ai/dsh-client-connection"),
        ("file-upload", "@deepseek-ai/dsh-client-file-upload"),
        ("deepseek-account", "@deepseek-ai/dsh-deepseek-account"),
        (
            "llm-deepseek-account",
            "@deepseek-ai/dsh-llm-deepseek-account",
        ),
        ("llm-pi-ai", "@deepseek-ai/dsh-llm-pi-ai"),
        ("hooks-claude-code", "@deepseek-ai/dsh-hooks-claude-code"),
        ("hooks-codex", "@deepseek-ai/dsh-hooks-codex"),
        ("plugin-manager", "@deepseek-ai/dsh-plugin-manager"),
        (
            "tool-plugin-manager",
            "@deepseek-ai/dsh-plugin-manager/tools",
        ),
        ("ptc-runtime", "@deepseek-ai/dsh-ptc-runtime-node"),
        ("workflow-ptc", "@deepseek-ai/dsh-workflow-ptc"),
        ("tool-workflow", "@deepseek-ai/dsh-tool-workflow"),
        ("tool-fs-search", "@deepseek-ai/dsh-tool-fs-search"),
        (
            "repeat-tool-reminder",
            "@deepseek-ai/dsh-repeat-tool-reminder",
        ),
        ("plan-mode", "@deepseek-ai/dsh-plan-mode"),
        ("typert", "@deepseek-ai/dsh-typert-registry"),
        ("typert-loader", "@deepseek-ai/dsh-typert-loader"),
        ("typert-gateway", "@deepseek-ai/dsh-api-gateway"),
        ("remote-access", "@deepseek-ai/dsh-api-remote-access"),
        ("timer", "@deepseek-ai/cordis-plugin-timer"),
        (
            "session-title-provider",
            "@deepseek-ai/dsh-session-title-all-prompts-llm",
        ),
    ];

    /// The overlay patch (JSON is valid YAML) appended after every scenario patch.
    pub fn exclusion_patch() -> String {
        exclusion_patch_with(&[])
    }

    /// The same overlay plus rows an operator disabled with `--disable-plugin` for a diagnostic run.
    /// Those rows are not normalization: a run that uses them is labelled as diagnostic.
    pub fn exclusion_patch_with(extra: &[String]) -> String {
        let rows: Vec<Value> = FOREIGN_PLUGINS
            .iter()
            .map(|(id, _)| *id)
            .chain(extra.iter().map(String::as_str))
            .map(|id| serde_json::json!({"id": id, "disabled": true, "insert": null}))
            .collect();
        hoh_util_values::js_json_stringify(&Value::Array(rows))
    }

    /// Every rule id; NORMALIZATION.md must carry a section for each.
    pub const RULES: &[&str] = &[
        "N-BOOT-FOREIGN-PLUGINS",
        "N-PROMPT-FOREIGN",
        "N-TOOLS-FOREIGN",
    ];

    /// Drop the foreign paragraphs from a recorded system prompt (one or several `system/message`
    /// revisions joined by the satellite separators).
    pub fn prompt(text: &str) -> String {
        let kept: Vec<&str> = text
            .split("\n\n")
            .filter(|paragraph| {
                !FOREIGN_PROMPT_PARAGRAPHS
                    .iter()
                    .any(|(prefix, _)| paragraph.starts_with(prefix))
            })
            .collect();
        let mut result = kept.join("\n\n");
        // A removed final paragraph carried the file's closing newline.
        if text.ends_with('\n') && !result.ends_with('\n') {
            result.push('\n');
        }
        result
    }

    /// Apply `N-TOOLS-FOREIGN` to the `tools` of every recorded `request/header` row, whether the
    /// row lists tool names or tool definitions.
    pub fn header_rows(rows: &mut [Value]) {
        for row in rows.iter_mut().filter(|r| r["type"] == "request/header") {
            if let Some(tools) = row.pointer_mut("/data/header/tools") {
                header_tools(tools);
            }
        }
    }

    /// `N-TOOLS-FOREIGN` for one recorded header's `tools` (names or definitions).
    pub fn header_tools(tools: &mut Value) {
        if let Some(list) = tools.as_array_mut() {
            list.retain(|tool| {
                let name = tool.as_str().or_else(|| tool["name"].as_str());
                !name.is_some_and(|name| FOREIGN_TOOLS.iter().any(|(foreign, _)| *foreign == name))
            });
        }
    }

    /// Drop the foreign tools from a recorded tool-schema set (an array of tool definitions).
    pub fn tools(set: &Value) -> Value {
        match set.as_array() {
            Some(tools) => Value::Array(
                tools
                    .iter()
                    .filter(|tool| {
                        !tool["name"].as_str().is_some_and(|name| {
                            FOREIGN_TOOLS.iter().any(|(foreign, _)| *foreign == name)
                        })
                    })
                    .cloned()
                    .collect(),
            ),
            None => set.clone(),
        }
    }
}

/// `normalizeSessionFormatMetadata`: the header generation is omitted; with a validated source
/// generation the delivery markers of that generation are tokenized. Other lines are untouched.
pub fn normalize_session_format_metadata(raw: &str, source_version: Option<u64>) -> Result<String> {
    raw.split('\n')
        .map(|line| {
            if hoh_util_values::js_trim(line).is_empty() {
                return Ok(line.to_owned());
            }
            let mut record = hoh_util_values::js_json_parse(line).map_err(|e| e.to_string())?;
            if record["type"] == "session" && record.get("version").is_some() {
                if let Some(object) = record.as_object_mut() {
                    object.shift_remove("version");
                }
            } else if let Some(source) = source_version
                && record["type"] == "session-log-deepseek/delivery-accepted"
            {
                if record["data"]["sessionFormatVersion"].as_u64() != Some(source) {
                    return Ok(line.to_owned());
                }
                record["data"]["sessionFormatVersion"] = json!("{{sourceSessionFormatVersion}}");
            } else {
                return Ok(line.to_owned());
            }
            Ok(hoh_util_values::js_json_stringify(&record))
        })
        .collect::<Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

fn scrub_lines(raw: &str, scrub: impl Fn(&mut Value, bool)) -> Result<String> {
    let lines: Vec<&str> = raw.split('\n').collect();
    let records: Vec<Option<Value>> = lines
        .iter()
        .map(|line| {
            if hoh_util_values::js_trim(line).is_empty() {
                Ok(None)
            } else {
                hoh_util_values::js_json_parse(line)
                    .map(Some)
                    .map_err(|e| e.to_string())
            }
        })
        .collect::<Result<_>>()?;
    let retain = records
        .iter()
        .flatten()
        .any(|r| r["type"] == "developer/message");
    Ok(lines
        .iter()
        .zip(records)
        .map(|(line, record)| match record {
            None => (*line).to_owned(),
            Some(mut record) => {
                let before = record.clone();
                scrub(&mut record, retain);
                if record == before {
                    (*line).to_owned()
                } else {
                    hoh_util_values::js_json_stringify(&record)
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// `scrubSystemPrompts`: only `system/message` text; every other line is byte-identical.
pub fn scrub_system_prompts(raw: &str) -> Result<String> {
    scrub_lines(raw, |r, _| scrub_system(r))
}
/// `scrubToolSchemas`: only request-header tools; every other line is byte-identical.
pub fn scrub_tool_schemas(raw: &str) -> Result<String> {
    scrub_lines(raw, scrub_tools)
}
/// `scrubModelRequestBulk`: both.
pub fn scrub_model_request_bulk(raw: &str) -> Result<String> {
    scrub_lines(raw, scrub_bulk_with)
}
fn scrub_bulk_with(record: &mut Value, retain: bool) {
    scrub_bulk(record, retain);
}

/// The feedback and Timeline clocks that `scrubSessionSnapshot` and the log normalizer both zero
/// (`normalizeFeedbackClocks`, `normalizeTimelineClocks`).
fn service_clocks(record: &mut Value, kind: &str) {
    match kind {
        "feedback/message-put" => {
            if let Some(item) = record.pointer_mut("/data/item") {
                zero(item, &["createdAt", "updatedAt"], false);
            }
        }
        "plugin:timeline/tree" => {
            if let Some(tree) = record.pointer_mut("/data/tree") {
                zero(tree, &["createdAt", "lastTaskUpdate"], true);
                if let Some(tasks) = tree["tasks"].as_array_mut() {
                    for task in tasks {
                        zero(task, &["createdAt", "updatedAt"], true);
                    }
                }
            }
            if let Some(receipt) = record.pointer_mut("/data/receipt") {
                zero(receipt, &["at", "previousAt"], true);
            }
        }
        "tool/result" => {
            if let Some(timeline) = record.pointer_mut("/data/meta/timeline") {
                zero(timeline, &["at", "previousAt"], true);
            }
        }
        _ => {}
    }
}

/// `scrubSessionSnapshot`: the header line is kept byte for byte; every body record is scrubbed,
/// loses its persistence envelope, expands `sourceEventSeqs` and gets stable service clocks.
pub fn scrub_session_snapshot(raw: &str) -> Result<String> {
    let scrubbed = scrub_model_request_bulk(raw)?;
    let mut first = true;
    scrubbed
        .split('\n')
        .map(|line| {
            if hoh_util_values::js_trim(line).is_empty() {
                return Ok(line.to_owned());
            }
            let mut record = hoh_util_values::js_json_parse(line).map_err(|e| e.to_string())?;
            if std::mem::take(&mut first) {
                if record["type"] != "session" {
                    return Err("session snapshot must start with a session header".into());
                }
                return Ok(line.to_owned());
            }
            if let Some(object) = record.as_object_mut() {
                for key in ["seq", "time", "seq0", "time0"] {
                    object.shift_remove(key);
                }
            }
            if let Some(ranges) = record.get_mut("sourceEventSeqs") {
                *ranges = decode_ranges(ranges)?;
            }
            let kind = record["type"].as_str().unwrap_or("").to_owned();
            service_clocks(&mut record, &kind);
            Ok(hoh_util_values::js_json_stringify(&record))
        })
        .collect::<Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

/// `extractSnapshotSpillPaths`: every snapshot-mode spill path of a log keyed by spill filename;
/// the last match wins per name.
pub fn extract_snapshot_spill_paths(
    content: &str,
) -> Result<std::collections::BTreeMap<String, String>> {
    let prefix = Regex::new(
        r"(?:[A-Za-z]:)?[\\/]+(?:tmp|t)[\\/]+(?:dsh-acp-snap-[0-9a-f]{9}|dsh-acp-snapshot-spill)[\\/]+session-[0-9a-f]{12}[\\/]+[0-9a-f]{12}-",
    )
    .map_err(|e| e.to_string())?;
    let mut result = std::collections::BTreeMap::new();
    let mut from = 0;
    while let Some(found) = prefix.find_at(content, from) {
        let rest = &content[found.end()..];
        let run = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || "._~-".contains(c)))
            .unwrap_or(rest.len());
        // The name is the shortest run of at least one character followed by a delimiter.
        let end = (1..=run).filter(|n| rest.is_char_boundary(*n)).find(|n| {
            let after = &rest[*n..];
            after.starts_with(". Use read with offset/limit")
                || after.starts_with(|c: char| {
                    hoh_util_values::is_js_white_space(c) || matches!(c, ')' | '"')
                })
                || after.trim_start_matches('\\').starts_with('"') && after.starts_with('\\')
                || after.is_empty()
        });
        match end {
            Some(length) => {
                result.insert(
                    rest[..length].to_owned(),
                    content[found.start()..found.end() + length].to_owned(),
                );
                from = found.end() + length;
            }
            None => from = found.start() + 1,
        }
    }
    Ok(result)
}

/// `tokenizeSessionFixtureCwd`: store one generated workspace as `{{cwd}}` while retaining every
/// other session value (the fixture writer of record/refresh runs).
pub fn tokenize_session_fixture_cwd(raw: &str) -> Result<String> {
    let lines: Vec<&str> = raw.split('\n').collect();
    let first = lines
        .iter()
        .find(|l| !hoh_util_values::js_trim(l).is_empty());
    let header = match first {
        Some(line) => hoh_util_values::js_json_parse(line).map_err(|e| e.to_string())?,
        None => Value::Null,
    };
    let cwd = header["cwd"].as_str().unwrap_or("").to_owned();
    let basename = cwd.rsplit(['\\', '/']).next().unwrap_or("").to_owned();
    if basename.is_empty() {
        return Err("acp-snapshot: cannot tokenize a cwd without a basename".into());
    }
    let ctx = Context {
        cwd,
        ..Context::default()
    };
    lines
        .iter()
        .map(|line| {
            if hoh_util_values::js_trim(line).is_empty() {
                return Ok((*line).to_owned());
            }
            let mut value = hoh_util_values::js_json_parse(line).map_err(|e| e.to_string())?;
            map_strings(&mut value, &mut |text, _| {
                tokenize_fixture_string(text, &ctx, &basename)
            });
            Ok(hoh_util_values::js_json_stringify(&value))
        })
        .collect::<Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

/// Replace any absolute spelling whose final segment is the generated cwd basename.
fn tokenize_fixture_string(text: &str, ctx: &Context, basename: &str) -> String {
    let exact = Normalizer::replace_known_cwd(text, ctx);
    let delimiter = |c: char| {
        matches!(
            c,
            '\\' | '/'
                | '\''
                | '"'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | '='
                | '<'
                | '>'
        ) || hoh_util_values::is_js_white_space(c)
    };
    let prefix =
        Regex::new(r#"(?:[A-Za-z]:)?[\\/](?:[^\\/\s<>"]+[\\/])*"#).expect("static pattern");
    let mut out = String::new();
    let mut cursor = 0;
    let bytes = exact.as_str();
    while cursor < bytes.len() {
        // The longest candidate starting here whose basename is followed by a delimiter.
        let start = cursor;
        let mut best = None;
        if let Some(head) = prefix.find_at(bytes, start).filter(|m| m.start() == start) {
            // Candidates: every occurrence of `basename` reachable through whole segments.
            let region_end = bytes[start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
                .map_or(bytes.len(), |i| start + i);
            let _ = head;
            let mut search = start;
            while let Some(at) = bytes[search..region_end].find(basename) {
                let name_start = search + at;
                let name_end = name_start + basename.len();
                let before_ok = bytes[..name_start].ends_with(['\\', '/'])
                    && prefix.is_match(&bytes[start..name_start])
                    && prefix
                        .find(&bytes[start..name_start])
                        .is_some_and(|m| m.start() == 0 && m.end() == name_start - start);
                let after_ok = name_end == bytes.len()
                    || bytes[name_end..].chars().next().is_some_and(delimiter);
                if before_ok && after_ok {
                    best = Some(name_end);
                }
                search = name_start + 1;
            }
        }
        match best {
            Some(end) => {
                out.push_str("{{cwd}}");
                cursor = end;
            }
            None => {
                let ch = bytes[cursor..].chars().next().expect("in range");
                out.push(ch);
                cursor += ch.len_utf8();
            }
        }
    }
    out.replace("/private{{cwd}}", "{{cwd}}")
}
