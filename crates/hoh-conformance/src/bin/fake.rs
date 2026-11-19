//! Explicitly selected executable double; receives the real runner's argv/environment/stdin.
use serde_json::{Value, json};
use std::{io::Write, process::ExitCode};
#[path = "fake/contract.rs"]
mod contract;
#[path = "fake/drop_in.rs"]
mod drop_in;
#[path = "fake/peer.rs"]
mod peer;

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
    match drop_in::execute() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "fake: {e}");
            ExitCode::from(97)
        }
    }
}
