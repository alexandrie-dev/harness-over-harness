//! Scenario bodies (`src/body`): each check passes on the recorded fixtures of the corpus and
//! fails on a corrupted copy.

use hoh_conformance::body::{BodyContext, Launch, body_check};
use hoh_conformance::compare::Check;
use hoh_conformance::records;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

fn refs() -> PathBuf {
    std::env::var_os("HOH_REFS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"))
}

fn scenario_dir(suite: &str, name: &str) -> PathBuf {
    refs().join("snapshots").join(suite).join(name)
}

/// `session.v4.jsonl`, `session.1.v4.jsonl`, … in role order.
fn fixtures(suite: &str, name: &str, cwd: &str) -> Vec<String> {
    let dir = scenario_dir(suite, name);
    let mut files: Vec<(u32, PathBuf)> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let path = e.unwrap().path();
            let file = path.file_name()?.to_str()?.to_owned();
            let middle = file.strip_prefix("session")?.strip_suffix(".v4.jsonl")?;
            let index = match middle {
                "" => 0,
                other => other.strip_prefix('.')?.parse().ok()?,
            };
            Some((index, path))
        })
        .collect();
    files.sort();
    if files.is_empty() {
        // Recordings that exist in one older generation only: the highest one stands in.
        let mut older: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                let f = p.file_name().unwrap().to_str().unwrap();
                f == "session.jsonl" || (f.starts_with("session.v") && f.ends_with(".jsonl"))
            })
            .collect();
        older.sort();
        files.push((
            0,
            older.pop().unwrap_or_else(|| panic!("{name}: no fixture")),
        ));
    }
    files
        .into_iter()
        .map(|(_, p)| std::fs::read_to_string(p).unwrap().replace("{{cwd}}", cwd))
        .collect()
}

fn manifest(suite: &str, name: &str) -> Value {
    let text = std::fs::read_to_string(scenario_dir(suite, name).join("snapshot.yml")).unwrap();
    // Only the few scalar paths the checks read are needed; the corpus loader owns the real parse.
    let mut manifest = json!({});
    if let Some(at) = text.find("DSH_SNAPSHOT_PROVIDER_CWD:") {
        let value = text[at..]
            .lines()
            .next()
            .unwrap()
            .split_once(':')
            .unwrap()
            .1
            .trim();
        manifest["environment"] = json!({"DSH_SNAPSHOT_PROVIDER_CWD": value});
    }
    if text.contains("attachments:") {
        let id = text
            .lines()
            .skip_while(|l| !l.contains("attachments:"))
            .find_map(|l| {
                l.trim()
                    .strip_prefix("- id:")
                    .or(l.trim().strip_prefix("id:"))
            })
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .to_owned();
        manifest["input"] = json!({"attachments": [{"id": id}]});
    }
    manifest
}

fn golden_notifications(name: &str) -> Vec<Value> {
    let dir = scenario_dir("sdk", name);
    [
        "notifications.expected.jsonl",
        "notifications.current.expected.jsonl",
    ]
    .iter()
    .map(|f| dir.join(f))
    .find(|p| p.exists())
    .map(|p| {
        records(
            &std::fs::read_to_string(p)
                .unwrap()
                .replace("{{cwd}}", "/work"),
        )
        .unwrap()
    })
    .unwrap_or_default()
}

/// The raw writer rows carry `seq` and `time`; the recorded fixtures keep neither.
fn with_seq(log: &str) -> String {
    let mut out = String::new();
    for (index, mut row) in records(log).unwrap().into_iter().enumerate() {
        if index > 0 && row["seq"].is_null() {
            row["seq"] = json!(index - 1);
            row["time"] = json!(0);
        }
        out.push_str(&format!("{row}\n"));
    }
    out
}

/// `subagent.started` / `subagent.finished` for every child Session of the run.
fn subagent_notifications(logs: &[String]) -> Vec<Value> {
    let mut out = vec![json!({"method": "subagent.started", "params": {}})];
    for log in &logs[1..] {
        let h = header(log);
        out.push(json!({"method": "subagent.finished", "params": {
            "childSessionId": h["id"], "parentSessionId": h["parentSession"]}}));
    }
    out
}

fn header(log: &str) -> Value {
    records(log).unwrap()[0].clone()
}

/// Notifications stood in for by the events of the durable log.
fn synthesized(logs: &[String]) -> Vec<Value> {
    let mut out = Vec::new();
    for log in logs {
        let id = header(log)["id"].clone();
        for row in records(log).unwrap().into_iter().skip(1) {
            out.push(json!({"method": "session.event", "params": {"sessionId": id, "event": row}}));
        }
    }
    out
}

struct Run<'a> {
    suite: &'a str,
    name: &'a str,
    actual: Vec<String>,
    expected: Vec<String>,
    notifications: Vec<Value>,
    cwd: PathBuf,
}

impl<'a> Run<'a> {
    fn recorded(suite: &'a str, name: &'a str) -> Self {
        let actual: Vec<String> = fixtures(suite, name, "/work")
            .iter()
            .map(|l| with_seq(l))
            .collect();
        Self {
            suite,
            name,
            expected: actual.clone(),
            actual,
            notifications: Vec::new(),
            cwd: PathBuf::from("/work"),
        }
    }

    fn check(&self) -> Option<Check> {
        let manifest = manifest(self.suite, self.name);
        let actual = self.actual.clone();
        let harvest = move || -> hoh_conformance::Result<Vec<String>> { Ok(actual.clone()) };
        let env = BTreeMap::new();
        let ctx = BodyContext {
            dir: &scenario_dir(self.suite, self.name),
            manifest: &manifest,
            actual: &self.actual,
            expected: &self.expected,
            notifications: &self.notifications,
            cwd: &self.cwd,
            harvest: &harvest,
            launch: Launch {
                hoh: Path::new("/nonexistent"),
                profile: "headless",
                patches: &[],
                env: &env,
                task: "task",
                timeout: std::time::Duration::from_secs(60),
            },
        };
        body_check(self.suite, self.name, &ctx)
    }

    fn passes(&self) {
        match self.check() {
            Some(Ok(())) => {}
            Some(Err(f)) => panic!("{}: {}\n{}", self.name, f.reason, f.diff),
            None => panic!("{}: no body", self.name),
        }
    }

    /// The same run with the scenario directory replaced (`BodyContext::dir`).
    fn fails_in(&self, dir: &Path, needle: &str) {
        let manifest = manifest(self.suite, self.name);
        let actual = self.actual.clone();
        let harvest = move || -> hoh_conformance::Result<Vec<String>> { Ok(actual.clone()) };
        let env = BTreeMap::new();
        let ctx = BodyContext {
            dir,
            manifest: &manifest,
            actual: &self.actual,
            expected: &self.expected,
            notifications: &self.notifications,
            cwd: &self.cwd,
            harvest: &harvest,
            launch: Launch {
                hoh: Path::new("/nonexistent"),
                profile: "headless",
                patches: &[],
                env: &env,
                task: "task",
                timeout: std::time::Duration::from_secs(60),
            },
        };
        let failure = body_check(self.suite, self.name, &ctx)
            .unwrap()
            .expect_err("a corrupted run passed");
        assert!(failure.reason.contains(needle), "{}", failure.reason);
    }

    fn clone_with(&self, f: impl FnOnce(Self) -> Self) -> Self {
        f(Self {
            suite: self.suite,
            name: self.name,
            actual: self.actual.clone(),
            expected: self.expected.clone(),
            notifications: self.notifications.clone(),
            cwd: self.cwd.clone(),
        })
    }

    fn fails_with(&self, needle: &str) {
        let reason = self.fails();
        assert!(reason.contains(needle), "{reason}");
    }

    fn fails(&self) -> String {
        match self.check() {
            Some(Err(f)) => f.reason,
            Some(Ok(())) => panic!("{}: a corrupted run passed", self.name),
            None => panic!("{}: no body", self.name),
        }
    }

    fn mutate_log(mut self, index: usize, from: &str, to: &str) -> Self {
        assert!(
            self.actual[index].contains(from),
            "{}: no {from:?} to corrupt",
            self.name
        );
        self.actual[index] = self.actual[index].replace(from, to);
        self
    }
}

fn pair(suite: &str, name: &str, from: &str, to: &str) {
    Run::recorded(suite, name).passes();
    Run::recorded(suite, name).mutate_log(0, from, to).fails();
}

#[test]
fn empty_response_retry_current_checks_the_retry_events() {
    pair(
        "session",
        "empty-response-retry-current",
        "\"type\":\"llm/retry-started\"",
        "\"type\":\"llm/retry-other\"",
    );
}

#[test]
fn tool_call_turn_compares_the_writer_with_the_migrated_retained_v3_input() {
    // The oracle is `session.v3.jsonl` of the corpus, migrated by the catalog: the V4 fixture the
    // runner replays is not read at all.
    let mut run = Run::recorded("session", "tool-call-turn");
    run.expected.clear();
    run.passes();
    // The writer's tool result differs from the migration output.
    run.clone_with(|r| r.mutate_log(0, "\"isError\":false", "\"isError\":true"))
        .fails_with("native tool-result messages match migration output");
    // A retained input that is corrupted in `content` no longer matches the (unchanged) writer output.
    let dir = tempfile::tempdir().unwrap();
    let source = scenario_dir("session", "tool-call-turn");
    let v3 = std::fs::read_to_string(source.join("session.v3.jsonl")).unwrap();
    assert!(v3.contains("SNAPSHOT_OK"));
    std::fs::write(
        dir.path().join("session.v3.jsonl"),
        v3.replace("SNAPSHOT_OK", "SNAPSHOT_CHANGED"),
    )
    .unwrap();
    run.fails_in(
        dir.path(),
        "native tool-result messages match migration output",
    );
    // The retained input must really be generation 3.
    std::fs::write(
        dir.path().join("session.v3.jsonl"),
        v3.replacen("\"version\":3", "\"version\":4", 1),
    )
    .unwrap();
    run.fails_in(dir.path(), "retained tool-result input version");
}

#[test]
fn toolcall_unused_args_checks_greps_and_goal() {
    pair(
        "session",
        "toolcall-unused-args",
        "Found 1 match",
        "Found 2 matches",
    );
    let run = Run::recorded("session", "toolcall-unused-args").mutate_log(
        0,
        "\"phase\":\"complete\"",
        "\"phase\":\"active\"",
    );
    run.fails();
}

#[test]
fn office_progressive_native_checks_the_render_refusal() {
    pair(
        "session",
        "office-progressive-native",
        "# Office task routing",
        "# Routing",
    );
    pair(
        "session",
        "office-progressive-native",
        "does not accept images",
        "is fine",
    );
}

#[test]
fn subagent_depth_two_rejection_checks_leaf_and_ancestors() {
    Run::recorded("session", "subagent-depth-two-rejection").passes();
    assert!(
        Run::recorded("session", "subagent-depth-two-rejection").actual[2]
            .contains("DEPTH_REJECTED")
    );
    Run::recorded("session", "subagent-depth-two-rejection")
        .mutate_log(2, "DEPTH_REJECTED", "DEPTH_OK")
        .fails();
    // An ancestor that received the settled notice of the leaf.
    let mut run = Run::recorded("session", "subagent-depth-two-rejection");
    let leaf = header(&run.actual[2])["id"].as_str().unwrap().to_owned();
    let notice = json!({"type": "user/message", "data": {"content": [], "source":
        {"kind": "subagent-settled", "senderSessionId": leaf}}});
    run.actual[0].push_str(&format!("{notice}\n"));
    run.fails();
}

#[test]
fn working_directory_checks_changes_and_notices() {
    pair(
        "session",
        "working-directory",
        "\"type\":\"working-directory/change\",\"data\":{\"cwd\":\"/work/selected\"",
        "\"type\":\"working-directory/change\",\"data\":{\"cwd\":\"/work/other\"",
    );
    pair(
        "session",
        "working-directory",
        "The working directory changed from",
        "The directory changed from",
    );
}

#[test]
fn packed_chunks_checks_the_row_types_and_the_unpacked_equality() {
    // The runner hands the raw fixture (packed rows carry `seq0`/`time0` or nothing).
    let raw =
        std::fs::read_to_string(scenario_dir("session", "packed-chunks").join("session.jsonl"))
            .unwrap();
    let mut run = Run::recorded("session", "packed-chunks");
    run.expected = vec![raw.clone()];
    run.passes();
    let mut other = run.clone_with(|r| r);
    other.expected[0] = raw.replace("\"type\":\"text-chunks\"", "\"type\":\"text-other\"");
    other.fails();
    // One packed row whose payload the decoder reads differently from the unpacked recording is
    // not tolerated: the `dt` gaps (event times) of a packed row are part of the comparison.
    let mut corrupt = run.clone_with(|r| r);
    let mut rows = hoh_conformance::records(&raw).unwrap();
    let at = rows
        .iter()
        .position(|r| r["type"] == "text-chunks")
        .expect("a packed text row");
    rows[at]["data"]["texts"][0] = json!("CHANGED");
    corrupt.expected[0] = rows.iter().map(|r| format!("{r}\n")).collect();
    // Changing the packed text is consistent in both forms; the corruption must be one the two
    // forms read differently — a text member count that disagrees with `dt`.
    let dt = rows[at]["data"]["dt"].as_array().unwrap().len();
    rows[at]["data"]["dt"] = json!(vec![0; dt + 1]);
    corrupt.expected[0] = rows.iter().map(|r| format!("{r}\n")).collect();
    corrupt.fails();
}

#[test]
fn background_confinement_failure_reads_the_audit() {
    let dir = tempfile::tempdir().unwrap();
    let write =
        |text: &str| std::fs::write(dir.path().join("confinement-audit.json"), text).unwrap();
    let mut run = Run::recorded("session", "background-confinement-failure");
    // The fixture holds the normalized job id; the writer produces `bash-` and 16 hex digits.
    run.actual[0] = run.actual[0].replace("{{id:1}}", "bash-0123456789abcdef");
    run.cwd = dir.path().to_owned();
    write("{\"confineCalls\":1,\"spawnCalls\":0}");
    run.passes();
    write("{\"confineCalls\":1,\"spawnCalls\":1}");
    run.fails();
    write("{\"confineCalls\":1,\"spawnCalls\":0}");
    run.mutate_log(0, "fixture asynchronous confinement refused", "fixture ran")
        .fails();
}

fn git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn worktree_checks_git_state_and_log() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    git(&cwd, &["init", "-q", "-b", "main"]);
    std::fs::write(cwd.join("tracked.txt"), "COMMITTED CONTENT\n").unwrap();
    git(&cwd, &["add", "tracked.txt"]);
    git(&cwd, &["commit", "-q", "-m", "seed"]);
    std::fs::write(
        cwd.join(".git/info/attributes"),
        "tracked.txt filter=dsh-snapshot-checkout\n",
    )
    .unwrap();
    git(
        &cwd,
        &[
            "config",
            "filter.dsh-snapshot-checkout.smudge",
            "printf 'FILTERED CHECKOUT CONTENT\\n'",
        ],
    );
    std::fs::copy(
        cwd.join(".git/config"),
        cwd.join(".git/config.before-worktree"),
    )
    .unwrap();
    std::fs::create_dir_all(cwd.join(".agents/worktrees")).unwrap();
    std::fs::write(cwd.join(".agents/worktrees/.gitignore"), "*\n").unwrap();
    git(
        &cwd,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "isolated",
            ".agents/worktrees/isolated",
        ],
    );
    let checkout = cwd.join(".agents/worktrees/isolated");
    std::fs::write(checkout.join("tracked.txt"), "COMMITTED CONTENT\n").unwrap();
    std::fs::write(checkout.join("result.txt"), "WORKTREE RESULT\n").unwrap();
    let text = cwd.to_string_lossy().into_owned();
    let mut run = Run::recorded("session", "worktree");
    run.actual = fixtures("session", "worktree", &text);
    run.cwd = cwd.clone();
    run.passes();
    std::fs::write(checkout.join("result.txt"), "OTHER\n").unwrap();
    run.fails();
    std::fs::write(checkout.join("result.txt"), "WORKTREE RESULT\n").unwrap();
    std::fs::write(checkout.join("local.txt"), "leak").unwrap();
    run.fails();
}

#[test]
fn provider_cwd_resume_launches_the_binary_twice() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let mut run = Run::recorded("session", "provider-cwd");
    run.actual = fixtures("session", "provider-cwd", "/recorded-provider/workspace")
        .iter()
        .map(|l| with_seq(l))
        .collect();
    let rows = records(&run.actual[0]).unwrap();
    let provider = rows[0]["cwd"].as_str().unwrap().to_owned();
    let id = rows[0]["id"].as_str().unwrap().to_owned();
    let text = hoh_conformance::compare::final_text(&rows);
    // The double of `hoh`: it refuses a foreign provider cwd and otherwise extends the Session.
    let sessions = root.join("sessions.jsonl");
    std::fs::write(&sessions, &run.actual[0]).unwrap();
    let script = root.join("hoh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$DSH_SNAPSHOT_PROVIDER_CWD\" in\n  */other) echo 'Session was recorded in \"{provider}\", not \"'\"$DSH_SNAPSHOT_PROVIDER_CWD\"'\"' >&2; exit 1;;\nesac\n\
printf '%s\\n' '{{\"type\":\"turn/start\",\"data\":{{\"turn\":2}}}}' >> '{s}'\n\
printf '%s\\n' '{{\"type\":\"session\",\"sessionId\":\"{id}\",\"cwd\":\"{provider}\"}}'\n\
printf '%s\\n' '{{\"type\":\"final\",\"sessionId\":\"{id}\",\"reason\":{{\"kind\":\"completed\"}},\"text\":{t}}}'\n",
            s = sessions.display(),
            t = json!(text),
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    run.cwd = root.clone();
    let manifest = manifest("session", "provider-cwd");
    let harvest = || -> hoh_conformance::Result<Vec<String>> {
        Ok(vec![std::fs::read_to_string(&sessions).unwrap()])
    };
    let check = provider_check(&script, &root, &manifest, &run.actual, &harvest);
    assert!(check.is_ok(), "{:?}", check.err().map(|f| f.reason));
    // A binary that resumes under a foreign cwd is caught.
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    let check = provider_check(&script, &root, &manifest, &run.actual, &harvest);
    assert!(check.is_err());
    // A binary that never returns is bounded and killed: the check fails, it does not hang the run.
    std::fs::write(&script, "#!/bin/sh\nexec sleep 30\n").unwrap();
    let started = std::time::Instant::now();
    let check = provider_check_within(
        std::time::Duration::from_millis(300),
        &script,
        &root,
        &manifest,
        &run.actual,
        &harvest,
    );
    let failure = check.unwrap_err();
    assert_eq!(failure.phase, "body");
    assert!(
        failure.reason.contains("process timeout"),
        "{}",
        failure.reason
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn sdk_command_scenarios_check_the_command_rows() {
    pair(
        "sdk",
        "command-attachments",
        "Plan mode on. Use /plan off to leave.",
        "Plan mode.",
    );
    pair(
        "sdk",
        "command-identity",
        "\"name\":\"access\"",
        "\"name\":\"acces\"",
    );
    pair(
        "sdk",
        "permission-switch",
        "\"policy\":\"never\"",
        "\"policy\":\"ask\"",
    );
}

#[test]
fn manual_compact_checks_the_compaction_rows_and_command_notifications() {
    let mut run = Run::recorded("sdk", "manual-compact");
    run.notifications = synthesized(&run.actual);
    run.passes();
    let mut broken = Run::recorded("sdk", "manual-compact").mutate_log(
        0,
        "\"llmStreamCall\":true",
        "\"llmStreamCall\":false",
    );
    broken.notifications = synthesized(&broken.actual);
    broken.fails();
    let silent = Run::recorded("sdk", "manual-compact");
    silent.fails();
}

#[test]
fn dynamic_tool_updates_compare_notifications_with_the_log() {
    // The goldens are normalized (`{{sessionId}}` for message ids); both sides drop the ids.
    let strip = |value: &mut Value| {
        if let Some(message) = value
            .pointer_mut("/data/message")
            .and_then(Value::as_object_mut)
        {
            message.remove("id");
        }
    };
    let golden = || -> Vec<Value> {
        let mut notes = golden_notifications("dynamic-tool-updates");
        for note in &mut notes {
            if let Some(event) = note.pointer_mut("/params/event") {
                strip(event);
            }
        }
        notes
    };
    let stripped = |run: &mut Run<'_>| {
        run.actual[0] = records(&run.actual[0])
            .unwrap()
            .into_iter()
            .map(|mut row| {
                strip(&mut row);
                format!("{row}\n")
            })
            .collect();
    };
    let mut run = Run::recorded("sdk", "dynamic-tool-updates");
    stripped(&mut run);
    run.notifications = golden();
    run.passes();
    // A subscriber that missed an event.
    let mut broken = Run::recorded("sdk", "dynamic-tool-updates");
    stripped(&mut broken);
    let mut notes = golden();
    let at = notes
        .iter()
        .position(|n| n["params"]["event"]["type"] == "developer/message")
        .unwrap();
    notes.remove(at);
    broken.notifications = notes;
    broken.fails();
    // A log whose tool registry update names another tool.
    let mut other = Run::recorded("sdk", "dynamic-tool-updates");
    stripped(&mut other);
    let mut other = other.mutate_log(
        0,
        "\"toolName\":\"snapshot_ping\"",
        "\"toolName\":\"other\"",
    );
    other.notifications = golden();
    other.fails();
}

#[test]
fn tool_scheduler_recovery_checks_both_calls() {
    pair(
        "sdk",
        "tool-scheduler-recovery",
        "SCHEDULER_RECOVERY_OK",
        "SCHEDULER_RECOVERY_NO",
    );
}

#[test]
fn subagent_activation_limit_checks_the_denial() {
    let mut run = Run::recorded("sdk", "subagent-activation-limit");
    run.notifications = subagent_notifications(&run.actual);
    run.passes();
    let mut broken = Run::recorded("sdk", "subagent-activation-limit").mutate_log(
        0,
        "subagent limit reached (active child limit: 1)",
        "denied",
    );
    broken.notifications = subagent_notifications(&broken.actual);
    broken.fails();
}

#[test]
fn system_prompt_in_history_checks_the_delivered_events() {
    // The goldens carry the normalized text `{{system}}`; the writer delivers the real prompts.
    let delivered = |second: &str| -> Vec<Value> {
        let mut seen = 0;
        golden_notifications("system-prompt-in-history")
            .into_iter()
            .map(|n| {
                if n["params"]["event"]["type"] != "system/message" {
                    return n;
                }
                seen += 1;
                let text = if seen == 1 {
                    "You are an agent."
                } else {
                    second
                };
                serde_json::from_str(&n.to_string().replace("{{system}}", text)).unwrap()
            })
            .collect()
    };
    let mut run = Run::recorded("sdk", "system-prompt-in-history");
    run.notifications = delivered("You are an agent. Snapshot guidance added after the first read");
    run.passes();
    let mut same = Run::recorded("sdk", "system-prompt-in-history");
    same.notifications = delivered("You are an agent.");
    same.fails();
}

#[test]
fn desktop_context_checks_the_runtime_context() {
    pair(
        "sdk",
        "desktop-context",
        "Current working directory:",
        "Directory:",
    );
}

#[test]
fn serial_created_checks_the_turn_reasons_and_methods() {
    Run::recorded("sdk", "serial-created").passes();
    Run::recorded("sdk", "serial-created")
        .mutate_log(
            0,
            "\"reason\":{\"kind\":\"completed\"}",
            "\"reason\":{\"kind\":\"error\"}",
        )
        .fails();
    let mut started = Run::recorded("sdk", "serial-created");
    started.notifications = vec![json!({"method": "subagent.started", "params": {}})];
    started.fails();
}

#[test]
fn multi_session_scenarios_need_matching_subagent_notifications() {
    let mut run = Run::recorded("sdk", "subagent-mixed");
    assert!(run.actual.len() > 1);
    let child = header(&run.actual[1]);
    let notes = |parent: Value| {
        vec![
            json!({"method": "subagent.started", "params": {}}),
            json!({"method": "subagent.finished", "params": {
                "childSessionId": child["id"], "parentSessionId": parent}}),
        ]
    };
    run.notifications = notes(child["parentSession"].clone());
    run.passes();
    run.notifications = notes(json!("someone-else"));
    run.fails();
    run.notifications = Vec::new();
    run.fails();
}

#[test]
fn scenarios_without_a_body_pass_and_unlisted_ones_are_not_claimed() {
    for name in ["multi-turn", "bash-tool"] {
        Run::recorded("sdk", name).passes();
        assert!(
            hoh_conformance::body::unchecked("sdk", name).is_none(),
            "{name}"
        );
    }
    let run = Run::recorded("sdk", "text-turn");
    assert!(body_check_of(&run, "persistent-tools").is_none());
    assert_eq!(hoh_conformance::body::sdk::SCENARIOS_IN.len(), 26);
}

fn body_check_of(run: &Run<'_>, name: &str) -> Option<Check> {
    let manifest = json!({});
    let harvest = || -> hoh_conformance::Result<Vec<String>> { Ok(Vec::new()) };
    let env = BTreeMap::new();
    let ctx = BodyContext {
        dir: Path::new("."),
        manifest: &manifest,
        actual: &run.actual,
        expected: &run.expected,
        notifications: &[],
        cwd: Path::new("/work"),
        harvest: &harvest,
        launch: Launch {
            hoh: Path::new("hoh"),
            profile: "sdk",
            patches: &[],
            env: &env,
            task: "",
            timeout: std::time::Duration::from_secs(60),
        },
    };
    body_check("sdk", name, &ctx)
}

fn provider_check(
    hoh: &Path,
    root: &Path,
    manifest: &Value,
    actual: &[String],
    harvest: &dyn Fn() -> hoh_conformance::Result<Vec<String>>,
) -> Check {
    provider_check_within(
        std::time::Duration::from_secs(60),
        hoh,
        root,
        manifest,
        actual,
        harvest,
    )
}

fn provider_check_within(
    timeout: std::time::Duration,
    hoh: &Path,
    root: &Path,
    manifest: &Value,
    actual: &[String],
    harvest: &dyn Fn() -> hoh_conformance::Result<Vec<String>>,
) -> Check {
    let env = BTreeMap::new();
    let ctx = BodyContext {
        dir: root,
        manifest,
        actual,
        expected: actual,
        notifications: &[],
        cwd: Path::new("/work"),
        harvest,
        launch: Launch {
            hoh,
            profile: "headless",
            patches: &[],
            env: &env,
            task: "task",
            timeout,
        },
    };
    body_check("session", "provider-cwd", &ctx).unwrap()
}

/// Review 220412 №2: a carried-but-partial SDK body is "passed, body not checked", never a clean pass.
#[test]
fn a_partially_carried_sdk_body_names_what_it_leaves_unchecked() {
    use hoh_conformance::{
        body::{gate, unchecked},
        compare::Failure,
        runner::Outcome,
        v1::{Entry, Row, Status, classify, totals},
    };
    assert_eq!(
        unchecked("sdk", "permission-switch").as_deref(),
        Some("sdk.permission-switch: modelRequests, expectedFinalResponse")
    );
    assert_eq!(
        unchecked("sdk", "desktop-context").as_deref(),
        Some("sdk.desktop-context: expectedFinalResponse")
    );
    assert_eq!(unchecked("sdk", "multi-turn"), None);
    assert_eq!(unchecked("session", "permission-switch"), None);
    for name in [
        "command-attachments",
        "command-identity",
        "permission-switch",
        "manual-compact",
    ] {
        let failure = gate("sdk", name, Ok(())).unwrap_err();
        assert_eq!(failure.phase, "scenario-check", "{name}");
        assert!(failure.reason.contains("modelRequests"), "{name}");
    }
    // A failing body stays a failure; a body without unchecked parts stays a pass.
    let failed = gate("sdk", "permission-switch", Err(Failure::new("body", "x"))).unwrap_err();
    assert_eq!(failed.phase, "body");
    assert!(gate("sdk", "multi-turn", Ok(())).is_ok());
    // The report counts it: Passed with the part in `body_check`.
    let outcome = Outcome {
        suite: "sdk".into(),
        scenario: "permission-switch".into(),
        failure: Some(gate("sdk", "permission-switch", Ok(())).unwrap_err()),
        runner_error: None,
    };
    let verdict = classify(&outcome, "");
    assert_eq!(verdict.status, Status::Passed);
    assert!(
        verdict
            .body_check
            .contains("sdk.permission-switch: modelRequests")
    );
    let row = Row {
        suite: "sdk".into(),
        scenario: "permission-switch".into(),
        group: "in".into(),
    };
    assert!(totals(&[Entry { row, verdict }]).contains("прошло 1 (тело не проверено 1)"));
}

/// Review 224637 №1: scenarios whose controller checks live in the protocol driver are never a clean pass.
#[test]
fn controller_only_sdk_scenarios_are_passed_body_not_checked() {
    use hoh_conformance::body::{gate, unchecked};
    for (name, part) in [
        ("approval", "approval/request params"),
        ("session-resume", "close/resume/missing-session"),
        ("subagent-continuable", "completion order"),
        ("session-title-stability", "provider-title waits"),
        ("text-turn", "feedback chain"),
    ] {
        assert!(unchecked("sdk", name).unwrap().contains(part), "{name}");
        // The recorded journal matches, the carried body (if any) passes — still gated.
        let failure = gate("sdk", name, Ok(())).unwrap_err();
        assert_eq!(failure.phase, "scenario-check", "{name}");
        assert!(failure.reason.contains(part) && failure.reason.contains("not implemented"));
    }
}

/// Review 230043 №6: `!result.isError` — an absent flag is not an error.
#[test]
fn toolcall_unused_args_accepts_a_tool_result_without_is_error() {
    let mut run = Run::recorded("session", "toolcall-unused-args");
    run.passes();
    let stripped = run.actual[0]
        .replace(",\"isError\":false", "")
        .replace("\"isError\":false,", "");
    assert_ne!(stripped, run.actual[0], "no isError to strip");
    run.actual[0] = stripped;
    run.passes();
    run.clone_with(|r| r.mutate_log(0, "\"call_grep_ts\"", "\"call_grep_ts\",\"isError\":true"))
        .fails();
}

/// Review 230043 №1: the second run on the retained V3 input is not carried — "passed, body not checked".
#[test]
fn tool_call_turn_is_passed_body_not_checked() {
    use hoh_conformance::body::{gate, unchecked};
    let part = unchecked("session", "tool-call-turn").unwrap();
    assert!(part.contains("retained V3"));
    let failure = gate("session", "tool-call-turn", Ok(())).unwrap_err();
    assert_eq!(failure.phase, "scenario-check");
    assert!(failure.reason.contains("not implemented"));
    assert!(gate("session", "toolcall-unused-args", Ok(())).is_ok());
}

/// Review 230043 №5: the comparison goes through the normalizer; chunk contents differ, clocks do not.
#[test]
fn packed_comparison_sees_chunk_contents_but_not_clocks() {
    let raw =
        std::fs::read_to_string(scenario_dir("session", "packed-chunks").join("session.jsonl"))
            .unwrap();
    let rows = hoh_conformance::records(&raw).unwrap();
    let at = rows
        .iter()
        .position(|r| r["type"] == "text-chunks")
        .unwrap();
    let render = |rows: &[Value]| -> String { rows.iter().map(|r| format!("{r}\n")).collect() };
    let base = hoh_conformance::compare::normalized_session(&raw).unwrap();
    // Other clocks, same chunks.
    let mut clocked = rows.clone();
    let dt = clocked[at]["data"]["dt"].as_array().unwrap().len();
    clocked[at]["data"]["dt"] = json!(vec![7; dt]);
    assert_eq!(
        hoh_conformance::compare::normalized_session(&render(&clocked)).unwrap(),
        base
    );
    // Another chunk text (same count): a different recording.
    let mut other = rows.clone();
    other[at]["data"]["texts"][0] = json!("CHANGED-CHUNK");
    assert_ne!(
        hoh_conformance::compare::normalized_session(&render(&other)).unwrap(),
        base
    );
}

/// Tail p.7 (`sdk.snapshot.ts:1288`): a manifest without attachments declares no id, so the
/// expected image block carries no `attachmentId` key (never `null`); a log that carries one differs.
#[test]
fn command_attachments_without_a_declared_attachment_omits_the_id_key() {
    let id = "\"attachmentId\":\"sha256:b1ff9c8ea3a780bad09b346c423d2d0e46815926879b18e841d928376a946640\",";
    let mut run = Run::recorded("sdk", "command-attachments");
    run.actual = run.actual.iter().map(|l| l.replace(id, "")).collect();
    run.expected = run.actual.clone();
    let check = |run: &Run<'_>, manifest: &Value| {
        let actual = run.actual.clone();
        let harvest = move || -> hoh_conformance::Result<Vec<String>> { Ok(actual.clone()) };
        let env = BTreeMap::new();
        let ctx = BodyContext {
            dir: &scenario_dir(run.suite, run.name),
            manifest,
            actual: &run.actual,
            expected: &run.expected,
            notifications: &run.notifications,
            cwd: &run.cwd,
            harvest: &harvest,
            launch: Launch {
                hoh: Path::new("/nonexistent"),
                profile: "headless",
                patches: &[],
                env: &env,
                task: "task",
                timeout: std::time::Duration::from_secs(60),
            },
        };
        body_check(run.suite, run.name, &ctx)
    };
    assert!(matches!(check(&run, &json!({})), Some(Ok(()))));
    // The same stripped log against a manifest that declares an attachment is a mismatch.
    let declared = json!({"input": {"attachments": [{"id": "sha256:x"}]}});
    assert!(matches!(check(&run, &declared), Some(Err(_))));
}
