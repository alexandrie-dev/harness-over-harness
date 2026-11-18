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
}

impl Normalizer {
    pub fn new() -> Result<Self> {
        fn re(s: &str) -> Result<Regex> {
            Regex::new(s).map_err(|e| e.to_string())
        }
        Ok(Self {
            uuid: re(r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")?,
            short_job: re(r"(?i)-[0-9a-f]{16}$")?,
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
            if let Some(header) = log.first() {
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

    pub fn string(&self, text: &str, ctx: &Context, legacy: bool) -> String {
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
        out = out.replace("/private{{cwd}}", "{{cwd}}");
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
                if key == Some("path") {
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

    pub fn logs(&self, raw: &[String], ctx: &Context) -> Result<Logs> {
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
            if let Some(o) = header.as_object_mut() {
                o.shift_remove("version");
            }
            for event in log.iter_mut().skip(1) {
                if event["type"] == "session-log-deepseek/delivery-accepted"
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
                    if record["type"] == "system/message"
                        && let Some(text) = record.pointer_mut("/data/message/content/0/text")
                    {
                        *text = json!("{{system}}");
                    }
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
        match kind.as_str() {
            "hook/result" => zero(&mut record["data"], &["durationMs"], false),
            "job/registered" | "job/settled" => {
                zero(&mut record["data"], &["startedAt", "finishedAt"], false)
            }
            "goal/change" => zero(&mut record["data"], &["createdAt", "updatedAt"], false),
            "subagent/catalog" => zero(&mut record["data"], &["childCreatedAt"], false),
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

fn scrub_bulk(record: &mut Value, retain_names: bool) {
    if record["type"] == "system/message"
        && let Some(text) = record
            .pointer_mut("/data/message/content/0/text")
            .filter(|v| v.is_string())
    {
        *text = json!("{{system}}");
    }
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
