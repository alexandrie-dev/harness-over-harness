//! The durable-log waits of `harness.ts` (`waitForPersisted*`): each polls the harvested session
//! logs of a sessions root until a predicate over the raw JSONL holds, or fails with its own text.
use crate::{Result, records};
use serde_json::Value;
use std::{path::Path, time::Duration};

const POLL: Duration = Duration::from_millis(10);

/// The log text before the last newline (a record being written is not a record yet).
fn complete(content: &str) -> &str {
    &content[..content.rfind('\n').map_or(0, |i| i + 1)]
}

/// `latestTurnIsClosed`.
pub fn latest_turn_is_closed(content: &str) -> bool {
    let done = complete(content);
    done.rfind("\n{\"type\":\"turn/end\",") > done.rfind("\n{\"type\":\"turn/start\",")
}

/// `latestTitleFollowsTurnEnd`.
pub fn latest_title_follows_turn_end(content: &str) -> bool {
    latest_event_follows_turn_end(content, "session/title")
}

/// `latestEventFollowsTurnEnd`.
pub fn latest_event_follows_turn_end(content: &str, kind: &str) -> bool {
    let done = complete(content);
    done.rfind("\n{\"type\":\"turn/end\",")
        .is_some_and(|end| done.rfind(&format!("\n{{\"type\":\"{kind}\",")) > Some(end))
}

/// `latestOpenTurn`: the open turn's number, validated; `None` while no turn is open.
pub fn latest_open_turn(content: &str) -> Result<Option<u64>> {
    let done = complete(content);
    let Some(start) = done.rfind("\n{\"type\":\"turn/start\",") else {
        return Ok(None);
    };
    if Some(start) <= done.rfind("\n{\"type\":\"turn/end\",") {
        return Ok(None);
    }
    let line = done[start + 1..].split('\n').next().unwrap_or_default();
    let record: Value = hoh_util_values::js_json_parse(line).map_err(|e| e.to_string())?;
    match record["data"]["turn"].as_u64() {
        Some(turn) if turn >= 1 => Ok(Some(turn)),
        _ => Err("snapshot-harness: invalid persisted turn/start record".into()),
    }
}

/// `hasClosedTurn`.
pub fn has_closed_turn(content: &str, turn: u64) -> Result<bool> {
    Ok(records(content)?
        .iter()
        .any(|r| r["type"] == "turn/end" && r["data"]["turn"] == turn))
}

/// `hasRequestHeaderAfterDescriptor`.
pub fn has_request_header_after_descriptor(content: &str) -> Result<bool> {
    let rows = records(complete(content))?;
    let Some(at) = rows
        .iter()
        .rposition(|r| r["type"] == "subagent/descriptor")
    else {
        return Ok(false);
    };
    Ok(rows[at + 1..].iter().any(|r| r["type"] == "request/header"))
}

fn goal_phase_reached(content: &str, phase: &str) -> Result<bool> {
    Ok(records(content)?
        .iter()
        .any(|r| r["type"] == "goal/change" && r["data"]["goal"]["phase"] == phase))
}

fn inbox_message_inserted(content: &str, text: &str) -> Result<bool> {
    Ok(records(content)?.iter().any(|r| {
        r["type"] == "agent/inbox/spliced"
            && r["data"]["inserted"].as_array().is_some_and(|rows| {
                rows.iter().any(|m| {
                    m["content"].as_array().is_some_and(|blocks| {
                        blocks.iter().any(|b| {
                            b["type"] == "text"
                                && b["text"].as_str().is_some_and(|t| t.contains(text))
                        })
                    })
                })
            })
    }))
}

/// Which harvested log a wait watches.
pub enum Target<'a> {
    Session(&'a str),
    /// The Nth harvested log (children in harvest order; the parent is 0).
    Index(usize),
}

/// Poll the harvest until `check` holds for the target log. `Ok(false)` and "no such log yet" keep
/// waiting; an `Err` from `check` is a scenario bug and ends the wait at once.
pub async fn wait_persisted(
    root: &Path,
    target: Target<'_>,
    timeout: Duration,
    message: String,
    check: impl Fn(&str) -> Result<bool>,
) -> Result<()> {
    let wait = async {
        loop {
            // Directory walk and file reads must not stall the runtime's workers.
            let owned = root.to_path_buf();
            let logs = tokio::task::spawn_blocking(move || {
                crate::runner::harvest(&owned, "acp").unwrap_or_default()
            })
            .await
            .map_err(|e| e.to_string())?;
            let log = match target {
                Target::Index(i) => logs.get(i),
                Target::Session(id) => logs.iter().find(|log| {
                    records(log)
                        .ok()
                        .and_then(|r| r.first().cloned())
                        .is_some_and(|h| h["id"] == id)
                }),
            };
            if let Some(log) = log
                && check(log)?
            {
                return Ok(());
            }
            tokio::time::sleep(POLL).await;
        }
    };
    tokio::time::timeout(timeout, wait)
        .await
        .map_err(|_| message)?
}

pub async fn turn_start(
    root: &Path,
    id: &str,
    timeout: Duration,
    minimum: Option<u64>,
) -> Result<()> {
    let detail = minimum.map_or("turn/start".to_owned(), |n| {
        format!("turn/start at or beyond turn {n}")
    });
    let message = format!(
        "snapshot-harness: session \"{id}\" did not persist {detail} within {}ms",
        timeout.as_millis()
    );
    wait_persisted(root, Target::Session(id), timeout, message, |log| {
        Ok(latest_open_turn(log)?.is_some_and(|open| minimum.is_none_or(|m| open >= m)))
    })
    .await
}

pub async fn turn_end(root: &Path, id: &str, timeout: Duration) -> Result<()> {
    let message = format!(
        "snapshot-harness: session \"{id}\" did not persist turn/end within {}ms",
        timeout.as_millis()
    );
    wait_persisted(root, Target::Session(id), timeout, message, |log| {
        Ok(latest_turn_is_closed(log))
    })
    .await
}

pub async fn child_turn_end(
    root: &Path,
    child: usize,
    timeout: Duration,
    minimum: u64,
) -> Result<()> {
    let message = format!(
        "snapshot-harness: subagent child #{child} did not persist closed turn {minimum} within {}ms",
        timeout.as_millis()
    );
    wait_persisted(root, Target::Index(child), timeout, message, |log| {
        Ok(latest_turn_is_closed(log)
            && has_request_header_after_descriptor(log)?
            && has_closed_turn(log, minimum)?)
    })
    .await
}

pub async fn goal_phase(root: &Path, id: &str, phase: &str, timeout: Duration) -> Result<()> {
    let message = format!(
        "snapshot-harness: session \"{id}\" did not persist goal phase \"{phase}\" within {}ms",
        timeout.as_millis()
    );
    wait_persisted(root, Target::Session(id), timeout, message, |log| {
        goal_phase_reached(log, phase)
    })
    .await
}

pub async fn inbox_message(root: &Path, id: &str, text: &str, timeout: Duration) -> Result<()> {
    let message = format!(
        "snapshot-harness: session \"{id}\" did not persist expected inbox message within {}ms",
        timeout.as_millis()
    );
    wait_persisted(root, Target::Session(id), timeout, message, |log| {
        inbox_message_inserted(log, text)
    })
    .await
}

pub async fn title_after_turn_end(root: &Path, id: &str, timeout: Duration) -> Result<()> {
    event_after_turn_end(root, id, "session/title", timeout).await
}

pub async fn event_after_turn_end(
    root: &Path,
    id: &str,
    kind: &str,
    timeout: Duration,
) -> Result<()> {
    let message = format!(
        "snapshot-harness: session \"{id}\" did not persist {kind} after turn/end within {}ms",
        timeout.as_millis()
    );
    wait_persisted(root, Target::Session(id), timeout, message, |log| {
        Ok(latest_event_follows_turn_end(log, kind))
    })
    .await
}
