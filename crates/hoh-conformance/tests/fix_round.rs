use hoh_conformance::{
    compare,
    corpus::Scenario,
    jsonl,
    normalize::{Context, Normalizer},
    read,
};
use serde_json::json;
use std::{path::PathBuf, process::Command};

#[test]
fn acp_golden_handshake_accepts_nonzero_exit_after_stdin_eof() {
    let root = tempfile::tempdir().unwrap();
    let control = root.path().join("fake.json");
    std::fs::write(
        &control,
        hoh_util_values::js_json_stringify(
            &json!({"refs":"/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness","exitCode":1}),
        ),
    )
    .unwrap();
    let results = root.path().join("results.tsv");
    let output = Command::new(env!("CARGO_BIN_EXE_hoh-conformance"))
        .env("HOH_CONFORMANCE_FAKE", &control)
        .arg("--hoh")
        .arg(env!("CARGO_BIN_EXE_hoh-conformance-fake"))
        // The fake hoh loads no plugin, so the fixtures the scenario names are of no consequence.
        .arg("--allow-unported-fixtures")
        .args([
            "--refs",
            "/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness",
            "--suite",
            "acp",
            "--scenario",
            "handshake",
        ])
        .arg("--runs")
        .arg(root.path())
        .arg("--results")
        .arg(&results)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let row = read(&results).unwrap();
    assert_eq!(row.split('\t').nth(2), Some("pass"), "{row}");
}

#[test]
fn legacy_job_uuid_in_header_and_prompt_skips_reserved_id_token() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    let mut rows = vec![
        json!({"type":"session","version":4,"id":"root","createdAt":0,"cwd":"{{cwd}}"}),
        json!({"type":"system/message","data":{"message":{"content":[{"type":"text","text":"{{system}}"}]}}}),
        json!({"type":"request/header","data":{"header":{"tools":"{{tools}}","job":"{{id:2}}"}}}),
        json!({"type":"job/registered","data":{"jobId":"{{id:2}}"}}),
        json!({"type":"job/settled","data":{"jobId":"{{id:2}}"}}),
        json!({"type":"fixture","data":{"text":"pre-existing {{id:1}}"}}),
    ];
    std::fs::write(dir.join("session.v4.jsonl"), jsonl(&rows)).unwrap();
    std::fs::write(dir.join("system-prompt.expected.md"), "Job {{id:2}}\n").unwrap();
    std::fs::write(
        dir.join("tool-schemas.expected.json"),
        "{\"initial\":[],\"changes\":[]}\n",
    )
    .unwrap();
    let scenario = Scenario {
        suite: "session".into(),
        name: "job-test".into(),
        dir: PathBuf::from(dir),
        manifest: json!({"profile":"headless","composition":"job-test","header":{"class":"job-test","pin":true}}),
    };
    let job = "fbc26de0-6697-4000-8000-000000000099";
    rows[1]["data"]["message"]["content"][0]["text"] = json!(format!("Job {job}"));
    rows[2]["data"]["header"]["tools"] = json!([]);
    for index in [2, 3, 4] {
        rows[index]["data"][if index == 2 { "header" } else { "jobId" }] = if index == 2 {
            json!({"tools":[],"job":job})
        } else {
            json!(job)
        };
    }
    let normalized = Normalizer::new()
        .unwrap()
        .legacy_log(&jsonl(&rows), &Context::default())
        .unwrap();
    assert_eq!(normalized[2]["data"]["header"]["job"], "{{id:2}}");
    assert_eq!(normalized[3]["data"]["jobId"], "{{id:2}}");
    assert_eq!(normalized[4]["data"]["jobId"], "{{id:2}}");
    assert_eq!(normalized[5]["data"]["text"], "pre-existing {{id:1}}");
    compare::headers(
        &Normalizer::new().unwrap(),
        std::slice::from_ref(&scenario),
        &scenario,
        &[jsonl(&rows)],
        &Context {
            cwd: "{{cwd}}".into(),
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn current_writer_and_common_sdk_wire_generations_are_independent_guards() {
    let log = |version| jsonl(&[json!({"type":"session","version":version,"id":"test"})]);
    assert!(compare::current_writer_generation(&[log(4)]).is_ok());
    let error = compare::current_writer_generation(&[log(4), log(3)]).unwrap_err();
    assert_eq!(error.phase, "log");
    assert!(error.reason.contains("role 1 wrote Some(3); expected v4"));
    assert!(compare::sdk_wire_generation(&[log(3), log(3)], "expected").is_ok());
    for side in ["actual", "expected"] {
        let error = compare::sdk_wire_generation(&[log(3), log(4)], side).unwrap_err();
        assert_eq!(error.phase, "frames");
        assert!(
            error
                .reason
                .contains(&format!("one common writer generation ({side})"))
        );
    }
}
