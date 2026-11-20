// The runs folder is reached through a symlink (`std::os::unix::fs::symlink`).
#![cfg(unix)]
#[path = "support/plans.rs"]
mod plans;
use hoh_conformance::{
    compare,
    corpus::{self, Scenario},
    read, records,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

struct Harness {
    root: tempfile::TempDir,
    /// Owns the symlinked parent of `root` (dropped after it).
    _base: tempfile::TempDir,
    refs: PathBuf,
    all: Vec<Scenario>,
}
impl Harness {
    fn new() -> Self {
        let refs = PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness");
        let all = corpus::discover(&refs).unwrap();
        assert_eq!(all.len(), 242);
        // The runs folder is reached through a symlink on purpose (a target dir linked into tmpfs):
        // the oracle's paths must not depend on how it is spelled.
        let base = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let base_path = std::fs::canonicalize(base.path()).unwrap();
        std::fs::create_dir(base_path.join("real")).unwrap();
        // The link name and its target differ in length (as macOS `/var` -> `/private/var`).
        std::os::unix::fs::symlink(base_path.join("real"), base_path.join("l")).unwrap();
        let root = tempfile::Builder::new()
            .prefix("round-")
            .tempdir_in(base_path.join("l"))
            .unwrap();
        assert_ne!(
            std::fs::canonicalize(root.path())
                .unwrap()
                .as_os_str()
                .len(),
            root.path().as_os_str().len(),
            "the runs folder must be spelled with a path of another length than its canonical form"
        );
        plans::write(&root.path().join("plans"), &refs, &all);
        Self {
            root,
            _base: base,
            refs,
            all,
        }
    }
    fn run(
        &self,
        label: &str,
        mutation: &str,
        selector: Option<(&str, &str)>,
        exit_codes: Value,
    ) -> (PathBuf, Vec<Vec<String>>) {
        self.run_with(label, mutation, selector, exit_codes, true)
    }
    fn run_with(
        &self,
        label: &str,
        mutation: &str,
        selector: Option<(&str, &str)>,
        exit_codes: Value,
        allow_unported_fixtures: bool,
    ) -> (PathBuf, Vec<Vec<String>>) {
        let out = self.root.path().join(label);
        std::fs::create_dir_all(&out).unwrap();
        let control = out.join("fake.json");
        std::fs::write(&control,hoh_util_values::js_json_stringify(&json!({"plans":self.root.path().join("plans").to_string_lossy(),"mutation":mutation,"exitCodes":exit_codes,"shutdownWithoutReply":"sdk/text-turn"}))).unwrap();
        let results = out.join("results.tsv");
        let mut command = Command::new(env!("CARGO_BIN_EXE_hoh-conformance"));
        // The inherited controls and poisoned proxies make environment assertions independent of the shell.
        let plan = hoh_util_values::js_json_parse(
            &read(&self.root.path().join("plans/session/text-turn.json")).unwrap(),
        )
        .unwrap();
        for key in plan["environment"].as_object().unwrap().keys() {
            if key != "HOME" {
                command.env_remove(key);
            }
        }
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ] {
            command.env(key, "http://proxy.invalid:9");
        }
        command
            .env("HOH_CONFORMANCE_FAKE", control)
            .env("DSH_TELEMETRY_DISABLED", "inherited-optout")
            .env("NODE_OPTIONS", "--trace-warnings")
            .env("DEEPSEEK_API_KEY", "inherited-key-must-be-overridden");
        command
            .arg("--hoh")
            .arg(env!("CARGO_BIN_EXE_hoh-conformance-fake"))
            .arg("--refs")
            .arg(&self.refs)
            .arg("--runs")
            .arg(&out)
            .arg("--results")
            .arg(&results)
            .args([
                "--jobs",
                "2",
                "--timeout-ms",
                "120000",
                "--wait-timeout-ms",
                "60000",
                "--request-timeout-ms",
                "120000",
            ]);
        if allow_unported_fixtures {
            // The fake hoh loads no plugin, so the fixtures a scenario names are of no consequence.
            command.arg("--allow-unported-fixtures");
        }
        if let Some((suite, scenario)) = selector {
            command.args(["--suite", suite, "--scenario", scenario]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "runner error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows = read(&results)
            .unwrap()
            .lines()
            .map(|s| s.split('\t').map(str::to_owned).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        assert!(
            read(&results.with_extension("runner-errors.tsv"))
                .unwrap()
                .is_empty()
        );
        (out, rows)
    }
}
fn attempt(out: &Path, suite: &str, name: &str) -> PathBuf {
    PathBuf::from(
        read(&out.join(suite).join(name).join("latest-run.txt"))
            .unwrap()
            .trim(),
    )
}
fn audit(out: &Path, row: &[String]) -> (Vec<Value>, Value) {
    let attempt = attempt(out, &row[0], &row[1]);
    let launch =
        hoh_util_values::js_json_parse(&read(&attempt.join("launch.json")).unwrap()).unwrap();
    let fake = hoh_util_values::js_json_parse(
        &read(&Path::new(launch["cwd"].as_str().unwrap()).join(".dsh/fake-assertions.json"))
            .unwrap(),
    )
    .unwrap();
    for key in ["argv", "environment", "stdinEof"] {
        assert_eq!(fake[key], true, "{}/{} {key}", row[0], row[1]);
    }
    if row[0] == "sdk" {
        assert_eq!(fake["shutdown"], true);
    }
    let checks = attempt.join("checks.jsonl");
    (
        if checks.exists() {
            records(&read(&checks).unwrap()).unwrap()
        } else {
            Vec::new()
        },
        fake,
    )
}

fn has_usage(v: &Value) -> bool {
    if v.get("usage")
        .and_then(Value::as_object)
        .is_some_and(|o| o.values().any(Value::is_number))
    {
        return true;
    }
    match v {
        Value::Array(a) => a.iter().any(has_usage),
        Value::Object(o) => o.values().any(has_usage),
        _ => false,
    }
}
fn expected_gate(s: &Scenario) -> Option<(&'static str, String)> {
    if s.manifest["workspace"]["setup"] == "git-worktree" {
        return Some((
            "setup",
            "check workspace.setup.git-worktree not implemented".into(),
        ));
    }
    let raw = read(&s.fixtures().unwrap()[0]).unwrap();
    let rows = records(&raw).unwrap();
    let model = rows.iter().any(|r| r["type"] == "request/header");
    // The runner reads every replayed fixture strictly under the released catalog (strictV4Catalog).
    if s.suite != "acp" || model {
        for file in s.fixtures().unwrap() {
            if let Err(f) = compare::strict_catalog(&read(&file).unwrap()) {
                return Some((f.phase, f.reason));
            }
        }
    }
    if s.suite == "sdk" {
        return Some((
            "scenario-check",
            format!("check sdk.{} not implemented", s.name),
        ));
    }
    if s.suite == "acp" && !model {
        return None;
    }
    let name = match s.name.as_str() {
        "provider-cwd" => "verifyProviderCwdResume",
        "worktree" => "verifyWorktree",
        "working-directory" => "verifyWorkingDirectory",
        "session-query-spill" => "verifySessionQuerySpill",
        "session-reference-spill" => "sessionReferenceSpillExpected",
        "team-tree-messages" => "verifyTeamTreeMessage",
        "team-task-inbox" => "verifyTeamTaskInbox",
        "advanced-toolchain" => "verifyAdvancedToolchain",
        "mcp-resources" | "mcp-resources-ptc" => "verifyMcpResources",
        "mcp-template-error" => "verifyMcpTemplateError",
        "mcp-empty" | "mcp-empty-ptc" => "verifyNoMcpServers",
        "background-confinement-failure" => "verifyBackgroundConfinementFailure",
        "plugin-manager-mcp" => "pluginManagerMcp",
        "tool-call-turn" => "verifyToolResultWriterParity",
        "packed-chunks" => "packedWriterParity",
        "toolcall-unused-args" => "toolcallUnusedArgs",
        "empty-response-retry-current" => "emptyResponseRetryCurrent",
        "office-progressive-native" | "office-progressive-ptc" => "officeProgressive",
        "subagent-depth-two-rejection" => "subagentDepthTwoRejection",
        _ => return None,
    };
    Some(("scenario-check", format!("check {name} not implemented")))
}

#[test]
fn complete_executable_corpus_identity_and_six_mutations_reach_their_checks() {
    let h = Harness::new();
    let (out, rows) = h.run(
        "identity",
        "",
        None,
        json!({"acp/handshake":1,"sdk/text-turn":1}),
    );
    assert_eq!(rows.len(), 242);
    let mut errors = Vec::new();
    let mut result_checks = 0;
    for row in &rows {
        let s = h
            .all
            .iter()
            .find(|s| s.suite == row[0] && s.name == row[1])
            .unwrap();
        let actual = (row[2].as_str(), row[3].as_str(), row[4].as_str());
        let expected = expected_gate(s);
        let matched = match &expected {
            // A ported body (f1's `body` module) decides the last gate itself: the scenario must reach
            // it (every comparison before it matched), whatever the body concludes about the fake.
            Some(("scenario-check", _)) => ["scenario-check", "body"].contains(&actual.1),
            Some((phase, reason)) => actual == ("fail", *phase, reason.as_str()),
            None => row[2] == "pass",
        };
        if !matched {
            let a = attempt(&out, &row[0], &row[1]);
            errors.push(format!(
                "{}/{}: got {actual:?}; expected {expected:?}; {}",
                row[0],
                row[1],
                read(&a.join("stderr.txt")).unwrap_or_default()
            ));
            continue;
        }
        let (checks, _) = audit(&out, row);
        if expected
            .as_ref()
            .is_some_and(|(phase, _)| *phase == "input")
        {
            assert!(
                checks
                    .iter()
                    .any(|c| c["check"] == "strict-catalog" && c["status"] == "fail")
            );
            assert!(!checks.iter().any(|c| c["check"] == "log"));
            continue;
        }
        for check in [
            "tree",
            if s.suite == "session" {
                "stdout"
            } else if s.suite == "acp" {
                "ACP stdout"
            } else {
                "log"
            },
        ] {
            assert!(
                checks
                    .iter()
                    .any(|r| r["check"] == check && r["status"] == "pass"),
                "missing executed check {check}: {}/{}",
                s.suite,
                s.name
            );
        }
        if s.dir.join("result.expected.json").exists() {
            assert!(
                checks
                    .iter()
                    .any(|r| r["check"] == "SDK result" && r["status"] == "pass")
            );
            result_checks += 1;
        }
    }
    assert!(
        errors.is_empty(),
        "identity did not reach declared gates:\n{}",
        errors.join("\n")
    );
    eprintln!(
        "executable identity: 242 scenarios reached their declared gates; {result_checks} SDK results compared"
    );
    let reachable_results = h
        .all
        .iter()
        .filter(|s| {
            s.dir.join("result.expected.json").exists()
                && !expected_gate(s).is_some_and(|(phase, _)| phase == "input")
        })
        .count();
    assert_eq!(result_checks, reachable_results);
    assert!(result_checks >= 13);
    let mut tool_count = 0;
    let mut usage_count = 0;
    let mut reorder_count = 0;
    for mutation in ["tool", "missing", "reorder", "usage", "prompt", "tree"] {
        let (out, rows) = h.run(mutation, mutation, None, json!({}));
        assert_eq!(rows.len(), 242);
        let mut applicable = 0;
        let mut blocked = 0;
        let mut insensitive = 0;
        for row in &rows {
            let s = h
                .all
                .iter()
                .find(|s| s.suite == row[0] && s.name == row[1])
                .unwrap();
            let logs = s.expected_logs().unwrap();
            let events: Vec<_> = logs.iter().flat_map(|raw| records(raw).unwrap()).collect();
            let model = events.iter().any(|r| r["type"] == "request/header");
            let prompt = events.iter().any(|r| r["type"] == "system/message");
            let (checks, _) = audit(&out, row);
            if let Some((phase, reason)) = expected_gate(s).filter(|(phase, _)| *phase == "input") {
                assert_eq!(
                    (&row[2], row[3].as_str(), &row[4]),
                    (&"fail".to_owned(), phase, &reason)
                );
                assert!(!checks.iter().any(|c| c["check"] == "log"));
                blocked += 1;
                continue;
            }
            let parent = records(&logs[0]).unwrap();
            if mutation != "tree" && s.suite == "acp" && !model {
                assert_eq!(row[2], "pass", "no durable model log target: {row:?}");
                insensitive += 1;
                continue;
            }
            if mutation == "reorder" && parent.len() <= 2 {
                let (phase, reason) = expected_gate(s).unwrap();
                assert_eq!((row[3].as_str(), &row[4]), (phase, &reason));
                insensitive += 1;
                continue;
            }
            let phase = if mutation == "tree" {
                "tree"
            } else if mutation == "prompt" && prompt {
                "headers"
            } else {
                "log"
            };
            assert!(
                row[2] == "fail" && row[3] == phase,
                "mutation {mutation}: {row:?}"
            );
            assert!(checks.iter().any(|c| c["status"] == "fail"));
            applicable += 1;
            let reason = &row[4];
            match mutation {
                "missing" => assert_eq!(
                    reason,
                    &format!(
                        "session role 0 row count: expected {}, got {}",
                        parent.len(),
                        parent.len() - 1
                    )
                ),
                "reorder" => {
                    assert_ne!(parent[1], *parent.last().unwrap());
                    assert_eq!(
                        reason,
                        &format!(
                            "session role 0 row 1: expected type {}, got {}; event type/order mismatch",
                            parent[1]["type"],
                            parent.last().unwrap()["type"]
                        )
                    );
                    reorder_count += 1;
                }
                "tool" => {
                    if let Some(tool) = events.iter().find(|r| r["type"] == "tool/result") {
                        let path = if tool.pointer("/data/message/content/0/text").is_some() {
                            "/data/message/content/0/text"
                        } else {
                            "/data/mutated"
                        };
                        assert!(
                            reason.contains("expected type \"tool/result\", got \"tool/result\""),
                            "{row:?}"
                        );
                        assert!(
                            reason.contains(&format!("value at {path}:"))
                                && reason.ends_with("MUTATED tool\""),
                            "{row:?}"
                        );
                        if tool.pointer("/data/message/content/0/text").is_some() {
                            tool_count += 1;
                        }
                    } else {
                        assert_eq!(
                            reason,
                            &format!(
                                "session role 0 row count: expected {}, got {}; extra type \"fake/tool\"",
                                parent.len(),
                                parent.len() + 1
                            )
                        );
                    }
                }
                "usage" => {
                    if events.iter().any(has_usage) {
                        let (prefix, values) = reason
                            .rsplit_once(": expected ")
                            .filter(|(prefix, _)| {
                                prefix.contains("value at ") && prefix.contains("/usage/")
                            })
                            .unwrap_or_else(|| panic!("not a usage field failure: {row:?}"));
                        assert!(prefix.starts_with("session role "));
                        let (before, after) = values.split_once(", got ").unwrap();
                        let before = hoh_util_values::js_json_parse(before)
                            .unwrap()
                            .as_f64()
                            .unwrap();
                        let after = hoh_util_values::js_json_parse(after)
                            .unwrap()
                            .as_f64()
                            .unwrap();
                        assert_eq!(after, before + 1.0);
                        usage_count += 1;
                    } else {
                        assert_eq!(
                            reason,
                            &format!(
                                "session role 0 row count: expected {}, got {}; extra type \"fake/usage\"",
                                parent.len(),
                                parent.len() + 1
                            )
                        );
                    }
                }
                "prompt" if !prompt => assert_eq!(
                    reason,
                    &format!(
                        "session role 0 row count: expected {}, got {}; extra type \"system/message\"",
                        parent.len(),
                        parent.len() + 1
                    )
                ),
                "prompt" => assert_eq!(reason, "system prompt satellite mismatch"),
                "tree" => assert_eq!(reason, "working tree mismatch"),
                _ => unreachable!("six named mutations"),
            }
        }
        assert_eq!(applicable + blocked + insensitive, 242);
        if mutation != "tree" {
            assert!(insensitive >= 2);
        }
        eprintln!(
            "executable sensitivity: {mutation}: {applicable} detected, {blocked} migration-blocked, {insensitive} not sensitive"
        );
    }
    assert!(tool_count > 100, "real tool fields: {tool_count}");
    assert!(usage_count > 200, "real usage fields: {usage_count}");
    assert!(reorder_count > 200, "reordered scenarios: {reorder_count}");
    eprintln!(
        "real mutation targets: tool={tool_count}, usage={usage_count}, reorder={reorder_count}"
    );
    let (out, rows) = h.run(
        "headless-correct-logs-exit-one",
        "",
        Some(("session", "bash-tool-turn")),
        json!({"session/bash-tool-turn":1}),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (&rows[0][2], &rows[0][3]),
        (&"fail".to_owned(), &"exit".to_owned())
    );
    assert_eq!(rows[0][4], "exit Some(1); expected 0; persisted sessions 1");
    let (checks, _) = audit(&out, &rows[0]);
    assert!(checks.is_empty(), "exit gate must precede comparisons");
    // A v1 fixture is migrated by the real catalog and then compared like a current one.
    let (out, rows) = h.run(
        "historical-migrates",
        "",
        Some(("session", "text-turn")),
        json!({}),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][2], "pass", "{:?}", rows[0]);
    let (checks, _) = audit(&out, &rows[0]);
    assert!(
        checks
            .iter()
            .any(|c| c["check"] == "strict-catalog" && c["status"] == "pass")
    );
    for (suite, name, mutation, phase, reason) in [
        (
            "session",
            "bash-tool-turn",
            "writer-v3",
            "log",
            "current writer generation:",
        ),
        (
            "sdk",
            "text-turn",
            "writer-v3",
            "log",
            "current writer generation:",
        ),
        (
            "sdk",
            "subagent-dsh-sdk-dynamic-route",
            "mixed-generation",
            "frames",
            "SDK wire goldens require one common writer generation (actual)",
        ),
    ] {
        let (_, rows) = h.run(
            &format!("{suite}-{mutation}"),
            mutation,
            Some((suite, name)),
            json!({}),
        );
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0][3] == phase && rows[0][4].starts_with(reason),
            "{rows:?}"
        );
    }
}

/// A `local:` fixture that is not ported stops the run at `launch` (the product would skip the plugin
/// silently and the run would show a divergence of the product instead); the ported ones run.
#[test]
fn an_unported_fixture_is_a_launch_failure_not_a_silent_skip() {
    let h = Harness::new();
    for (suite, name, reason) in [
        (
            "session",
            "subagent-structured-parent",
            Some(
                "фикстура subagent-structured-parent.ts не перенесена (local:packages/test-support/session-snapshot/tests/fixtures/subagent-structured-parent.ts)",
            ),
        ),
        (
            "sdk",
            "desktop-context",
            Some(
                "фикстура clock.mjs не перенесена (local:snapshots/sdk/desktop-context/clock.mjs)",
            ),
        ),
        // The child patches and the profile overlay are checked too, not only the root patches.
        (
            "sdk",
            "subagent-dsh-sdk-dynamic-route",
            Some("*child-mock-llm.ts*"),
        ),
        ("session", "web-fetch", None),
        ("session", "model-switch-notice", None),
        ("session", "bash-tool-turn", None),
    ] {
        let (out, rows) = h.run_with(
            &format!("unported-{name}"),
            "",
            Some((suite, name)),
            json!({}),
            false,
        );
        assert_eq!(rows.len(), 1);
        match reason {
            Some(reason) => {
                let got = (
                    rows[0][2].as_str(),
                    rows[0][3].as_str(),
                    rows[0][4].as_str(),
                );
                match reason.strip_prefix('*').and_then(|r| r.strip_suffix('*')) {
                    Some(part) => {
                        assert!(got.2.contains(part), "{got:?}");
                        assert_eq!((got.0, got.1), ("fail", "launch"));
                    }
                    None => assert_eq!(got, ("fail", "launch", reason)),
                }
                assert!(
                    !attempt(&out, suite, name).join("launch.json").exists(),
                    "the process must not have been started"
                );
            }
            None => assert_ne!(
                rows[0][3], "launch",
                "{name} is fully ported: {:?}",
                rows[0]
            ),
        }
    }
}
