//! Scenario bodies: the assertions the source snapshot runners make after a run, beyond the
//! generic log/tree/stdout comparisons (`snapshots/session/headless.snapshot.ts`,
//! `snapshots/sdk/sdk.snapshot.ts`). Entry point: [`body_check`].

use crate::{
    compare::{Check, Failure},
    records,
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, time::Duration};

pub mod sdk;
pub mod session;

/// What a scenario body may look at. Built by the runner after the process has finished.
pub struct BodyContext<'a> {
    /// The scenario directory of the corpus (`snapshot.yml`, fixtures).
    pub dir: &'a Path,
    /// The scenario manifest (`snapshot.yml` as JSON).
    pub manifest: &'a Value,
    /// Harvested Session logs of the run, in role order.
    pub actual: &'a [String],
    /// The recorded fixture logs of the scenario, in role order.
    pub expected: &'a [String],
    /// SDK wire notifications of the run, as received (empty outside the SDK suite).
    pub notifications: &'a [Value],
    /// Canonical working directory of the run.
    pub cwd: &'a Path,
    /// Re-reads the persisted Session logs of the run (`persistedSessions(cwd)`).
    pub harvest: &'a dyn Fn() -> crate::Result<Vec<String>>,
    /// How the checked process was launched (only `provider-cwd` launches it again).
    pub launch: Launch<'a>,
}

/// The command line of the checked process.
pub struct Launch<'a> {
    pub hoh: &'a Path,
    pub profile: &'a str,
    pub patches: &'a [std::path::PathBuf],
    pub env: &'a BTreeMap<String, String>,
    pub task: &'a str,
    /// Upper bound for every process a body check starts (the runner's process timeout).
    pub timeout: Duration,
}

/// Run a child process to completion within `limit`. The child leads its own process group; on
/// expiry — and after a normal exit, for leftovers such as a background job that inherited the
/// pipes — the whole group is killed, so no pipe can stay open and nothing waits without a
/// deadline. Unlike the source (`snapshotGit` in `headless.snapshot.ts:906-920` is a `spawnSync`
/// without a timeout, and the provider-cwd launch has its own), every body process is bounded here.
pub(crate) fn run_bounded(
    command: &mut std::process::Command,
    limit: Duration,
    what: &str,
) -> std::result::Result<std::process::Output, Failure> {
    use std::{io::Read, os::unix::process::CommandExt, process::Stdio, sync::mpsc};
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| fail(format!("{what}: {e}")))?;
    let group = rustix::process::Pid::from_raw(child.id() as i32);
    let kill_group = || {
        if let Some(group) = group {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
    };
    fn drain(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = tx.send(bytes);
        });
        rx
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = std::time::Instant::now() + limit;
    // The child is only *observed* exiting (`WNOWAIT`) and reaped after the group was signalled:
    // while it is a zombie its pid, hence the group id, cannot be reused by another process.
    let exited = |child: &mut std::process::Child| -> std::result::Result<bool, Failure> {
        let Some(pid) = group else {
            return child
                .try_wait()
                .map(|s| s.is_some())
                .map_err(|e| fail(format!("{what}: {e}")));
        };
        rustix::process::waitid(
            rustix::process::WaitId::Pid(pid),
            rustix::process::WaitIdOptions::EXITED
                | rustix::process::WaitIdOptions::NOHANG
                | rustix::process::WaitIdOptions::NOWAIT,
        )
        .map(|state| state.is_some())
        .map_err(|e| fail(format!("{what}: {e}")))
    };
    loop {
        if exited(&mut child)? {
            break;
        }
        if std::time::Instant::now() >= deadline {
            kill_group();
            let _ = child.wait();
            return Err(fail(format!(
                "{what}: process timeout after {} ms",
                limit.as_millis()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    kill_group();
    let status = child.wait().map_err(|e| fail(format!("{what}: {e}")))?;
    // The group is dead, so the pipes close; a process that escaped it (`setsid`) cannot hold us.
    let grace = Duration::from_secs(1);
    Ok(std::process::Output {
        status,
        stdout: stdout.recv_timeout(grace).unwrap_or_default(),
        stderr: stderr.recv_timeout(grace).unwrap_or_default(),
    })
}

/// The parts of a scenario body that this module does not carry (the runner keeps neither the
/// captured model requests nor the per-call SDK results), as `sdk.<name>: <part>, …`. A scenario
/// whose carried part passes is still only "passed, body not checked" while this is `Some`.
pub fn unchecked(suite: &str, name: &str) -> Option<String> {
    if suite == "session" && name == "tool-call-turn" {
        // headless.snapshot.ts:1349-1350, 1476-1478: the second full replay with `session.v3.jsonl`
        // as the primary input (exit, stdout, log, headers) is not run; only the tool-result parity is.
        return Some("session.tool-call-turn: replay on the retained V3 input".into());
    }
    if suite != "sdk" {
        return None;
    }
    // Controller checks of `sdk.snapshot.ts` that run inside the protocol driver and are not ported.
    let controller = match name {
        // :880-890 — `approval/request` method and `toMatchObject({sessionId, toolName: 'fixture-write'})`.
        "approval" => Some("approval/request params"),
        // :948-958 — close, relaunch, refusal of `missing-session`, `session.id`.
        "session-resume" => Some("close/resume/missing-session"),
        // :898, :989-1017 — `finalResponse == DONE`, completion order, `session/wait == {}`.
        "subagent-continuable" => Some("completion order, session/wait result"),
        // :893-896, :982-988 — provider-title wait after every turn.
        "session-title-stability" => Some("provider-title waits"),
        // :1018-1023 — `feedback/*` chain of the scenarios run with DSH_SNAPSHOT_FEEDBACK.
        "text-turn" => Some("feedback chain"),
        _ => None,
    };
    let model_requests = matches!(
        name,
        "command-attachments" | "command-identity" | "permission-switch" | "manual-compact"
    );
    let final_response = matches!(
        name,
        "desktop-context"
            | "manual-compact"
            | "permission-switch"
            | "command-identity"
            | "dynamic-tool-updates"
    );
    let parts: Vec<&str> = [
        controller,
        model_requests.then_some("modelRequests"),
        final_response.then_some("expectedFinalResponse"),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| format!("sdk.{name}: {}", parts.join(", ")))
}

/// A body result as the runner reports it: a carried body that passed but leaves parts of the
/// source body unchecked is the `scenario-check` gate (`check … not implemented`), never a clean pass.
pub fn gate(suite: &str, name: &str, result: Check) -> Check {
    result?;
    match unchecked(suite, name) {
        Some(parts) => Err(Failure::new(
            "scenario-check",
            format!("check {parts} not implemented"),
        )),
        None => Ok(()),
    }
}

/// The body check of one scenario, or `None` if the source makes no assertion beyond the common
/// comparisons (or if this module does not carry it).
pub fn body_check(suite: &str, name: &str, ctx: &BodyContext<'_>) -> Option<Check> {
    match suite {
        "session" => session::check(name, ctx),
        "sdk" => sdk::check(name, ctx),
        _ => None,
    }
}

pub(crate) fn fail(message: impl Into<String>) -> Failure {
    Failure::new("body", message)
}

/// `expect(actual).toEqual(expected)` for one labelled value: key order does not matter.
pub(crate) fn expect_eq(label: &str, actual: &Value, expected: &Value) -> Check {
    if actual == expected {
        return Ok(());
    }
    crate::compare::equal("body", label, expected, actual)?;
    Err(fail(format!("{label} mismatch")))
}

pub(crate) fn rows(log: &str) -> std::result::Result<Vec<Value>, Failure> {
    records(log).map_err(fail)
}

pub(crate) fn rows_of(
    logs: &[String],
    index: usize,
    what: &str,
) -> std::result::Result<Vec<Value>, Failure> {
    rows(
        logs.get(index)
            .ok_or_else(|| fail(format!("{what}: no persisted Session {index}")))?,
    )
}

pub(crate) fn of_type<'a>(rows: &'a [Value], kind: &str) -> Vec<&'a Value> {
    rows.iter().filter(|r| r["type"] == kind).collect()
}

/// `tool/result` messages (`event.data.message`).
pub(crate) fn tool_results(rows: &[Value]) -> Vec<&Value> {
    of_type(rows, "tool/result")
        .into_iter()
        .map(|r| &r["data"]["message"])
        .collect()
}

pub(crate) fn text_of(message: &Value) -> String {
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
}

/// `expect(value).toMatchObject(pattern)`: every key of `pattern` matches recursively; arrays
/// must have the same length.
pub(crate) fn matches_object(value: &Value, pattern: &Value) -> bool {
    match (value, pattern) {
        (Value::Object(v), Value::Object(p)) => p
            .iter()
            .all(|(k, p)| v.get(k).is_some_and(|v| matches_object(v, p))),
        (Value::Array(v), Value::Array(p)) => {
            v.len() == p.len() && v.iter().zip(p).all(|(v, p)| matches_object(v, p))
        }
        _ => value == pattern,
    }
}

pub(crate) fn expect_match(label: &str, value: &Value, pattern: &Value) -> Check {
    if matches_object(value, pattern) {
        Ok(())
    } else {
        Err(fail(format!(
            "{label}: expected to match {}; got {}",
            hoh_util_values::js_json_stringify(pattern),
            hoh_util_values::js_json_stringify(value)
        )))
    }
}

pub(crate) fn expect(condition: bool, message: impl FnOnce() -> String) -> Check {
    if condition {
        Ok(())
    } else {
        Err(fail(message()))
    }
}

#[cfg(test)]
mod tests {
    use super::run_bounded;
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn a_background_job_holding_the_pipes_does_not_hang_a_normal_exit() {
        let started = Instant::now();
        let out = run_bounded(
            &mut sh("echo done; sleep 600 & exit 0"),
            Duration::from_millis(100),
            "probe",
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(String::from_utf8_lossy(&out.unwrap().stdout), "done\n");
    }

    #[test]
    fn a_timeout_kills_the_whole_group_and_returns_in_time() {
        let started = Instant::now();
        let failure = run_bounded(
            &mut sh("sleep 600 & sleep 600; echo never"),
            Duration::from_millis(100),
            "probe",
        )
        .unwrap_err();
        assert_eq!(failure.phase, "body");
        assert!(
            failure.reason.contains("process timeout"),
            "{}",
            failure.reason
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn output_and_status_of_a_quick_child_are_returned() {
        let out = run_bounded(
            &mut sh("echo a; echo b >&2; exit 3"),
            Duration::from_secs(5),
            "p",
        )
        .unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(out.stdout, b"a\n");
        assert_eq!(out.stderr, b"b\n");
    }
}
