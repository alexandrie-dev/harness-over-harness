use hoh_conformance::{
    compare,
    corpus::{self, Scenario},
    jsonl,
    normalize::{Context, Normalizer},
    read, records, runner, workspace,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn refs() -> PathBuf {
    std::env::var_os("HOH_CONFORMANCE_REFS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"))
}
fn all() -> Vec<Scenario> {
    let all = corpus::discover(&refs()).unwrap();
    assert_eq!(all.len(), 242);
    all
}

#[test]
fn identity_fake_process_entire_corpus_logs_headers_stdout_and_exit() {
    let root = tempfile::tempdir().unwrap();
    let all = all();
    let n = Normalizer::new().unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_hoh-conformance-fake"))
        .args(["--refs"])
        .arg(refs())
        .arg("--out")
        .arg(root.path())
        .status()
        .unwrap();
    assert!(status.success());
    let mut differences = Vec::new();
    for s in &all {
        let out = root.path().join(&s.suite).join(&s.name);
        let actual = runner::harvest(&out.join("sessions"), &s.suite).unwrap();
        let ctx = compare::context(&actual, &out.join("workspace").to_string_lossy()).unwrap();
        let expected = s.expected_logs().unwrap();
        for (check, result) in [
            ("log", compare::logs(&n, &expected, &actual, &ctx)),
            ("headers", compare::headers(&n, &all, s, &actual, &ctx)),
        ] {
            if let Err(f) = result {
                std::fs::write(out.join(format!("{check}.diff")), &f.diff).unwrap();
                differences.push(format!(
                    "{}/{}\t{check}\t{}\n{}",
                    s.suite, s.name, f.reason, f.diff
                ));
            }
        }
        if s.suite != "session" {
            compare::catalog_creation_times(&actual).unwrap();
        }
        let fixture = read(&s.fixtures().unwrap()[0]).unwrap();
        let meta = hoh_util_values::js_json_parse(&read(&out.join("meta.json")).unwrap()).unwrap();
        if s.suite == "session" {
            compare::headless_stdout(
                &fixture,
                &read(&out.join("stdout.txt")).unwrap(),
                &read(&out.join("stderr.txt")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                meta["exitCode"],
                compare::expected_exit(&records(&fixture).unwrap())
            );
        } else if s.suite == "acp" {
            assert_eq!(
                n.frames(
                    &read(&s.dir.join("stdout.expected.jsonl")).unwrap(),
                    &Context::default()
                )
                .unwrap(),
                n.frames(&read(&out.join("stdout.txt")).unwrap(), &ctx)
                    .unwrap()
            );
        } else {
            let file = s.dir.join(if s.manifest.get("sessionFormat").is_some() {
                "notifications.current.expected.jsonl"
            } else {
                "notifications.expected.jsonl"
            });
            if file.exists() {
                let notifications = records(&read(&out.join("stdout.txt")).unwrap()).unwrap();
                let version = records(&expected[0]).unwrap()[0]["version"].as_u64();
                let actual = n.notifications(&notifications, &ctx, version).unwrap();
                let mut expected = records(&read(&file).unwrap()).unwrap();
                for notification in &mut expected {
                    if notification["params"]["event"]["type"]
                        == "session-log-deepseek/delivery-accepted"
                        && notification["params"]["event"]["data"]["sessionFormatVersion"].as_u64()
                            == version
                        && version.is_some()
                    {
                        notification["params"]["event"]["data"]["sessionFormatVersion"] =
                            json!("{{sourceSessionFormatVersion}}");
                    }
                }
                if let Err(f) = compare::equal(
                    "frames",
                    "SDK notifications",
                    &json!(expected),
                    &json!(actual),
                ) {
                    differences.push(format!("sdk/{}\tframes\t{}\n{}", s.name, f.reason, f.diff));
                }
            }
        }
    }
    if !differences.is_empty() {
        std::fs::write(
            "/tmp/hoh-cf1-identity-differences.txt",
            differences.join("\n\n"),
        )
        .unwrap();
    }
    assert!(
        differences.is_empty(),
        "{} identity differences; full diffs /tmp/hoh-cf1-identity-differences.txt\n{}",
        differences.len(),
        differences
            .iter()
            .map(|s| s.lines().next().unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn historical_inputs_use_available_migration_then_name_the_missing_edge() {
    for s in all() {
        for file in s.fixtures().unwrap() {
            let raw = read(&file).unwrap();
            let version = records(&raw).unwrap()[0]["version"].as_u64().unwrap_or(0);
            if version < 4 {
                let f = compare::migration_gate(&raw).unwrap_err();
                assert_eq!(f.phase, "input");
                let next = version.max(1);
                assert_eq!(
                    f.reason,
                    format!("migration v{next}->v{} not ported", next + 1),
                    "{}/{} {}",
                    s.suite,
                    s.name,
                    file.display()
                );
            }
        }
    }
}

#[track_caller]
fn must_fail(
    n: &Normalizer,
    expected: &[String],
    mutated: &[String],
    ctx: &Context,
    phase: &str,
    label: &str,
) {
    let f = compare::logs(n, expected, mutated, ctx).expect_err(label);
    assert_eq!(f.phase, phase);
}
#[test]
fn sensitivity_entire_corpus_changed_text_missing_reordered_usage_prompt_tree() {
    let all = all();
    let n = Normalizer::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_hoh-conformance-fake"))
        .arg("--refs")
        .arg(refs())
        .arg("--out")
        .arg(root.path())
        .status()
        .unwrap();
    assert!(status.success());
    let mut tool_count = 0;
    let mut usage_count = 0;
    for s in &all {
        let expected = s.expected_logs().unwrap();
        let out = root.path().join(&s.suite).join(&s.name);
        let actual = runner::harvest(&out.join("sessions"), &s.suite).unwrap();
        let ctx = compare::context(&actual, &out.join("workspace").to_string_lossy()).unwrap();
        let baseline: Vec<Vec<Value>> = actual.iter().map(|s| records(s).unwrap()).collect();
        let mut mutation = baseline.clone();
        let tool = mutation
            .iter_mut()
            .flat_map(|l| l.iter_mut())
            .find(|r| r["type"] == "tool/result");
        if let Some(tool) = tool {
            tool_count += 1;
            if let Some(text) = tool.pointer_mut("/data/message/content/0/text") {
                *text = json!(text.as_str().unwrap_or("").to_owned() + " MUTATED TOOL RESULT");
            } else {
                tool["data"]["unexpectedToolResult"] = json!("MUTATED TOOL RESULT");
            }
        } else {
            mutation[0].push(json!({"type":"tool/result","data":{"message":{"content":[{"type":"text","text":"MUTATED TOOL RESULT"}]}}}));
        }
        must_fail(
            &n,
            &expected,
            &mutation.iter().map(|l| jsonl(l)).collect::<Vec<_>>(),
            &ctx,
            "log",
            &format!("{}/{} changed tool result", s.suite, s.name),
        );
        let mut mutation = baseline.clone();
        mutation[0].pop();
        must_fail(
            &n,
            &expected,
            &mutation.iter().map(|l| jsonl(l)).collect::<Vec<_>>(),
            &ctx,
            "log",
            &format!("{}/{} missing event", s.suite, s.name),
        );
        let mut mutation = baseline.clone();
        let last = mutation[0].len() - 1;
        if last > 1 {
            mutation[0].swap(1, last);
            must_fail(
                &n,
                &expected,
                &mutation.iter().map(|l| jsonl(l)).collect::<Vec<_>>(),
                &ctx,
                "log",
                &format!("{}/{} reordered event", s.suite, s.name),
            );
        } else {
            // These ACP fixtures have no model events; reorder two distinct protocol responses instead.
            let expected = n
                .frames(
                    &read(&s.dir.join("stdout.expected.jsonl")).unwrap(),
                    &Context::default(),
                )
                .unwrap();
            let mut frames = records(&read(&out.join("stdout.txt")).unwrap()).unwrap();
            assert!(frames.len() >= 2);
            frames.swap(0, 1);
            let actual = n.frames(&jsonl(&frames), &ctx).unwrap();
            assert_eq!(
                compare::equal(
                    "frames",
                    "reordered ACP responses",
                    &json!(expected),
                    &json!(actual)
                )
                .unwrap_err()
                .phase,
                "frames"
            );
        }
        fn usage(v: &mut Value) -> bool {
            if let Some(u) = v.get_mut("usage").and_then(Value::as_object_mut)
                && let Some(number) = u.values_mut().find(|v| v.is_number())
            {
                *number = json!(number.as_f64().unwrap() + 1.0);
                return true;
            }
            match v {
                Value::Array(a) => a.iter_mut().any(usage),
                Value::Object(o) => o.values_mut().any(usage),
                _ => false,
            }
        }
        let mut mutation = baseline.clone();
        if mutation.iter_mut().flat_map(|l| l.iter_mut()).any(usage) {
            usage_count += 1;
        } else {
            mutation[0]
                .push(json!({"type":"assistant/message","data":{"usage":{"inputTokens":999}}}));
        }
        must_fail(
            &n,
            &expected,
            &mutation.iter().map(|l| jsonl(l)).collect::<Vec<_>>(),
            &ctx,
            "log",
            &format!("{}/{} usage", s.suite, s.name),
        );
        // Alter a real satellite-expanded system/message, then use the same header verifier.
        let mut expanded = baseline.clone();
        if let Some(prompt) = expanded
            .iter_mut()
            .flat_map(|l| l.iter_mut())
            .find(|r| r["type"] == "system/message")
        {
            if let Some(text) = prompt.pointer_mut("/data/message/content/0/text") {
                *text = json!(text.as_str().unwrap().to_owned() + " MUTATED SYSTEM PROMPT");
            } else {
                prompt["data"]["message"]["content"] =
                    json!([{"type":"text","text":"MUTATED SYSTEM PROMPT"}]);
            }
            let actual: Vec<_> = expanded.iter().map(|l| jsonl(l)).collect();
            assert_eq!(
                compare::headers(&n, &all, s, &actual, &ctx)
                    .unwrap_err()
                    .phase,
                "headers"
            );
        }
        let tree_dir = root.path().join(&s.suite).join(&s.name);
        std::fs::create_dir_all(&tree_dir).unwrap();
        let before = workspace::capture(&tree_dir, &[], false).unwrap();
        std::fs::write(tree_dir.join("extra-file"), "unexpected\n").unwrap();
        let after = workspace::capture(&tree_dir, &[], false).unwrap();
        assert_eq!(
            compare::equal("tree", "working tree", &json!(before), &json!(after))
                .unwrap_err()
                .phase,
            "tree"
        );
    }
    assert!(tool_count > 100);
    assert!(usage_count > 200);
}

#[test]
fn manifest_expected_inventory_and_patches_entire_corpus() {
    let all = all();
    let expected =
        read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/EXPECTED.tsv"))
            .unwrap();
    let expected: Vec<_> = expected
        .lines()
        .skip(1)
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .collect();
    assert_eq!(expected.len(), 242);
    for s in &all {
        let row = expected
            .iter()
            .find(|r| r[0] == s.suite && r[1] == s.name)
            .unwrap();
        assert_eq!(row[2], s.profile());
        assert_eq!(row[3], s.composition());
        assert!(!row[6].is_empty()); // Read the plugin list; it is never a pass oracle.
        for source in corpus::patch_sources(&all, s, &refs()).unwrap() {
            hoh_conformance::yaml::materialize(&read(&source).unwrap(), &source, &refs()).unwrap();
        }
        assert_eq!(
            s.fixtures().unwrap().len(),
            s.expected_logs().unwrap().len()
        );
    }
}

#[test]
fn bin_false_all_three_suites_are_failures_without_runner_errors() {
    std::fs::create_dir_all("/mnt/toshiba/alexandrie/hoh/target-cf1/runs").unwrap();
    let root = tempfile::tempdir_in("/mnt/toshiba/alexandrie/hoh/target-cf1/runs").unwrap();
    let results = root.path().join("false.tsv");
    let output = Command::new(env!("CARGO_BIN_EXE_hoh-conformance"))
        .arg("--hoh")
        .arg("/bin/false")
        .arg("--refs")
        .arg(refs())
        .arg("--runs")
        .arg(root.path())
        .arg("--results")
        .arg(&results)
        .args(["--jobs", "4"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "scenarios: 0 of 242 pass, 242 fail, 0 runner errors"
    );
    let rows = read(&results).unwrap();
    assert_eq!(rows.lines().count(), 242);
    for row in rows.lines() {
        let fields: Vec<_> = row.split('\t').collect();
        assert_eq!(fields[2], "fail");
        assert!(["launch", "exit"].contains(&fields[3]), "{row}");
    }
}
