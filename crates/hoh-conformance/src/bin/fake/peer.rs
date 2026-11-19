use hoh_conformance::{Result, normalize::map_strings, read};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, Write},
    path::{Path, PathBuf},
};

pub fn ensure(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
pub fn parse_file(path: &Path) -> Result<Value> {
    hoh_util_values::js_json_parse(&read(path)?).map_err(|e| e.to_string())
}
fn emit(v: &Value) -> Result<()> {
    writeln!(
        std::io::stdout(),
        "{}",
        hoh_util_values::js_json_stringify(v)
    )
    .map_err(|e| e.to_string())?;
    std::io::stdout().flush().map_err(|e| e.to_string())
}
fn receive(input: &mut impl BufRead) -> Result<Option<Value>> {
    let mut line = String::new();
    if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
        return Ok(None);
    }
    hoh_util_values::js_json_parse(&line)
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn execute(config: &Value) -> Result<u8> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let name = cwd
        .ancestors()
        .nth(2)
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .ok_or("scenario missing")?;
    let suite = cwd
        .ancestors()
        .nth(3)
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .ok_or("suite missing")?;
    let plan = parse_file(
        &Path::new(config["plans"].as_str().ok_or("plans missing")?)
            .join(suite)
            .join(format!("{name}.json")),
    )?;
    super::contract::check(&plan, &cwd)?;
    let live = Live::new(&cwd, suite)?;
    let mut logs = plan["logs"].as_array().ok_or("logs missing")?.clone();
    for (i, log) in logs.iter_mut().enumerate() {
        for row in log.as_array_mut().ok_or("role missing")?.iter_mut() {
            live.value(row);
            super::volatile_clocks(row);
        }
        log[0]["createdAt"] = json!(1_800_000_000_000_u64 + i as u64);
        for (j, row) in log.as_array_mut().unwrap().iter_mut().enumerate().skip(1) {
            row["seq"] = json!(j - 1);
            row["time"] = json!(1_800_000_000_000_u64 + j as u64);
        }
    }
    let created: BTreeMap<String, Value> = logs
        .iter()
        .filter_map(|l| Some((l[0]["id"].as_str()?.to_owned(), l[0]["createdAt"].clone())))
        .collect();
    for row in logs.iter_mut().filter_map(Value::as_array_mut).flatten() {
        if row["type"] == "subagent/catalog"
            && let Some(time) = row["data"]["childId"]
                .as_str()
                .and_then(|id| created.get(id))
        {
            row["data"]["childCreatedAt"] = time.clone();
        }
    }
    let mutation = config["mutation"].as_str().unwrap_or("");
    let model = logs
        .iter()
        .filter_map(Value::as_array)
        .flatten()
        .any(|r| r["type"] == "request/header");
    let mut persisted = logs.clone();
    if suite != "acp" || model {
        mutate(&mut persisted, mutation)?;
    }
    // ACP polls persisted turn boundaries while speaking the protocol. Keep that
    // exchange intact; corrupt the harvested output only once the client closes stdin.
    let deferred = suite == "acp" && !mutation.is_empty();
    write_outputs(
        &plan,
        if deferred { &logs } else { &persisted },
        &cwd,
        if deferred { "" } else { mutation },
    )?;
    let mut wire = plan["wire"].as_array().cloned().unwrap_or_default();
    for frame in &mut wire {
        live.value(frame);
        if frame["method"] == "session.event"
            && frame["params"]["event"]["type"] == "agent/inbox/spliced"
            && let Some(ms) = frame["params"]["event"]["data"]["inserted"].as_array_mut()
        {
            for m in ms {
                if let Some(old) = logs
                    .iter()
                    .filter_map(Value::as_array)
                    .flatten()
                    .filter(|r| r["type"] == "user/message")
                    .map(|r| &r["data"])
                    .find(|old| old["content"] == m["content"] && old["source"] == m["source"])
                {
                    m["id"] = old["id"].clone();
                }
            }
        }
        fn usage(v: &mut Value) {
            match v {
                Value::Array(a) => {
                    for v in a {
                        usage(v);
                    }
                }
                Value::Object(o) => {
                    if o.get("used").is_some_and(|v| v == "{{usedTokens}}") {
                        o.insert("used".into(), json!(173));
                    }
                    for v in o.values_mut() {
                        usage(v);
                    }
                }
                _ => {}
            }
        }
        usage(frame);
    }
    let mut peer = Peer {
        plan: &plan,
        live: &live,
        logs: &logs,
        cwd: &cwd,
        wire,
        cursor: 0,
        input: std::io::BufReader::new(std::io::stdin()),
        queries: 0,
        shutdown: false,
        no_shutdown_reply: config["shutdownWithoutReply"] == format!("{suite}/{name}"),
        permission_count: 0,
    };
    match suite {
        "session" => {
            ensure(
                receive(&mut peer.input)?.is_none(),
                "headless stdin must be closed",
            )?;
            std::io::stdout()
                .write_all(plan["stdout"].as_str().unwrap_or("").as_bytes())
                .map_err(|e| e.to_string())?;
            std::io::stderr()
                .write_all(plan["stderr"].as_str().unwrap_or("").as_bytes())
                .map_err(|e| e.to_string())?;
        }
        "acp" => peer.acp()?,
        "sdk" => peer.sdk()?,
        _ => return Err("unknown suite".into()),
    }
    if deferred {
        write_outputs(&plan, &persisted, &cwd, mutation)?;
    }
    let home = cwd.join(".dsh");
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    std::fs::write(home.join("fake-assertions.json"),hoh_util_values::js_json_stringify(&json!({"argv":true,"environment":true,"stdinEof":true,"shutdown":peer.shutdown,"queries":peer.queries}))).map_err(|e|e.to_string())?;
    Ok(config["exitCodes"][format!("{suite}/{name}")]
        .as_u64()
        .unwrap_or(if suite == "session" {
            plan["exitCode"].as_u64().unwrap_or(0)
        } else {
            0
        }) as u8)
}

struct Live {
    cwd: String,
    suite: String,
    ids: regex::Regex,
    spill: regex::Regex,
}
impl Live {
    fn new(cwd: &Path, suite: &str) -> Result<Self> {
        Ok(Self {
            cwd: cwd.to_string_lossy().into_owned(),
            suite: suite.into(),
            ids: regex::Regex::new(
                r"\{\{(session|message|approval|workflow|command|rpc|retry|id):([1-9][0-9]*)\}\}",
            )
            .map_err(|e| e.to_string())?,
            spill: regex::Regex::new(r"\{\{spillLocator:([^}]+)\}\}").map_err(|e| e.to_string())?,
        })
    }
    fn session(&self, n: u64) -> String {
        if n == 1 && self.suite == "sdk" {
            "fixture-root-session".into()
        } else {
            format!("00000001-0000-4000-8000-{n:012x}")
        }
    }
    fn text(&self, s: &str) -> String {
        let s = s
            .replace("{{cwd}}", &self.cwd)
            .replace("{{sessionId}}", &self.session(1))
            .replace("{{messageId}}", "00000002-0000-4000-8000-000000000001")
            .replace("{{eventTime}}", "1800000000173")
            .replace("{{eventOmittedBytes}}", "173");
        let s = self.spill.replace_all(
            &s,
            "/tmp/dsh-acp-snap-123456789/session-0123456789ab/0123456789ab-${1}",
        );
        self.ids
            .replace_all(&s, |c: &regex::Captures<'_>| {
                let n = c[2].parse::<u64>().unwrap_or(0);
                if &c[1] == "session" {
                    self.session(n)
                } else {
                    let k = [
                        "session", "message", "approval", "workflow", "command", "rpc", "retry",
                        "id",
                    ]
                    .iter()
                    .position(|s| *s == &c[1])
                    .unwrap_or(0)
                        + 1;
                    format!("{k:08x}-0000-4000-8000-{n:012x}")
                }
            })
            .into_owned()
    }
    fn value(&self, v: &mut Value) {
        map_strings(v, &mut |s, _| self.text(s));
    }
}

fn mutate(logs: &mut [Value], mutation: &str) -> Result<()> {
    match mutation {
        "missing" => {
            logs[0].as_array_mut().ok_or("role missing")?.remove(1);
        }
        "reorder" => {
            let rows = logs[0].as_array_mut().ok_or("role missing")?;
            let last = rows.len() - 1;
            if last > 1 {
                rows.swap(1, last);
            }
        }
        "tool" => {
            if let Some(tool) = logs
                .iter_mut()
                .filter_map(Value::as_array_mut)
                .flatten()
                .find(|r| r["type"] == "tool/result")
            {
                if let Some(t) = tool.pointer_mut("/data/message/content/0/text") {
                    *t = json!(format!("{} MUTATED tool", t.as_str().unwrap_or("")));
                } else {
                    tool["data"]["mutated"] = json!("MUTATED tool");
                }
            } else {
                logs[0].as_array_mut().unwrap().push(
                    json!({"type":"fake/tool","seq":999,"time":0,"data":{"text":"MUTATED tool"}}),
                );
            }
        }
        "usage" => {
            fn change(v: &mut Value) -> bool {
                if let Some(u) = v.get_mut("usage").and_then(Value::as_object_mut)
                    && let Some(n) = u.values_mut().find(|v| v.is_number())
                {
                    *n = json!(n.as_f64().unwrap_or(0.0) + 1.0);
                    return true;
                }
                match v {
                    Value::Array(a) => a.iter_mut().any(change),
                    Value::Object(o) => o.values_mut().any(change),
                    _ => false,
                }
            }
            if !logs.iter_mut().any(change) {
                logs[0].as_array_mut().unwrap().push(json!({"type":"fake/usage","seq":999,"time":0,"data":{"usage":{"inputTokens":999}}}));
            }
        }
        "prompt" => {
            let text = logs
                .iter_mut()
                .filter_map(Value::as_array_mut)
                .flatten()
                .find(|r| r["type"] == "system/message")
                .and_then(|r| r.pointer_mut("/data/message/content/0/text"));
            if let Some(text) = text {
                *text = json!(format!("{} MUTATED prompt", text.as_str().unwrap_or("")));
            } else {
                logs[0].as_array_mut().unwrap().push(json!({"type":"system/message","seq":999,"time":0,"data":{"message":{"content":[{"type":"text","text":"MUTATED prompt"}]}}}));
            }
        }
        "writer-v3" => {
            for role in logs {
                role[0]["version"] = json!(3);
            }
        }
        "mixed-generation" => logs.get_mut(1).ok_or("child missing")?[0]["version"] = json!(3),
        _ => {}
    }
    Ok(())
}

fn write_outputs(plan: &Value, logs: &[Value], cwd: &Path, mutation: &str) -> Result<()> {
    for (i, role) in logs.iter().enumerate() {
        let root = if plan["suite"] == "acp" {
            PathBuf::from(std::env::var("DSH_SNAPSHOT_SESSIONS_ROOT").map_err(|e| e.to_string())?)
        } else if plan["name"] == "subagent-dsh-sdk-dynamic-route" && i > 0 {
            cwd.join(".child-dsh/sessions")
        } else {
            cwd.join(".dsh/sessions")
        };
        let root = root
            .join("project")
            .join(role[0]["id"].as_str().ok_or("id missing")?);
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        std::fs::write(
            root.join(
                hoh_session_format::session_format_log_filename(
                    role[0]["version"].as_u64().ok_or("version missing")?,
                )
                .map_err(|e| e.to_string())?,
            ),
            hoh_conformance::jsonl(role.as_array().ok_or("role missing")?),
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(tree) = plan["tree"].as_array() {
        let ignored = match plan["suite"].as_str().unwrap_or("") {
            "sdk" => vec![
                ".agents",
                ".child-dsh",
                ".dsh",
                ".replay-fixtures",
                ".snapshot-patches",
            ],
            "acp" => vec![
                ".agents",
                ".dsh",
                ".dsh-profile-patches",
                ".dsh-snapshot-stream-ready",
            ],
            _ if plan["manifest"]["workspace"]["setup"] == "git-worktree" => {
                vec![".dsh", ".snapshot-patches", ".git"]
            }
            _ => vec![".agents", ".dsh", ".snapshot-patches"],
        };
        for entry in std::fs::read_dir(cwd).map_err(|e| e.to_string())? {
            let e = entry.map_err(|e| e.to_string())?;
            if ignored.iter().any(|s| e.file_name() == *s) {
                continue;
            }
            if e.file_type().map_err(|e| e.to_string())?.is_dir() {
                std::fs::remove_dir_all(e.path())
            } else {
                std::fs::remove_file(e.path())
            }
            .map_err(|e| e.to_string())?;
        }
        for entry in tree {
            let p = cwd.join(entry["path"].as_str().ok_or("tree path missing")?);
            std::fs::create_dir_all(p.parent().ok_or("tree parent missing")?)
                .map_err(|e| e.to_string())?;
            match entry["kind"].as_str().unwrap_or("") {
                "text" => std::fs::write(p, entry["content"].as_str().ok_or("content missing")?)
                    .map_err(|e| e.to_string())?,
                "binary" => {
                    use base64::Engine;
                    std::fs::write(
                        p,
                        base64::engine::general_purpose::STANDARD
                            .decode(entry["base64"].as_str().ok_or("bytes missing")?)
                            .map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                }
                "empty-directory" => std::fs::create_dir_all(p).map_err(|e| e.to_string())?,
                "symlink" => {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(
                        entry["target"].as_str().ok_or("link target missing")?,
                        p,
                    )
                    .map_err(|e| e.to_string())?;
                }
                _ => return Err("unknown tree kind".into()),
            }
        }
    }
    if mutation == "tree" {
        std::fs::write(cwd.join("MUTATED-extra-file"), "unexpected\n")
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

struct Peer<'a> {
    plan: &'a Value,
    live: &'a Live,
    logs: &'a [Value],
    cwd: &'a Path,
    wire: Vec<Value>,
    cursor: usize,
    input: std::io::BufReader<std::io::Stdin>,
    queries: usize,
    shutdown: bool,
    no_shutdown_reply: bool,
    permission_count: usize,
}
impl Peer<'_> {
    fn acp(&mut self) -> Result<()> {
        for step in self.plan["script"]["steps"]
            .as_array()
            .ok_or("ACP steps missing")?
        {
            let op = step["op"].as_str().unwrap_or("");
            if ["waitForTurnEnd", "waitForFile"].contains(&op) {
                continue;
            }
            let req = receive(&mut self.input)?.ok_or("ACP request missing")?;
            let method = match op {
                "initialize" => "initialize",
                "newSession" | "newSessionExpectError" => "session/new",
                _ => "session/prompt",
            };
            ensure(req["method"] == method, format!("expected ACP {method}"))?;
            if op == "initialize" {
                ensure(
                    req["params"] == json!({"protocolVersion":1,"clientCapabilities":{}}),
                    "ACP initialize mismatch",
                )?;
            } else if method == "session/new" {
                let mut p = json!({"cwd":self.cwd.to_string_lossy(),"mcpServers":[]});
                if let Some(extra) = step.get("additionalDirectories") {
                    p["additionalDirectories"] = extra.clone();
                }
                ensure(req["params"] == p, "ACP newSession mismatch")?;
            } else {
                let prompt = if op == "promptContent" {
                    step["content"].clone()
                } else {
                    json!([{"type":"text","text":step["text"]}])
                };
                ensure(
                    req["params"] == json!({"sessionId":self.live.session(1),"prompt":prompt}),
                    "ACP prompt mismatch",
                )?;
                self.queries += 1;
                if op == "promptAndCancel" {
                    if let Some(file) = step["waitForFile"]["path"].as_str() {
                        let p = self.cwd.join(file);
                        if !p.exists() {
                            std::fs::write(p, "").map_err(|e| e.to_string())?;
                        }
                    }
                    let cancel = receive(&mut self.input)?.ok_or("cancel missing")?;
                    ensure(
                        cancel["method"] == "session/cancel"
                            && cancel["params"]["sessionId"] == self.live.session(1),
                        "cancel mismatch",
                    )?;
                }
            }
            loop {
                let mut f = self
                    .wire
                    .get(self.cursor)
                    .ok_or("ACP recorded response missing")?
                    .clone();
                self.cursor += 1;
                if f.get("method").is_some() && f.get("id").is_some() {
                    // Agent and client requests have independent id spaces; retain the
                    // recorded reuse (e.g. agent id 1 after initialize response id 1).
                    emit(&f)?;
                    let answer = receive(&mut self.input)?.ok_or("permission answer missing")?;
                    let expected = if let Some(kind) = self.plan["script"]["permissionAnswers"]
                        [self.permission_count]["kind"]
                        .as_str()
                    {
                        let option = f["params"]["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|o| o["kind"] == kind)
                            .ok_or("permission kind not offered")?;
                        json!({"outcome":{"outcome":"selected","optionId":option["optionId"]}})
                    } else {
                        json!({"outcome":{"outcome":"cancelled"}})
                    };
                    self.permission_count += 1;
                    ensure(
                        answer["id"] == f["id"] && answer["result"] == expected,
                        "permission answer mismatch",
                    )?;
                    continue;
                }
                let response = f.get("id").is_some();
                if response {
                    f["id"] = req["id"].clone();
                }
                emit(&f)?;
                if response {
                    break;
                }
            }
        }
        ensure(self.cursor == self.wire.len(), "ACP frames not exhausted")?;
        ensure(
            receive(&mut self.input)?.is_none(),
            "ACP expected stdin EOF",
        )
    }
    fn sdk(&mut self) -> Result<()> {
        let init = receive(&mut self.input)?.ok_or("SDK initialize missing")?;
        let mut params = json!({"cwd":self.cwd.to_string_lossy(),"provider":self.plan["route"]["provider"],"model":self.plan["route"]["model"]});
        if self.plan["name"] == "approval" {
            params["approvalRequests"] = json!(true);
        }
        ensure(
            init["method"] == "initialize" && init["params"] == params,
            "SDK initialize mismatch",
        )?;
        emit(
            &json!({"jsonrpc":"2.0","id":init["id"],"result":{"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"0.0.1"}}}),
        )?;
        let queries: Vec<_> = self.plan["actions"]
            .as_array()
            .ok_or("actions missing")?
            .iter()
            .filter(|a| a["content"].is_array())
            .collect();
        let mut boundaries = Vec::new();
        if !self.wire.is_empty() {
            for action in &queries {
                let id = action["messageId"].as_str().map(|id| self.live.text(id));
                let command = action["commandId"].as_str().map(|id| self.live.text(id));
                let found = self
                    .wire
                    .iter()
                    .enumerate()
                    .skip(boundaries.last().copied().unwrap_or(0))
                    .find(|(_, n)| {
                        let e = &n["params"]["event"];
                        if let Some(id) = &id {
                            e["type"] == "agent/inbox/spliced"
                                && e["data"]["inserted"]
                                    .as_array()
                                    .is_some_and(|ms| ms.iter().any(|m| m["id"] == *id))
                        } else {
                            e["type"] == "command/run"
                                && command
                                    .as_ref()
                                    .is_some_and(|id| e["data"]["commandId"] == *id)
                        }
                    })
                    .map(|(i, _)| i)
                    .ok_or("SDK wire lacks query admission boundary")?;
                boundaries.push(found);
            }
        }
        for (index, action) in queries.iter().enumerate() {
            let req = receive(&mut self.input)?.ok_or("SDK query missing")?;
            let mut content = action["content"].clone();
            self.live.value(&mut content);
            for block in content.as_array_mut().ok_or("input missing")? {
                if block["type"] == "image" {
                    let a = self.plan["manifest"]["input"]["attachments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|a| a["id"] == block["attachment"]["attachmentId"])
                        .ok_or("attachment missing")?;
                    *block = json!({"type":"image","data":a["data"],"mimeType":a["mediaType"]});
                }
            }
            ensure(
                req["method"] == "session/query"
                    && req["params"]
                        == json!({"sessionId":"fixture-root-session","contentBlocks":content}),
                format!("SDK query {} content mismatch", index + 1),
            )?;
            self.queries += 1;
            let receipt = action["messageId"]
                .as_str()
                .map(|s| self.live.text(s))
                .unwrap_or_else(|| format!("receipt-{index}"));
            let cmd = action["commandId"].as_str().map(|s| self.live.text(s));
            emit(
                &json!({"jsonrpc":"2.0","id":req["id"],"result":if let Some(cmd)=cmd{json!({"kind":"command","execution":{"commandId":cmd,"kind":"success","text":""}})}else{json!({"kind":"prompt","messageId":receipt})}}),
            )?;
            if self.wire.is_empty() {
                self.synthetic(action, &receipt, index == 0)?;
            } else {
                let end = boundaries
                    .get(index + 1)
                    .copied()
                    .unwrap_or(self.wire.len());
                for n in &self.wire[self.cursor..end] {
                    let mut n = n.clone();
                    n["jsonrpc"] = json!("2.0");
                    emit(&n)?;
                }
                self.cursor = end;
            }
        }
        if ["serial-created", "subagent-continuable"]
            .contains(&self.plan["name"].as_str().unwrap_or(""))
        {
            let req = receive(&mut self.input)?.ok_or("session/wait missing")?;
            ensure(
                req["method"] == "session/wait"
                    && req["params"]["sessionId"] == "fixture-root-session",
                "SDK session/wait mismatch",
            )?;
            emit(&json!({"jsonrpc":"2.0","id":req["id"],"result":{}}))?;
        }
        let req = receive(&mut self.input)?.ok_or("SDK shutdown missing")?;
        ensure(req["method"] == "shutdown", "SDK expected shutdown")?;
        self.shutdown = true;
        if !self.no_shutdown_reply {
            emit(&json!({"jsonrpc":"2.0","id":req["id"],"result":{}}))?;
        }
        ensure(
            receive(&mut self.input)?.is_none(),
            "SDK expected stdin EOF after shutdown",
        )
    }
    fn synthetic(&self, action: &Value, receipt: &str, first: bool) -> Result<()> {
        if first {
            for role in self.logs.iter().skip(1) {
                emit(
                    &json!({"jsonrpc":"2.0","method":"subagent.started","params":{"parentSessionId":"fixture-root-session","childSessionId":role[0]["id"]}}),
                )?;
            }
        }
        emit(
            &json!({"jsonrpc":"2.0","method":"session.event","params":{"sessionId":"fixture-root-session","event":{"type":"agent/inbox/spliced","data":{"inserted":[{"id":receipt}]}}}}),
        )?;
        let turn = action["turn"].as_u64();
        for row in self.logs[0]
            .as_array()
            .ok_or("parent rows missing")?
            .iter()
            .skip(1)
        {
            if turn.is_some() && row["data"]["turn"].as_u64() == turn {
                emit(
                    &json!({"jsonrpc":"2.0","method":"session.event","params":{"sessionId":"fixture-root-session","event":row}}),
                )?;
            }
        }
        for a in self.plan["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["content"].is_null())
        {
            if let Some(t) = a["turn"].as_u64() {
                emit(
                    &json!({"jsonrpc":"2.0","method":"session.event","params":{"sessionId":"fixture-root-session","event":{"type":"turn/end","data":{"turn":t,"reason":{"kind":"completed"}}}}}),
                )?;
            }
        }
        emit(
            &json!({"jsonrpc":"2.0","method":"session.status","params":{"sessionId":"fixture-root-session","status":"idle"}}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reorder_requires_two_body_rows() {
        let two = json!([{"type":"session"},{"type":"only"}]);
        let mut logs = vec![two.clone()];
        mutate(&mut logs, "reorder").unwrap();
        assert_eq!(logs[0], two);
        let mut logs = vec![json!([{"type":"session"},{"type":"first"},{"type":"last"}])];
        mutate(&mut logs, "reorder").unwrap();
        assert_eq!(logs[0][1]["type"], "last");
        assert_eq!(logs[0][2]["type"], "first");
    }
}
