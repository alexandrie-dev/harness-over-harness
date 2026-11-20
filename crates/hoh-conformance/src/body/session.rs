//! Headless scenario bodies: `snapshots/session/headless.snapshot.ts`.

use super::{
    BodyContext, expect, expect_eq, expect_match, fail, of_type, rows, rows_of, text_of,
    tool_results,
};
use crate::compare::{self, Check};
use serde_json::{Value, json};
use std::{path::Path, process::Command};

const WORKTREE_CHECKOUT_FILTER: &str = "printf 'FILTERED CHECKOUT CONTENT\\n'";
const WORKTREE_CHECKOUT_ATTRIBUTES: &str = "tracked.txt filter=dsh-snapshot-checkout\n";
/// `SESSION_FORMAT_VERSION`.
const SESSION_FORMAT_VERSION: u64 = 4;

pub fn check(name: &str, ctx: &BodyContext<'_>) -> Option<Check> {
    Some(match name {
        "empty-response-retry-current" => empty_response_retry_current(ctx),
        "tool-call-turn" => tool_result_writer_parity(ctx),
        "toolcall-unused-args" => toolcall_unused_args(ctx),
        "office-progressive-native" | "office-progressive-ptc" => {
            office_progressive(name == "office-progressive-native", ctx)
        }
        "subagent-depth-two-rejection" => subagent_depth_two_rejection(ctx),
        "provider-cwd" => provider_cwd_resume(ctx),
        "background-confinement-failure" => background_confinement_failure(ctx),
        "working-directory" => working_directory(ctx),
        "worktree" => worktree(ctx),
        "packed-chunks" => packed_chunks(ctx),
        _ => return None,
    })
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn js_string(text: &str) -> String {
    hoh_util_values::js_json_stringify(&json!(text))
}

/// headless.snapshot.ts:1447–1453.
fn empty_response_retry_current(ctx: &BodyContext<'_>) -> Check {
    let rows = rows_of(ctx.actual, 0, "empty-response-retry-current")?;
    let kinds: Vec<&str> = rows
        .iter()
        .filter_map(|r| r["type"].as_str())
        .filter(|kind| {
            [
                "assistant/attempt",
                "llm/retry",
                "llm/retry-started",
                "assistant/message",
            ]
            .contains(kind)
        })
        .collect();
    expect_eq(
        "retry events",
        &json!(kinds),
        &json!([
            "assistant/attempt",
            "llm/retry",
            "llm/retry-started",
            "assistant/message"
        ]),
    )
}

/// headless.snapshot.ts:171–183, :1349–1350, :1610: the retained V3 input (`session.v3.jsonl`) is
/// migrated by the catalog and its stock tool results equal the raw writer output, ids aside.
fn tool_result_writer_parity(ctx: &BodyContext<'_>) -> Check {
    let retained = read_text(&ctx.dir.join("session.v3.jsonl"))?;
    let version_of = |log: &str, label: &str| -> std::result::Result<u64, compare::Failure> {
        rows(log)?
            .first()
            .and_then(|h| h["version"].as_u64())
            .ok_or_else(|| fail(format!("{label}: no Session header version")))
    };
    expect_eq(
        "retained tool-result input version",
        &json!(version_of(&retained, "retained tool-result input")?),
        &json!(3),
    )?;
    let written_log = ctx
        .actual
        .first()
        .ok_or_else(|| fail("tool-call-turn: no persisted Session"))?;
    expect(
        version_of(written_log, "raw tool-result writer")? == SESSION_FORMAT_VERSION,
        || "raw tool-result writer did not write the current generation".into(),
    )?;
    let without_id = |messages: Vec<Value>| -> Vec<Value> {
        messages
            .into_iter()
            .map(|mut message| {
                if let Some(object) = message.as_object_mut() {
                    object.shift_remove("id");
                }
                message
            })
            .collect()
    };
    let migrated = compare::migrate_fixture(&retained)?;
    let expected = without_id(
        migrated
            .events
            .iter()
            .filter(|event| event["type"] == "tool/result")
            .map(|event| event["data"]["message"].clone())
            .collect(),
    );
    let written = without_id(
        tool_results(&rows(written_log)?)
            .into_iter()
            .cloned()
            .collect(),
    );
    expect(!expected.is_empty(), || {
        "recorded stock tool results: none".into()
    })?;
    expect_eq(
        "native tool-result messages match migration output",
        &json!(written),
        &json!(expected),
    )
}

/// headless.snapshot.ts:1485–1503.
/// JavaScript truthiness of a JSON value (`!result.isError` of the source).
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn toolcall_unused_args(ctx: &BodyContext<'_>) -> Check {
    let rows = rows_of(ctx.actual, 0, "toolcall-unused-args")?;
    let results = tool_results(&rows);
    expect(results.len() == 6, || {
        format!("expected 6 tool results, got {}", results.len())
    })?;
    expect(results.iter().all(|r| !js_truthy(&r["isError"])), || {
        "a tool result is an error".into()
    })?;
    for extension in ["ts", "md"] {
        let id = format!("call_grep_{extension}");
        let text = results
            .iter()
            .find(|r| r["toolCallId"] == id.as_str())
            .map(|r| {
                r["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        for needle in [
            "Found 1 match".to_owned(),
            format!("seed/example.{extension}"),
            "marker".to_owned(),
        ] {
            expect(text.contains(&needle), || {
                format!("{id}: result lacks {needle:?}: {text:?}")
            })?;
        }
    }
    let changes: Vec<&Value> = of_type(&rows, "goal/change")
        .into_iter()
        .map(|r| &r["data"])
        .collect();
    expect_eq(
        "goal operations",
        &json!(
            changes
                .iter()
                .map(|c| c["operation"].clone())
                .collect::<Vec<_>>()
        ),
        &json!(["create", "complete"]),
    )?;
    let original = &changes[0]["goal"];
    expect_match(
        "created goal",
        original,
        &json!({"objective": "Find marker in both seed files.", "maxGoalRounds": 4}),
    )?;
    let mut completed = original.clone();
    completed["revision"] = json!(2);
    completed["phase"] = json!("complete");
    expect_eq("completed goal", &changes[1]["goal"], &completed)
}

/// headless.snapshot.ts:1504–1519.
fn office_progressive(native: bool, ctx: &BodyContext<'_>) -> Check {
    let log = ctx
        .actual
        .first()
        .ok_or_else(|| fail("office-progressive: no persisted Session"))?;
    let rows = rows(log)?;
    let mut results = tool_results(&rows);
    if native {
        // office_render refuses a render for a model route without image input.
        let render = results
            .pop()
            .ok_or_else(|| fail("office-progressive-native: no tool results"))?;
        expect(render["isError"] == true, || {
            "the render of a route without image input is not an error".into()
        })?;
        let content = render["content"].as_array().cloned().unwrap_or_default();
        let pattern = regex::Regex::new(
            r#"^Error: cannot render ".+[\\/]report\.docx": model "deepseek-v4-flash" does not accept images$"#,
        )
        .map_err(|e| fail(e.to_string()))?;
        expect(
            content.len() == 1
                && content[0]["type"] == "text"
                && content[0]["text"]
                    .as_str()
                    .is_some_and(|t| pattern.is_match(t)),
            || format!("render error content: {}", json!(content)),
        )?;
    }
    expect(results.len() == 2, || {
        format!("expected 2 tool results, got {}", results.len())
    })?;
    expect(results.iter().all(|r| r["isError"] == false), || {
        "a tool result is an error".into()
    })?;
    expect(log.contains("# Office task routing"), || {
        "the Session lacks the office routing prompt".into()
    })
}

/// headless.snapshot.ts:1523–1531.
fn subagent_depth_two_rejection(ctx: &BodyContext<'_>) -> Check {
    expect(ctx.actual.len() == 3, || {
        format!("expected 3 Sessions, got {}", ctx.actual.len())
    })?;
    let leaf = &ctx.actual[2];
    expect(leaf.contains("DEPTH_REJECTED"), || {
        "the leaf Session lacks DEPTH_REJECTED".into()
    })?;
    let leaf_id = rows(leaf)?
        .first()
        .map(|h| h["id"].clone())
        .unwrap_or(Value::Null);
    for (index, ancestor) in ctx.actual[..2].iter().enumerate() {
        let notices = rows(ancestor)?
            .iter()
            .filter(|r| {
                r["type"] == "user/message"
                    && r["data"]["source"]["kind"] == "subagent-settled"
                    && r["data"]["source"]["senderSessionId"] == leaf_id
            })
            .count();
        expect(notices == 0, || {
            format!("ancestor {index} received {notices} settled notice(s) of the leaf")
        })?;
    }
    Ok(())
}

/// headless.snapshot.ts:862–879.
fn background_confinement_failure(ctx: &BodyContext<'_>) -> Check {
    let rows = rows_of(ctx.actual, 0, "background-confinement-failure")?;
    let results = tool_results(&rows);
    let by_id = |id: &str| results.iter().find(|r| r["toolCallId"] == id).copied();
    let started = by_id("async-confinement-start");
    let inspected = by_id("async-confinement-result");
    let registered = of_type(&rows, "job/registered")
        .first()
        .copied()
        .ok_or_else(|| fail("missing registered job"))?;
    let job_id = registered["data"]["jobId"].as_str().unwrap_or_default();
    let pattern = regex::Regex::new(r"^bash-[0-9a-f]{16}$").map_err(|e| fail(e.to_string()))?;
    expect(pattern.is_match(job_id), || {
        format!("job id {job_id:?} does not match bash-<16 hex>")
    })?;
    expect_match(
        "background start result",
        started.unwrap_or(&Value::Null),
        &json!({
            "isError": false,
            "content": [{"type": "text", "text": format!("started background job {job_id}")}],
        }),
    )?;
    let inspected = inspected.unwrap_or(&Value::Null);
    expect(inspected["isError"] == false, || {
        "the inspection result is not a success".into()
    })?;
    let text = text_of(inspected);
    for needle in [
        "[status: failed,",
        "fixture asynchronous confinement refused",
    ] {
        expect(text.contains(needle), || {
            format!("inspection result lacks {needle:?}: {text:?}")
        })?;
    }
    let audit = std::fs::read_to_string(ctx.cwd.join("confinement-audit.json"))
        .map_err(|e| fail(format!("confinement-audit.json: {e}")))?;
    let audit = hoh_util_values::js_json_parse(&audit).map_err(|e| fail(e.to_string()))?;
    expect_eq(
        "confinement audit",
        &audit,
        &json!({"confineCalls": 1, "spawnCalls": 0}),
    )
}

/// headless.snapshot.ts:881–904.
fn working_directory(ctx: &BodyContext<'_>) -> Check {
    let events = rows_of(ctx.actual, 0, "working-directory")?;
    let cwd = path_text(ctx.cwd);
    let join = |name: &str| format!("{cwd}/{name}");
    let directories: Vec<Value> = of_type(&events, "working-directory/change")
        .iter()
        .map(|e| e["data"]["cwd"].clone())
        .collect();
    expect_eq(
        "working-directory/change",
        &json!(directories),
        &json!([join("selected"), join("second"), cwd]),
    )?;
    let systems = of_type(&events, "system/message").len();
    expect(systems == 1, || {
        format!("expected 1 system/message, got {systems}")
    })?;
    let users: Vec<&Value> = of_type(&events, "user/message")
        .into_iter()
        .map(|e| &e["data"])
        .collect();
    let mut contexts = Vec::new();
    for message in &users {
        if message["source"]["kind"] != "runtime-context" {
            continue;
        }
        for section in message["source"]["sections"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if section["name"] == "working-directory:current" {
                contexts.push(section["text"].clone());
            }
        }
    }
    let current = |directory: &str| format!("Current working directory: {}.", js_string(directory));
    expect_eq(
        "working-directory:current sections",
        &json!(contexts),
        &json!([
            current(&cwd),
            current(&join("selected")),
            current(&join("second")),
            current(&cwd)
        ]),
    )?;
    let notices: Vec<String> = users
        .iter()
        .filter(|m| m["source"]["kind"] == "working-directory")
        .map(|m| {
            m["content"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|b| b["text"].as_str().unwrap_or_default())
                .collect::<String>()
        })
        .collect();
    expect_eq(
        "working-directory notices",
        &json!(notices),
        &json!([
            format!(
                "The working directory changed from {} to {}.",
                js_string(&cwd),
                js_string(&join("selected"))
            ),
            format!(
                "The working directory changed from {} to {}.",
                js_string(&join("selected")),
                js_string(&join("second"))
            ),
            format!(
                "The working directory {} is unavailable. The working directory is now {}.",
                js_string(&join("second")),
                js_string(&cwd)
            ),
        ]),
    )
}

/// `snapshotGit` (headless.snapshot.ts:906–921): deterministic identity, no hooks or user config.
fn snapshot_git(
    limit: std::time::Duration,
    cwd: &Path,
    args: &[&str],
) -> std::result::Result<String, compare::Failure> {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.autocrlf=false",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args);
    command.env_clear();
    for (key, value) in std::env::vars().filter(|(k, _)| !k.starts_with("GIT_")) {
        command.env(key, value);
    }
    command.envs([
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_SYSTEM", "/dev/null"),
        ("GIT_AUTHOR_NAME", "DSH Snapshot"),
        ("GIT_COMMITTER_NAME", "DSH Snapshot"),
        ("GIT_AUTHOR_EMAIL", "snapshot@example.invalid"),
        ("GIT_COMMITTER_EMAIL", "snapshot@example.invalid"),
        ("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z"),
        ("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z"),
    ]);
    let output = crate::body::run_bounded(&mut command, limit, "snapshot Git failed")?;
    if !output.status.success() {
        return Err(fail(format!(
            "snapshot Git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(hoh_util_values::js_trim(&String::from_utf8_lossy(&output.stdout)).to_owned())
}

fn read_text(path: &Path) -> std::result::Result<String, compare::Failure> {
    std::fs::read_to_string(path).map_err(|e| fail(format!("{}: {e}", path.display())))
}

fn expect_text(label: &str, actual: &str, expected: &str) -> Check {
    expect_eq(label, &json!(actual), &json!(expected))
}

/// headless.snapshot.ts:923–942.
fn worktree(ctx: &BodyContext<'_>) -> Check {
    let cwd = ctx.cwd;
    let checkout = cwd.join(".agents/worktrees/isolated");
    let config = std::fs::read(cwd.join(".git/config")).map_err(|e| fail(e.to_string()))?;
    let before =
        std::fs::read(cwd.join(".git/config.before-worktree")).map_err(|e| fail(e.to_string()))?;
    expect(config == before, || {
        ".git/config changed during the run".into()
    })?;
    expect_text(
        ".git/info/attributes",
        &read_text(&cwd.join(".git/info/attributes"))?,
        WORKTREE_CHECKOUT_ATTRIBUTES,
    )?;
    expect_text(
        "checkout filter",
        &snapshot_git(
            ctx.launch.timeout,
            cwd,
            &["config", "filter.dsh-snapshot-checkout.smudge"],
        )?,
        WORKTREE_CHECKOUT_FILTER,
    )?;
    expect_text(
        "tracked.txt",
        &read_text(&checkout.join("tracked.txt"))?,
        "COMMITTED CONTENT\n",
    )?;
    expect_text(
        "result.txt",
        &read_text(&checkout.join("result.txt"))?,
        "WORKTREE RESULT\n",
    )?;
    expect(!checkout.join("local.txt").exists(), || {
        "local.txt reached the linked checkout".into()
    })?;
    expect_text(
        "worktrees/.gitignore",
        &read_text(&cwd.join(".agents/worktrees/.gitignore"))?,
        "*\n",
    )?;
    expect_text(
        "source branch",
        &snapshot_git(ctx.launch.timeout, cwd, &["branch", "--show-current"])?,
        "main",
    )?;
    expect_text(
        "checkout branch",
        &snapshot_git(ctx.launch.timeout, &checkout, &["branch", "--show-current"])?,
        "isolated",
    )?;
    expect_text(
        "checkout HEAD",
        &snapshot_git(ctx.launch.timeout, &checkout, &["rev-parse", "HEAD"])?,
        &snapshot_git(ctx.launch.timeout, cwd, &["rev-parse", "HEAD"])?,
    )?;
    let listing = snapshot_git(
        ctx.launch.timeout,
        cwd,
        &["worktree", "list", "--porcelain"],
    )?;
    let entry = format!("worktree {}", path_text(&checkout));
    expect(listing.contains(&entry), || {
        format!("git worktree list lacks {entry:?}: {listing:?}")
    })?;
    expect_text(
        "check-ignore",
        &snapshot_git(
            ctx.launch.timeout,
            cwd,
            &["check-ignore", ".agents/worktrees/isolated/result.txt"],
        )?,
        ".agents/worktrees/isolated/result.txt",
    )?;
    let events = rows_of(ctx.actual, 0, "worktree")?;
    let directories: Vec<Value> = of_type(&events, "working-directory/change")
        .iter()
        .map(|e| e["data"]["cwd"].clone())
        .collect();
    expect_eq(
        "working-directory/change",
        &json!(directories),
        &json!([path_text(&checkout), path_text(cwd)]),
    )?;
    let systems = of_type(&events, "system/message").len();
    expect(systems == 1, || {
        format!("expected 1 system/message, got {systems}")
    })
}

/// headless.snapshot.ts:944–1008: resume of a Session recorded under another provider cwd.
fn provider_cwd_resume(ctx: &BodyContext<'_>) -> Check {
    let provider_cwd = ctx.manifest["environment"]["DSH_SNAPSHOT_PROVIDER_CWD"]
        .as_str()
        .ok_or_else(|| fail("provider-cwd: DSH_SNAPSHOT_PROVIDER_CWD is not declared"))?
        .to_owned();
    let primary = ctx
        .actual
        .first()
        .ok_or_else(|| fail("provider-cwd: no persisted Session"))?;
    let primary_rows = rows(primary)?;
    let header = primary_rows.first().cloned().unwrap_or(Value::Null);
    expect(header["cwd"] == provider_cwd.as_str(), || {
        format!(
            "the Session header cwd is {}, not the provider cwd",
            header["cwd"]
        )
    })?;
    expect(header["cwd"] != path_text(ctx.cwd).as_str(), || {
        "the Session header cwd is the host cwd".into()
    })?;
    let session_id = header["id"].as_str().unwrap_or_default().to_owned();
    let other_host = tempfile::Builder::new()
        .prefix("dsh-provider-resume-")
        .tempdir()
        .map_err(|e| fail(e.to_string()))?;
    let launch = |provider: &str, json_output: bool| -> std::result::Result<_, compare::Failure> {
        let mut command = Command::new(ctx.launch.hoh);
        command
            .current_dir(other_host.path())
            .args(["--profile", ctx.launch.profile]);
        for patch in ctx.launch.patches {
            command.arg("--patch").arg(patch);
        }
        command.args(["--session-id", &session_id]);
        if json_output {
            command.arg("--json");
        }
        command.arg(ctx.launch.task).envs(ctx.launch.env);
        command.env("DSH_SNAPSHOT_PROVIDER_CWD", provider);
        crate::body::run_bounded(&mut command, ctx.launch.timeout, "provider-cwd resume")
    };
    let refused = launch(&format!("{provider_cwd}/other"), false)?;
    expect(refused.status.code() == Some(1), || {
        format!(
            "mismatched resume exited {:?}, expected 1",
            refused.status.code()
        )
    })?;
    expect(refused.stdout.is_empty(), || {
        "mismatched resume wrote stdout".into()
    })?;
    let stderr = String::from_utf8_lossy(&refused.stderr);
    let needle = format!("was recorded in \"{provider_cwd}\", not \"{provider_cwd}/other\"");
    expect(stderr.contains(&needle), || {
        format!("mismatched resume stderr lacks {needle:?}: {stderr:?}")
    })?;
    let after_refusal = (ctx.harvest)().map_err(fail)?;
    expect(after_refusal == ctx.actual, || {
        "the refused resume changed the persisted Sessions".into()
    })?;
    let resumed = launch(&provider_cwd, true)?;
    expect(resumed.status.code() == Some(0), || {
        format!(
            "matching resume exited {:?}: {}",
            resumed.status.code(),
            String::from_utf8_lossy(&resumed.stderr)
        )
    })?;
    let output = rows(&String::from_utf8_lossy(&resumed.stdout))?;
    expect_eq(
        "resume session line",
        output.first().unwrap_or(&Value::Null),
        &json!({"type": "session", "sessionId": session_id, "cwd": provider_cwd}),
    )?;
    expect_eq(
        "resume final line",
        output.last().unwrap_or(&Value::Null),
        &json!({
            "type": "final", "sessionId": session_id,
            "reason": {"kind": "completed"}, "text": compare::final_text(&primary_rows),
        }),
    )?;
    let continued = (ctx.harvest)().map_err(fail)?;
    expect(continued.len() == 1, || {
        format!(
            "expected 1 persisted Session after resume, got {}",
            continued.len()
        )
    })?;
    let continued_rows = rows(&continued[0])?;
    expect_eq(
        "resumed header",
        continued_rows.first().unwrap_or(&Value::Null),
        &header,
    )?;
    expect(continued[0].starts_with(primary.as_str()), || {
        "the resumed Session does not extend the recorded content".into()
    })?;
    let turns = of_type(&continued_rows, "turn/start").len();
    expect(turns == 2, || {
        format!("expected 2 turn/start rows, got {turns}")
    })
}

/// headless.snapshot.ts:1122–1150, over the recorded fixture: the three packed row types occur
/// (the first assertion of the source test). The logical equality of the packed and the unpacked
/// recording needs the released V0 decoder run row by row; the strict catalog decode that
/// `execute` performs on every replayed fixture exercises the same decoder, and the log
/// comparison covers the writer output, so the second assertion is not repeated here.
fn packed_chunks(ctx: &BodyContext<'_>) -> Check {
    let fixture = ctx
        .expected
        .first()
        .ok_or_else(|| fail("packed-chunks: no recorded fixture"))?;
    let rows = rows(fixture)?;
    let packed = ["text-chunks", "reasoning-chunks", "tool-call-chunks"];
    let mut kinds: Vec<&str> = rows
        .iter()
        .skip(1)
        .filter_map(|r| r["type"].as_str())
        .filter(|kind| packed.contains(kind))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    expect_eq(
        "packed row types",
        &json!(kinds),
        &json!(["reasoning-chunks", "text-chunks", "tool-call-chunks"]),
    )?;
    packed_equals_unpacked(fixture)
}

/// :1129–1150: the packed fixture, expanded row by row by the released V0 decoder, migrates to
/// the same logical Session as the packed one.
fn packed_equals_unpacked(fixture: &str) -> Check {
    use hoh_session_format::{
        SessionFormatCodec, SessionFormatError, SessionFormatEvent, SessionFormatEventRun,
        SessionFormatMigrationContext, SessionFormatRecovery,
    };
    struct Expanded(Vec<SessionFormatEvent>);
    impl SessionFormatMigrationContext for Expanded {
        fn emit_event(&mut self, event: SessionFormatEvent) -> Result<(), SessionFormatError> {
            self.0.push(event);
            Ok(())
        }
        fn emit_run(
            &mut self,
            run: Box<dyn SessionFormatEventRun>,
        ) -> Result<(), SessionFormatError> {
            self.0.extend(run.expand());
            Ok(())
        }
    }
    let all = rows(fixture)?;
    let (header, body) = all
        .split_first()
        .ok_or_else(|| fail("packed-chunks: empty fixture"))?;
    let mut header = header.clone();
    header["cwd"] = json!("/snapshot");
    let mut decoder = hoh_session_format_v0_to_v1::RELEASED_V0_SESSION_FORMAT_CODEC
        .create_decoder(&header, SessionFormatRecovery::Strict)
        .map_err(|e| fail(e.to_string()))?;
    let mut out = Expanded(Vec::new());
    let mut seq = 0_u64;
    for row in body {
        let mut row = row.clone();
        let kind = row["type"].as_str().unwrap_or("").to_owned();
        if let Some(values) = match kind.as_str() {
            "text-chunks" | "reasoning-chunks" => row["data"]["texts"].as_array(),
            "tool-call-chunks" => row["data"]["args"].as_array(),
            _ => None,
        } {
            let count = values.len() as u64;
            row["seq0"] = json!(seq);
            row["time0"] = json!(0);
            seq += count;
        } else {
            row["seq"] = json!(seq);
            row["time"] = json!(0);
            seq += 1;
        }
        decoder
            .decode_row(row, &mut out)
            .map_err(|e| fail(e.to_string()))?;
    }
    decoder.finish(&mut out).map_err(|e| fail(e.to_string()))?;
    let mut unpacked = hoh_util_values::js_json_stringify(&all[0]);
    unpacked.push('\n');
    for event in out.0 {
        unpacked.push_str(&hoh_util_values::js_json_stringify(&Value::Object(event)));
        unpacked.push('\n');
    }
    expect_eq(
        "packed chunk rows are logically equal to their unpacked recording",
        &compare::normalized_session(fixture)?,
        &compare::normalized_session(&unpacked)?,
    )
}
