//! Test-only recorded-output writer. Never selected by the real runner implicitly.
use hoh_conformance::{Result, compare, corpus, normalize::map_strings};
use serde_json::{Value, json};
use std::{io::Write, path::PathBuf, process::ExitCode};

fn execute() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut refs = None;
    let mut suite = None;
    let mut scenario = None;
    let mut out = None;
    while let Some(arg) = args.next() {
        let value = args.next().ok_or("missing fake argument")?;
        match arg.as_str() {
            "--refs" => refs = Some(PathBuf::from(value)),
            "--suite" => suite = Some(value),
            "--scenario" => scenario = Some(value),
            "--out" => out = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown fake argument {arg}")),
        }
    }
    let refs = refs.ok_or("missing refs")?;
    let out = out.ok_or("missing out")?;
    let all = corpus::discover(&refs)?;
    let selected: Vec<_> = all
        .iter()
        .filter(|s| {
            suite.as_ref().is_none_or(|v| v == &s.suite)
                && scenario.as_ref().is_none_or(|v| v == &s.name)
        })
        .collect();
    if selected.is_empty() {
        return Err("unknown fake scenario".into());
    }
    let token = regex::Regex::new(
        r"\{\{(session|message|approval|workflow|command|rpc|retry|id):([1-9][0-9]*)\}\}",
    )
    .map_err(|e| e.to_string())?;
    let spill_token =
        regex::Regex::new(r"\{\{spillLocator:([^}]+)\}\}").map_err(|e| e.to_string())?;
    for s in selected {
        let out = out.join(&s.suite).join(&s.name);
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        let cwd = out.join("workspace");
        std::fs::create_dir_all(&cwd).map_err(|e| e.to_string())?;
        let mut logs = compare::hydrate(&all, s, &s.expected_logs()?)?;
        let live = |text: &str| -> String {
            let text = text.replace("{{cwd}}", &cwd.to_string_lossy());
            let text = spill_token.replace_all(
                &text,
                "/tmp/dsh-acp-snap-123456789/session-0123456789ab/0123456789ab-${1}",
            );
            let text = text
                .replace("{{eventTime}}", "1800000000173")
                .replace("{{eventOmittedBytes}}", "173");
            token
                .replace_all(&text, |c: &regex::Captures<'_>| {
                    let kind = [
                        "session", "message", "approval", "workflow", "command", "rpc", "retry",
                        "id",
                    ]
                    .iter()
                    .position(|s| *s == &c[1])
                    .unwrap_or(0)
                        + 1;
                    let ordinal = c[2].parse::<u64>().unwrap_or(0);
                    format!("{kind:08x}-0000-4000-8000-{ordinal:012x}")
                })
                .into_owned()
        };
        let sessions = out.join("sessions");
        let creation_times: std::collections::HashMap<String, u64> = logs
            .iter()
            .enumerate()
            .map(|(i, log)| {
                (
                    live(log[0]["id"].as_str().unwrap_or("")),
                    1_800_000_000_000 + i as u64,
                )
            })
            .collect();
        for (i, log) in logs.iter_mut().enumerate() {
            for row in log.iter_mut() {
                map_strings(row, &mut |s, _| live(s));
                volatile_clocks(row);
                if row["type"] == "subagent/catalog"
                    && let Some(created) = row["data"]["childId"]
                        .as_str()
                        .and_then(|id| creation_times.get(id))
                {
                    row["data"]["childCreatedAt"] = json!(created);
                }
            }
            // Real creation order and envelopes; normalization must remove only the volatile fields.
            log[0]["createdAt"] = json!(1_800_000_000_000_u64 + i as u64);
            for (j, row) in log.iter_mut().enumerate().skip(1) {
                row["seq"] = json!(j - 1);
                row["time"] = json!(1_800_000_000_000_u64 + j as u64);
            }
            let id = log[0]["id"].as_str().ok_or("fake header lacks id")?;
            let parent = sessions.join("project").join(id);
            std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
            let version = log[0]["version"].as_u64().unwrap_or(4);
            log[0]["version"] = json!(version);
            std::fs::write(
                parent.join(
                    hoh_session_format::session_format_log_filename(version)
                        .map_err(|e| e.to_string())?,
                ),
                hoh_conformance::jsonl(log),
            )
            .map_err(|e| e.to_string())?;
        }
        let fixtures = s.fixtures()?;
        let fixture = hoh_conformance::records(&hoh_conformance::read(&fixtures[0])?)?;
        let stdout = if s.suite == "session" {
            compare::final_text(&fixture) + "\n"
        } else {
            let file = s.dir.join(if s.suite == "acp" {
                "stdout.expected.jsonl"
            } else if s.manifest.get("sessionFormat").is_some() {
                "notifications.current.expected.jsonl"
            } else {
                "notifications.expected.jsonl"
            });
            if file.is_file() {
                let raw = hoh_conformance::read(&file)?;
                let mut frames = hoh_conformance::records(&raw)?;
                for frame in &mut frames {
                    map_strings(frame, &mut |text, _| {
                        live(text)
                            .replace("{{sessionId}}", "00000001-0000-4000-8000-000000000001")
                            .replace("{{messageId}}", "00000002-0000-4000-8000-000000000001")
                    });
                    if let Some(id) = frame.get_mut("id").and_then(|v| v.as_u64().map(|n| (v, n))) {
                        *id.0 = json!(id.1 * 7 + 101);
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
                hoh_conformance::jsonl(&frames)
            } else {
                String::new()
            }
        };
        let stderr = if s.suite == "session" {
            compare::stderr(&fixture)?
        } else {
            String::new()
        };
        for (file, text) in [("stdout.txt", stdout), ("stderr.txt", stderr)] {
            std::fs::write(out.join(file), text).map_err(|e| e.to_string())?;
        }
        let meta: Value = json!({"exitCode":if s.suite=="session"{compare::expected_exit(&fixture)}else{0},"cwd":cwd.to_string_lossy()});
        std::fs::write(
            out.join("meta.json"),
            hoh_util_values::js_json_stringify(&meta),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn volatile_clocks(row: &mut Value) {
    fn clocks(value: Option<&mut Value>, keys: &[&str]) {
        if let Some(value) = value {
            for key in keys {
                if let Some(number) = value.get_mut(*key).filter(|v| v.is_number()) {
                    *number = json!(1_800_000_000_173_u64);
                }
            }
        }
    }
    match row["type"].as_str().unwrap_or("") {
        "hook/result" => clocks(row.get_mut("data"), &["durationMs"]),
        "job/registered" | "job/settled" => {
            clocks(row.get_mut("data"), &["startedAt", "finishedAt"])
        }
        "goal/change" => clocks(row.get_mut("data"), &["createdAt", "updatedAt"]),
        "feedback/message-put" => {
            clocks(row.pointer_mut("/data/item"), &["createdAt", "updatedAt"])
        }
        "plugin:timeline/tree" => {
            clocks(
                row.pointer_mut("/data/tree"),
                &["createdAt", "lastTaskUpdate"],
            );
            if let Some(tasks) = row
                .pointer_mut("/data/tree/tasks")
                .and_then(Value::as_array_mut)
            {
                for task in tasks {
                    clocks(Some(task), &["createdAt", "updatedAt"]);
                }
            }
            clocks(row.pointer_mut("/data/receipt"), &["at", "previousAt"]);
        }
        "tool/result" => clocks(
            row.pointer_mut("/data/meta/timeline"),
            &["at", "previousAt"],
        ),
        "session-log-deepseek/delivery-accepted" => {
            if row["data"]["sessionFormatVersion"] == "{{sourceSessionFormatVersion}}" {
                row["data"]["sessionFormatVersion"] = json!(4);
            }
        }
        "assistant/message" | "assistant/attempt" => {
            if let Some(stream) = row
                .pointer_mut("/data/stream")
                .and_then(Value::as_array_mut)
            {
                for entry in stream {
                    clocks(Some(&mut *entry), &["time", "time0"]);
                    if let Some(dt) = entry.get_mut("dt").and_then(Value::as_array_mut) {
                        for (i, v) in dt.iter_mut().enumerate() {
                            *v = json!(i + 1);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}
fn main() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "fake: {e}");
            ExitCode::FAILURE
        }
    }
}
