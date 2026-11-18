use serde_json::{Value, json};
use std::{fs, path::Path};

#[derive(Clone)]
pub struct Recording {
    pub events: Vec<Value>,
    pub first: usize,
}
#[derive(Clone, Debug)]
pub struct Action {
    pub end: usize,
    pub command: Option<String>,
    pub command_id: Option<String>,
    pub command_done: Option<usize>,
    pub has_turn: bool,
    pub input: Option<Value>,
    pub message_id: Option<String>,
}
pub fn kind(event: &Value) -> &str {
    event["type"].as_str().unwrap_or("")
}
fn inserted_user(event: &Value) -> Option<&Value> {
    event["data"]["inserted"]
        .as_array()?
        .iter()
        .find(|m| m["source"]["kind"] == "user")
}
impl Recording {
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut rows = text
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(|s| serde_json::from_str::<Value>(s).map_err(|e| e.to_string()));
        let header = rows.next().ok_or("empty recording")??;
        if header["type"] != "session" || header["version"] != 4 {
            return Err("stub requires a V4 session recording".into());
        }
        let events: Vec<Value> = rows.collect::<Result<_, _>>()?;
        let first = events
            .iter()
            .position(|e| {
                (kind(e) == "agent/inbox/spliced" && inserted_user(e).is_some())
                    || kind(e) == "command/run"
            })
            .ok_or("recording has no caller input")?;
        Ok(Self { events, first })
    }
    pub fn action(&self, start: usize) -> Result<Action, String> {
        let event = self.events.get(start).ok_or("recording exhausted")?;
        let is_command = kind(event) == "command/run";
        let command_id =
            is_command.then(|| event["data"]["commandId"].as_str().unwrap_or("").to_owned());
        let command = if is_command {
            Some(format!(
                "/{}{}",
                event["data"]["name"]
                    .as_str()
                    .ok_or("command name missing")?,
                event["data"]["args"]
                    .as_str()
                    .ok_or("standalone command args missing")?
            ))
        } else {
            None
        };
        let done = if let Some(id) = &command_id {
            Some(
                (start..self.events.len())
                    .find(|i| {
                        kind(&self.events[*i]) == "command/done"
                            && self.events[*i]["data"]["commandId"] == *id
                    })
                    .ok_or("command/done missing")?,
            )
        } else {
            None
        };
        let command_input = done.is_some_and(|done| {
            self.events[start..=done]
                .iter()
                .any(|e| kind(e) == "agent/inbox/spliced" && inserted_user(e).is_some())
        });
        let has_turn = !is_command || command_input;
        let end = if has_turn {
            (start..self.events.len())
                .find(|i| kind(&self.events[*i]) == "turn/end")
                .ok_or("turn/end missing")?
                + 1
        } else {
            done.unwrap() + 1
        };
        let input_message = self.events[start..end].iter().find_map(|e| {
            if kind(e) == "agent/inbox/spliced" {
                inserted_user(e)
            } else {
                None
            }
        });
        Ok(Action {
            end,
            command,
            command_id,
            command_done: done,
            has_turn,
            input: input_message.map(|m| m["content"].clone()),
            message_id: input_message
                .and_then(|m| m["id"].as_str())
                .map(str::to_owned),
        })
    }
    pub fn wire_event(&self, index: usize, session: &str, cwd: &str) -> Value {
        let row = &self.events[index];
        let mut event = json!({"type":row["type"],"seq":index,"time":0,"data":row["data"]});
        for (key, value) in row.as_object().expect("recording event object") {
            if !matches!(key.as_str(), "type" | "data" | "seq" | "time") {
                event[key] = value.clone()
            }
        }
        materialize(&mut event, session, cwd);
        normalize_times(&mut event);
        event
    }
    pub fn tool_result(&self, from: usize, call_id: &Value) -> Option<&Value> {
        self.events[from..]
            .iter()
            .find(|e| kind(e) == "tool/result" && e["data"]["message"]["toolCallId"] == *call_id)
    }
}
pub fn materialize(value: &mut Value, session: &str, cwd: &str) {
    match value {
        Value::String(text) => {
            *text = text
                .replace("{{session:1}}", session)
                .replace("{{cwd}}", cwd)
        }
        Value::Array(items) => {
            for item in items {
                materialize(item, session, cwd)
            }
        }
        Value::Object(items) => {
            for item in items.values_mut() {
                materialize(item, session, cwd)
            }
        }
        _ => {}
    }
}
fn normalize_times(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_times(item)
            }
        }
        Value::Object(items) => {
            for (key, item) in items {
                if matches!(key.as_str(), "time" | "time0") {
                    *item = json!(0)
                } else {
                    normalize_times(item)
                }
            }
        }
        _ => {}
    }
}
