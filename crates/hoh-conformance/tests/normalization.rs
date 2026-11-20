use hoh_conformance::{
    compare, corpus, jsonl,
    normalize::{Context, Normalizer},
    read, records,
};
use serde_json::{Value, json};
use std::path::Path;

fn n() -> Normalizer {
    Normalizer::new().unwrap()
}
fn ctx() -> Context {
    Context {
        cwd: "/tmp/live-workspace".into(),
        aliases: vec!["/real/workspace".into()],
        session_ids: vec!["live-session".into()],
    }
}
fn fixture(suite: &str, name: &str) -> Vec<Value> {
    let root = Path::new("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/snapshots")
        .join(suite)
        .join(name);
    records(&read(&corpus::fixture_files(&root).unwrap()[0]).unwrap()).unwrap()
}

#[test]
fn typed_identities_preserve_shared_parent_child_relationships() {
    let mut logs = vec![fixture("session", "subagent-parallel")];
    let canonical = logs.clone();
    n().identities(&mut logs);
    assert_eq!(logs, canonical);
    let mut logs = vec![
        vec![
            json!({"type":"session","id":"parent"}),
            json!({"type":"subagent/catalog","data":{"childId":"child"}}),
        ],
        vec![json!({"type":"session","id":"child","parentSession":"parent"})],
    ];
    n().identities(&mut logs);
    assert_eq!(logs[0][1]["data"]["childId"], "{{session:2}}");
    assert_eq!(logs[1][0]["parentSession"], "{{session:1}}");
}
#[test]
fn messages_approvals_workflows_commands_rpc_retries_and_jobs_have_distinct_spaces() {
    let id = "fbc26de0-6697-4000-8000-000000000001";
    let mut logs = vec![vec![
        json!({"type":"session","id":"root"}),
        json!({"type":"user/message","data":{"id":id,"role":"user","content":[],"source":{}}}),
        json!({"type":"approval/asked","data":{"id":"fbc26de0-6697-4000-8000-000000000002"}}),
        json!({"type":"fixture","data":{"runId":"fbc26de0-6697-4000-8000-000000000003","commandId":"command-short","rpcId":"rpc-short","retryId":"fbc26de0-6697-4000-8000-000000000004","jobId":"job-0123456789abcdef"}}),
    ]];
    n().identities(&mut logs);
    assert_eq!(logs[0][1]["data"]["id"], "{{message:1}}");
    assert_eq!(logs[0][2]["data"]["id"], "{{approval:1}}");
    for (key, kind) in [
        ("runId", "workflow"),
        ("commandId", "command"),
        ("rpcId", "rpc"),
        ("retryId", "retry"),
        ("jobId", "id"),
    ] {
        assert_eq!(logs[0][3]["data"][key], format!("{{{{{kind}:1}}}}"));
    }
}
#[test]
fn arbitrary_tool_text_and_usage_are_preserved() {
    let mut logs = vec![fixture("session", "bash-tool-turn")];
    logs[0].push(json!({"type":"tool/result","data":{"message":{"content":[{"type":"text","text":"example fbc26de0-6697-4000-8000-000000000099"}]}},"usage":{"tokens":173}}));
    n().identities(&mut logs);
    let last = logs[0].last().unwrap();
    assert_eq!(last["usage"]["tokens"], 173);
    assert!(
        last["data"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("fbc26de0")
    );
}
#[test]
fn cwd_aliases_mac_alias_and_path_boundaries() {
    let n = n();
    let c = ctx();
    assert_eq!(
        n.string("myfile:///tmp/live-workspace/x", &c, false),
        "myfile:///tmp/live-workspace/x"
    );
    for path in [
        "/tmp/live-workspace/x",
        "/real/workspace/x",
        "/private/tmp/live-workspace/x",
        "file:///tmp/live-workspace/x",
    ] {
        assert!(n.string(path, &c, false).contains("{{cwd}}/x"));
    }
    for path in [
        "/tmp/live-workspace-other/x",
        "x/tmp/live-workspace/x",
        "/tmp/live-workspace.txt",
    ] {
        assert_eq!(n.string(path, &c, false), path);
    }
}
#[test]
fn canonical_paths_do_not_rewrite_regex_or_commands() {
    let n = n();
    let c = ctx();
    assert_eq!(
        n.string(
            r"{{cwd}}\dir\a.txt <path>C:\x\a</path> Additional instructions from: C:\x\b",
            &c,
            false
        ),
        "{{cwd}}/dir/a.txt <path>C:/x/a</path> Additional instructions from: C:/x/b"
    );
    assert_eq!(
        n.string(r"printf '\n' and regex \d+", &c, false),
        r"printf '\n' and regex \d+"
    );
    let mut v = json!({"path":r"C:\a\b","other":r"C:\a\b"});
    n.value(&mut v, &c, false);
    assert_eq!(v["path"], "C:/a/b");
    assert_eq!(v["other"], r"C:\a\b");
}
#[test]
fn local_and_snapshot_spill_locators_keep_only_filename() {
    let n = n();
    let c = ctx();
    for path in [
        "{{cwd}}/.spill/session-0123456789ab/0123456789ab-result.txt",
        "/tmp/dsh-acp-snap-012345678/session-0123456789ab/0123456789ab-result.txt",
        "C:\\t\\dsh-acp-snap-012345678\\session-0123456789ab\\0123456789ab-result.txt",
    ] {
        assert_eq!(n.string(path, &c, false), "{{spillLocator:result.txt}}");
    }
    assert_eq!(n.string("saved at /tmp/dsh-acp-snap-012345678/session-0123456789ab/0123456789ab-result.txt. Use read with offset/limit",&c,false),"saved at {{spillLocator:result.txt}}. Use read with offset/limit");
}
#[test]
fn event_read_time_and_omitted_bytes_only_in_target_envelope() {
    let truncated =
        "Session {{session:1}} — text\nTarget event seq 17:\n```json\n{\n  \"time\": 173,\n";
    assert_eq!(n().string(truncated, &ctx(), false), truncated);
    let text = "Session {{session:1}} — text\nTarget event seq 17:\n```json\n{\n  \"time\": 1783600629539,\n  \"text\": \"keep\"\n}\n```\n\n(Omitted 173 bytes.)";
    let result = n().string(text, &ctx(), false);
    assert!(result.contains("\"time\": {{eventTime}},"));
    assert!(result.contains("Omitted {{eventOmittedBytes}} bytes."));
    assert_eq!(
        n().string(
            "```json\n{\n  \"time\": 173,\n}\n```\n\n(Omitted 173 bytes.)",
            &ctx(),
            false
        ),
        "```json\n{\n  \"time\": 173,\n}\n```\n\n(Omitted 173 bytes.)"
    );
}
#[test]
fn session_event_and_embedded_stream_clocks() {
    let mut log = fixture("session", "bash-tool-turn");
    for row in &mut log {
        n().clocks(row).unwrap();
    }
    assert_eq!(log[0]["createdAt"], 0);
    let mut stream = json!({"type":"assistant/message","time":17,"data":{"stream":[{"type":"text-chunks","time0":17,"dt":[1,2,3],"texts":["a","b","c"]},{"type":"chunk","time":18,"chunk":{"type":"usage","usage":{"inputTokens":173}}}]}});
    n().clocks(&mut stream).unwrap();
    assert_eq!(stream["time"], 0);
    assert_eq!(stream["data"]["stream"][0]["dt"], json!([0, 0, 0]));
    assert_eq!(
        stream["data"]["stream"][1]["chunk"]["usage"]["inputTokens"],
        173
    );
}
#[test]
fn packed_row_clocks_preserve_lengths() {
    let mut row = json!({"type":"reasoning-chunks","time0":173,"data":{"dt":[1,2,3],"texts":["first","second","third"]}});
    n().clocks(&mut row).unwrap();
    assert_eq!(row["time0"], 0);
    assert_eq!(row["data"]["dt"], json!([0, 0, 0]));
}
#[test]
fn hook_job_goal_and_catalog_clocks_are_scoped() {
    for (kind, keys) in [
        ("hook/result", vec!["durationMs"]),
        ("job/registered", vec!["startedAt", "finishedAt"]),
        ("job/settled", vec!["startedAt", "finishedAt"]),
        ("goal/change", vec!["createdAt", "updatedAt"]),
        ("subagent/catalog", vec!["childCreatedAt"]),
    ] {
        let mut row = json!({"type":kind,"data":{"durationMs":173,"startedAt":173,"finishedAt":173,"createdAt":173,"updatedAt":173,"childCreatedAt":173,"usage":173}});
        n().clocks(&mut row).unwrap();
        for key in keys {
            assert_eq!(row["data"][key], 0);
        }
        assert_eq!(row["data"]["usage"], 173);
    }
}
#[test]
fn feedback_clocks_and_version_identity() {
    let mut rows = fixture("session", "team-direct-inbox");
    rows.push(json!({"type":"feedback/message-put","data":{"item":{"version":"fbc26de0-6697-4000-8000-000000000099","createdAt":173,"updatedAt":173,"text":"keep"}}}));
    let mut logs = vec![rows];
    n().identities(&mut logs);
    let last = logs[0].last_mut().unwrap();
    n().clocks(last).unwrap();
    assert_eq!(last["data"]["item"]["createdAt"], 0);
    assert!(
        last["data"]["item"]["version"]
            .as_str()
            .unwrap()
            .starts_with("{{id:")
    );
}
#[test]
fn timeline_tree_tasks_receipts_and_tool_meta() {
    let mut row = json!({"type":"plugin:timeline/tree","data":{"tree":{"createdAt":173,"lastTaskUpdate":173,"tasks":[{"createdAt":173,"updatedAt":173,"count":17}]},"receipt":{"at":173,"previousAt":173}}});
    n().clocks(&mut row).unwrap();
    assert_eq!(row["data"]["tree"]["tasks"][0]["count"], 17);
    assert_eq!(row["data"]["receipt"]["at"], 0);
    let mut row = json!({"type":"tool/result","data":{"meta":{"timeline":{"at":173,"previousAt":173}},"message":{"content":[{"text":"173"}]}}});
    n().clocks(&mut row).unwrap();
    assert_eq!(row["data"]["meta"]["timeline"]["at"], 0);
    assert_eq!(row["data"]["message"]["content"][0]["text"], "173");
}
#[test]
fn envelopes_are_removed_references_and_event_order_remain() {
    let log = vec![
        json!({"type":"session","version":4,"id":"{{session:1}}","createdAt":173}),
        json!({"type":"compaction/summary","seq":173,"time":173,"seq0":173,"time0":173,"sourceEventSeqs":[0,[1,3]],"data":{"sourceEventSeq":17,"headerSeq":13,"messageSeqs":[11,12],"surfaceOp":{"op":"replace","startSeq":1,"endSeq":2}}}),
    ];
    let projected = n().logs(&[jsonl(&log)], &ctx()).unwrap();
    let event = &projected[0][1];
    for key in ["seq", "time", "seq0", "time0"] {
        assert!(event.get(key).is_none());
    }
    assert_eq!(event["sourceEventSeqs"], json!([0, 1, 2, 3]));
    assert_eq!(event["data"], log[1]["data"]);
}
#[test]
fn system_prompt_bulk_is_scrubbed_only_in_first_block() {
    let mut log = fixture("session", "bash-tool-turn");
    let system = log
        .iter_mut()
        .find(|r| r["type"] == "system/message")
        .unwrap();
    system["data"]["message"]["content"][0]["text"] = json!("exact satellite text");
    let projected = n().logs(&[jsonl(&log)], &ctx()).unwrap();
    assert_eq!(
        projected[0]
            .iter()
            .find(|r| r["type"] == "system/message")
            .unwrap()["data"]["message"]["content"][0]["text"],
        "{{system}}"
    );
}
#[test]
fn developer_messages_retain_ordered_tool_names() {
    let mut log = fixture("session", "bash-tool-turn");
    log.push(json!({"type":"developer/message","data":{}}));
    log.iter_mut()
        .find(|r| r["type"] == "request/header")
        .unwrap()["data"]["header"]["tools"] =
        json!([{"name":"bash","description":"precise"},{"name":"write"}]);
    let projected = n().logs(&[jsonl(&log)], &ctx()).unwrap();
    assert_eq!(
        projected[0]
            .iter()
            .find(|r| r["type"] == "request/header")
            .unwrap()["data"]["header"]["tools"],
        json!(["bash", "write"])
    );
}
#[test]
fn native_delivery_generation_only_matches_source_generation() {
    let log = vec![
        json!({"type":"session","version":4,"id":"root"}),
        json!({"type":"session-log-deepseek/delivery-accepted","data":{"sessionFormatVersion":4}}),
        json!({"type":"session-log-deepseek/delivery-accepted","data":{"sessionFormatVersion":3}}),
    ];
    let projected = n().logs(&[jsonl(&log)], &ctx()).unwrap();
    assert!(projected[0][0].get("version").is_none());
    assert_eq!(
        projected[0][1]["data"]["sessionFormatVersion"],
        "{{sourceSessionFormatVersion}}"
    );
    assert_eq!(projected[0][2]["data"]["sessionFormatVersion"], 3);
}
#[test]
fn acp_usage_tokens_and_legacy_message_ids() {
    let mut v = json!({"sessionUpdate":"usage_update","used":173,"total":999,"messageId":"whatever","sessionId":"live-session"});
    n().value(&mut v, &ctx(), true);
    assert_eq!(v["used"], "{{usedTokens}}");
    assert_eq!(v["total"], 999);
    assert_eq!(v["messageId"], "{{messageId}}");
    assert_eq!(v["sessionId"], "{{sessionId}}");
}
#[test]
fn rpc_ids_share_one_namespace_preserve_string_vs_number() {
    let frames = n()
        .frames(
            "{\"id\":7}\n{\"id\":\"7\"}\n{\"id\":7}\n{\"id\":null}\n",
            &ctx(),
        )
        .unwrap();
    assert_eq!(frames[0]["id"], 1);
    assert_eq!(frames[1]["id"], 2);
    assert_eq!(frames[2]["id"], 1);
    assert_eq!(frames[3]["id"], Value::Null);
    assert!(n().frames("ordinary stdout\n", &ctx()).is_err());
}
#[test]
fn satellites_prompt_changes_and_exact_strings() {
    let file = Path::new(
        "/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/snapshots/session/text-turn/system-prompt.expected.md",
    );
    let prompt = read(file).unwrap();
    assert_eq!(
        compare::format_prompts(&compare::prompt_parts(&prompt).unwrap()),
        prompt
    );
    let prompts = vec!["one".into(), "two\n".into()];
    assert_eq!(
        compare::format_prompts(&prompts),
        "one\n\n<!-- system/message change 1 -->\n\ntwo\n"
    );
}
#[test]
fn fixture_selection_generations_roles_and_header_mismatch() {
    let root = tempfile::tempdir().unwrap();
    let write = |name: &str, version: u64| {
        std::fs::write(
            root.path().join(name),
            format!("{{\"type\":\"session\",\"version\":{version}}}\n"),
        )
        .unwrap()
    };
    write("session.v3.jsonl", 3);
    write("session.v4.jsonl", 4);
    write("session.1.v4.jsonl", 4);
    assert_eq!(corpus::fixture_files(root.path()).unwrap().len(), 2);
    write("session.3.v4.jsonl", 4);
    assert!(
        corpus::fixture_files(root.path())
            .unwrap_err()
            .contains("contiguous")
    );
    std::fs::remove_file(root.path().join("session.3.v4.jsonl")).unwrap();
    write("session.v4.jsonl", 3);
    assert!(corpus::fixture_files(root.path()).is_err());
}

mod v1_rules {
    use hoh_conformance::normalize::v1;
    use serde_json::{Value, json};

    #[test]
    fn foreign_prompt_paragraphs_are_dropped_and_others_kept() {
        let prompt = "Intro.\n\nUse the glob tool — not shell find — to discover files by path pattern.\n\nUse the grep tool — not shell grep or rg — to search file contents. Use read on a matched file when you need surrounding context.\n\nTrack every background job id you start. Keep it.\n\nUse the workflow tool ONLY when the user explicitly asks for a workflow or for large orchestration.\n\nTail.\n";
        assert_eq!(
            v1::prompt(prompt),
            "Intro.\n\nTrack every background job id you start. Keep it.\n\nTail.\n"
        );
    }

    #[test]
    fn a_dropped_last_paragraph_keeps_the_closing_newline() {
        let prompt =
            "Intro.\n\nUse the glob tool — not shell find — to discover files by path pattern.\n";
        assert_eq!(v1::prompt(prompt), "Intro.\n");
        assert!(v1::prompt(prompt).ends_with('\n'));
        // A prompt with nothing foreign is returned byte for byte.
        assert_eq!(v1::prompt("A\n\nB\n"), "A\n\nB\n");
    }

    #[test]
    fn a_similar_but_different_paragraph_is_not_dropped() {
        let text = "Use the glob tool differently.\n\nuse the grep tool — not shell grep or rg — lower case\n";
        assert_eq!(v1::prompt(text), text);
    }

    #[test]
    fn foreign_tools_are_dropped_from_a_schema_set_and_the_rest_is_untouched() {
        let set = json!([
            {"name": "read", "description": "Use glob for patterns."},
            {"name": "glob"}, {"name": "grep"}, {"name": "workflow"},
            {"name": "job_list"}, {"name": "run_code"}
        ]);
        assert_eq!(
            v1::tools(&set),
            json!([
                {"name": "read", "description": "Use glob for patterns."},
                {"name": "job_list"}, {"name": "run_code"}
            ])
        );
        assert_eq!(v1::tools(&Value::Null), Value::Null);
    }

    #[test]
    fn foreign_tools_are_dropped_from_recorded_header_rows_in_both_forms() {
        let mut rows = vec![
            json!({"type":"request/header","data":{"header":{"tools":["read","glob","grep","workflow","bash"]}}}),
            json!({"type":"request/header","data":{"header":{"tools":[{"name":"glob"},{"name":"read"}]}}}),
            json!({"type":"request/header","data":{"header":{"model":"m"}}}),
            json!({"type":"tool/call","data":{"tools":["glob"]}}),
        ];
        v1::header_rows(&mut rows);
        assert_eq!(rows[0]["data"]["header"]["tools"], json!(["read", "bash"]));
        assert_eq!(rows[1]["data"]["header"]["tools"], json!([{"name":"read"}]));
        assert_eq!(rows[2]["data"]["header"], json!({"model":"m"}));
        assert_eq!(
            rows[3]["data"]["tools"],
            json!(["glob"]),
            "only request/header rows are touched"
        );
    }

    #[test]
    fn exclusion_patch_disables_every_foreign_row_and_nothing_else() {
        let patch: Value = serde_json::from_str(&v1::exclusion_patch()).unwrap();
        let rows = patch.as_array().unwrap();
        assert_eq!(rows.len(), v1::FOREIGN_PLUGINS.len());
        for (row, (id, _)) in rows.iter().zip(v1::FOREIGN_PLUGINS) {
            assert_eq!(row["id"], *id);
            assert_eq!(row["disabled"], true);
            assert_eq!(
                row.as_object().unwrap().len(),
                3,
                "only id, disabled, insert"
            );
        }
        let ids: std::collections::HashSet<_> =
            v1::FOREIGN_PLUGINS.iter().map(|(i, _)| *i).collect();
        assert_eq!(ids.len(), v1::FOREIGN_PLUGINS.len(), "ids are unique");
    }

    #[test]
    fn no_foreign_plugin_is_inside_the_118() {
        // The rows of the first version that are still being written must stay un-normalized.
        for inside in ["tool-jobs", "jobs-local"] {
            assert!(
                v1::FOREIGN_PLUGINS.iter().all(|(id, _)| *id != inside),
                "{inside} is inside the 118"
            );
        }
        assert!(
            !v1::FOREIGN_PROMPT_PARAGRAPHS
                .iter()
                .any(|(prefix, _)| prefix.starts_with("Track every background job")),
            "the background-job paragraph belongs to tool-jobs"
        );
    }

    #[test]
    fn every_rule_has_a_section_in_normalization_md() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/NORMALIZATION.md"))
                .unwrap();
        for rule in v1::RULES {
            assert!(
                text.contains(&format!("### {rule}\n")),
                "{rule} has no section"
            );
        }
        let sections = text.lines().filter(|l| l.starts_with("### N-")).count();
        assert_eq!(
            sections,
            v1::RULES.len(),
            "a section without a rule in code"
        );
    }

    #[test]
    fn provider_replacement_adds_no_normalization_rule() {
        assert!(v1::RULES.iter().all(|rule| !rule.contains("PROVIDER")));
    }
}

/// Review 220412 №6: cases of `normalize.spec.ts` / `suite.spec.ts` whose function exists in Rust.
mod review_220412 {
    use super::{ctx, n};
    use hoh_conformance::{compare, corpus, normalize::scrub_bulk};
    use serde_json::{Value, json};

    const HEADERLESS: &str = "session snapshot must start with a session header";

    fn lines(rows: &[Value]) -> String {
        rows.iter().map(|r| format!("{r}\n")).collect()
    }

    /// normalize.spec.ts:252.
    #[test]
    fn blank_lines_in_a_frame_stream_are_ignored() {
        let raw = format!("\n{}\n\n", json!({"jsonrpc":"2.0","id":1,"method":"m"}));
        let frames = n().frames(&raw, &ctx()).unwrap();
        assert_eq!(frames.len(), 1);
    }

    /// normalize.spec.ts:534: only `hook/result` loses its duration.
    #[test]
    fn a_non_hook_duration_is_left_untouched() {
        let mut event = json!({"type":"tool/result","seq":2,"time":5,"data":{"durationMs":88}});
        n().clocks(&mut event).unwrap();
        assert_eq!(event["data"]["durationMs"], 88);
        let mut hook = json!({"type":"hook/result","seq":2,"time":5,"data":{"durationMs":88}});
        n().clocks(&mut hook).unwrap();
        assert_eq!(hook["data"]["durationMs"], 0);
    }

    /// normalize.spec.ts:799, :999 and scrub.spec :1268 (one path in Rust: `Normalizer::logs`).
    #[test]
    fn a_snapshot_must_start_with_a_session_header() {
        for raw in [
            lines(&[json!({"type":"turn/start","data":{"turn":1}})]),
            "{\"type\":\"turn/start\"}\n".to_owned(),
        ] {
            let error = n().logs(&[raw], &ctx()).unwrap_err();
            assert_eq!(error, HEADERLESS);
        }
        assert_eq!(compare::strict_catalog("").unwrap_err().reason, HEADERLESS);
    }

    fn system(content: Value) -> Value {
        json!({"type":"system/message","seq":1,"time":2,"data":{"message":{"content":content}}})
    }

    /// normalize.spec.ts:1177: presence is behavior.
    #[test]
    fn an_absent_tools_field_stays_absent_and_an_empty_system_prompt_empty() {
        let mut empty = system(json!([]));
        scrub_bulk(&mut empty, false);
        assert_eq!(empty["data"]["message"]["content"], json!([]));
        let mut header = json!({"type":"request/header","seq":2,"time":2,
            "data":{"header":{"config":{"model":"m"}},"reason":"initial"}});
        let before = header.clone();
        scrub_bulk(&mut header, false);
        assert_eq!(header, before);
        assert!(header["data"]["header"].get("tools").is_none());
    }

    /// normalize.spec.ts:1184: records with nothing to scrub are byte-identical.
    #[test]
    fn malformed_records_without_a_scrubbable_payload_are_unchanged() {
        let records = [
            json!({"type":"request/header","seq":10,"time":9,"data":{"reason":"initial"}}),
            json!({"type":"request/header","seq":11,"time":9,"data":null}),
            json!({"type":"system/message","seq":12,"time":9,"data":{"turn":1,"step":1}}),
            json!({"type":"system/message","seq":13,"time":9,
                "data":{"message":{"content":[{"type":"image","data":"x"}]}}}),
        ];
        for record in records {
            let mut scrubbed = record.clone();
            scrub_bulk(&mut scrubbed, false);
            assert_eq!(scrubbed, record);
        }
    }

    /// suite.spec.ts:519, :523.
    #[test]
    fn a_primary_only_scenario_is_accepted_and_a_missing_primary_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("session.1.jsonl"),
            "{\"type\":\"session\"}\n",
        )
        .unwrap();
        assert_eq!(
            corpus::fixture_files(dir.path()).unwrap_err(),
            "missing parent session fixture"
        );
        std::fs::write(dir.path().join("session.jsonl"), "{\"type\":\"session\"}\n").unwrap();
        std::fs::remove_file(dir.path().join("session.1.jsonl")).unwrap();
        let files = corpus::fixture_files(dir.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("session.jsonl"));
    }

    /// suite.spec.ts:1626.
    #[test]
    fn a_catalog_creation_time_that_disagrees_with_the_child_header_is_rejected() {
        let log = |parent_time: u64| {
            vec![
                lines(&[
                    json!({"type":"session","id":"parent","createdAt":1}),
                    json!({"type":"subagent/catalog","data":{"childId":"child","childCreatedAt":parent_time}}),
                ]),
                lines(&[json!({"type":"session","id":"child","createdAt":100})]),
            ]
        };
        assert!(compare::catalog_creation_times(&log(100)).is_ok());
        let failure = compare::catalog_creation_times(&log(200)).unwrap_err();
        assert!(failure.reason.contains("disagrees with child header"));
        // A partial corpus (the child is absent) and unrelated rows are not judged.
        let partial = vec![log(200).remove(0)];
        assert!(compare::catalog_creation_times(&partial).is_ok());
    }

    /// normalize.ts:659 (review 222017 №2): only a string `text` becomes `{{system}}`; the log and
    /// bulk paths agree.
    #[test]
    fn a_non_string_system_text_is_not_replaced_by_the_token() {
        let header = json!({"type":"session","version":4,"id":"live-session","createdAt":1});
        let record = |text: Value| {
            json!({"type":"system/message","seq":1,"time":2,
                "data":{"message":{"content":[{"type":"text","text":text}]}}})
        };
        let logs = n()
            .logs(
                &[lines(&[
                    header.clone(),
                    record(Value::Null),
                    record(json!(5)),
                    record(json!("secret prompt")),
                ])],
                &ctx(),
            )
            .unwrap();
        let texts: Vec<&Value> = logs[0][1..]
            .iter()
            .map(|r| &r["data"]["message"]["content"][0]["text"])
            .collect();
        assert_eq!(texts, [&Value::Null, &json!(5), &json!("{{system}}")]);
    }

    /// identity.spec.ts:74 (review 222017 №6): the first record only reserves an id when it is a session.
    #[test]
    fn a_first_record_that_is_not_a_session_reserves_no_identity() {
        let mut logs = vec![vec![json!({"type":"not-a-session","id":"s1"})]];
        let before = logs.clone();
        n().identities(&mut logs);
        assert_eq!(logs, before);
    }

    /// normalize.spec.ts:842 (review 230043 №4).
    #[test]
    fn a_source_delivery_marker_claiming_the_target_generation_is_refused() {
        let log = |version: u64, delivery: u64| {
            lines(&[
                json!({"type":"session","version":version,"id":"delivery","createdAt":1,"isSeeded":false,"delegationDepth":0}),
                json!({"type":"turn/start","seq":0,"time":1,"data":{"turn":1}}),
                json!({"type":"session-log-deepseek/delivery-accepted","seq":1,"time":2,
                    "data":{"sessionId":"delivery","throughSeq":0,"sessionFormatVersion":delivery}}),
            ])
        };
        assert_eq!(
            n().logs(&[log(3, 4)], &ctx()).unwrap_err(),
            "format v3 delivery marker claims target format v4"
        );
        assert_eq!(
            n().logs(&[log(2, 3)], &ctx()).unwrap_err(),
            "format v2 delivery marker claims target format v3"
        );
        // Its own generation and future generations are retained.
        for (version, delivery) in [(3, 3), (3, 99), (4, 4)] {
            assert!(n().logs(&[log(version, delivery)], &ctx()).is_ok());
        }
    }
}
