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
