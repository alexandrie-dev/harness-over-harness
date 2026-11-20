//! `suite.spec.ts` 943–1640: message-id stabilization, refresh replacements, refresh log
//! stabilization and the raw catalog clocks.
use hoh_conformance::{
    normalize::{Context, tokenize_session_fixture_cwd},
    refresh::*,
    sidecars::fixture_context,
};
use serde_json::{Value, json};

const FRESH: &str = "11111111-1111-4111-8111-111111111111";
const OLD: &str = "22222222-2222-4222-8222-222222222222";

fn lines(rows: &[Value]) -> String {
    rows.iter().map(|r| format!("{r}\n")).collect()
}
fn parse(text: &str) -> Vec<Value> {
    text.trim()
        .split('\n')
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn stabilize_ids(fresh: &[String], existing: &[String]) -> Vec<String> {
    stabilize_fixture_message_ids(fresh, existing).unwrap()
}
fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}
fn stabilize(fresh: &str, existing: &str) -> String {
    stabilize_with(fresh, existing, &[], &fixture_context(fresh).unwrap())
}
fn stabilize_with(
    fresh: &str,
    existing: &str,
    replacements: &[Replacement],
    ctx: &Context,
) -> String {
    stabilize_refresh_log(fresh, existing, replacements, ctx).unwrap()
}
fn pair(a: &str, b: &str) -> Replacement {
    (a.to_owned(), b.to_owned())
}
fn user(id: &str, text: &str) -> Value {
    json!({"type":"user/message","data":{"id":id,"role":"user",
        "content":[{"type":"text","text":text}],"source":{"kind":"user"}}})
}

/// :944
#[test]
fn reuses_one_committed_message_uuid_across_fixture_ready_parent_and_child_logs() {
    let log = |session: &str, id: &str| {
        lines(&[
            json!({"type":"session","id":session,"cwd":"{{cwd}}"}),
            user(id, "same"),
        ])
    };
    let fresh = [log("fresh-parent", FRESH), log("fresh-child", FRESH)];
    let existing = [log("old-parent", OLD), log("old-child", OLD)];
    let stable = stabilize_ids(&fresh, &existing);
    assert_eq!(stable.len(), 2);
    for fixture in stable {
        assert!(fixture.contains(&format!("\"id\":\"{OLD}\"")));
        assert!(!fixture.contains(FRESH));
    }
}

/// :967
#[test]
fn stabilizes_developer_message_identities_through_their_explicit_surface_case() {
    let log = |id: &str| {
        lines(&[
            json!({"type":"developer/message","data":{"turn":1,"step":1,"message":{
            "id":id,"role":"developer","source":{"kind":"tool-registry"},
            "content":[{"type":"tool-addition","toolName":"search"}]}}}),
        ])
    };
    assert_eq!(stabilize_ids(&[log(FRESH)], &[log(OLD)]), vec![log(OLD)]);
}

/// :976
#[test]
fn rewrites_only_complete_messages_carried_by_surface_events_or_durable_inbox_splices() {
    let ids = [
        ("fu", "11111111-1111-4111-8111-111111111111"),
        ("ou", "22222222-2222-4222-8222-222222222222"),
        ("fa", "33333333-3333-4333-8333-333333333333"),
        ("oa", "44444444-4444-4444-8444-444444444444"),
        ("ft", "55555555-5555-4555-8555-555555555555"),
        ("ot", "66666666-6666-4666-8666-666666666666"),
        ("om", "77777777-7777-4777-8777-777777777777"),
    ];
    let id = |k: &str| ids.iter().find(|(n, _)| *n == k).unwrap().1;
    let message = |id: &str, role: &str, text: &str| {
        json!({"id":id,"role":role,"content":[{"type":"text","text":text}],
            "source":{"kind": if role == "user" {"user"} else {"model"}}})
    };
    let with_null_source = |id: &str, text: &str| {
        let mut m = message(id, "user", text);
        m["source"] = Value::Null;
        m
    };
    let log = |user_id: &str, assistant: &str, tool: &str, malformed: &str| {
        lines(&[
            json!({"type":"session","id":"same","cwd":"{{cwd}}"}),
            json!({"type":"agent/inbox/spliced","data":{"inserted":[
                message(user_id,"user","user"), with_null_source(user_id,"malformed inbox")]}}),
            json!({"type":"user/message","data":message(user_id,"user","user")}),
            json!({"type":"assistant/message","data":{"message":message(assistant,"assistant","assistant")}}),
            json!({"type":"tool/result","data":{"message":message(tool,"tool","tool")}}),
            json!({"type":"turn/start","data":{"id":user_id}}),
            json!({"type":"steering/message","data":message(user_id,"user","obsolete")}),
            json!({"type":"user/message","data":with_null_source(user_id,"malformed")}),
            json!({"type":"user/message","data":message(malformed,"user","non-UUID")}),
            json!({"type":"assistant/message","data":null}),
            json!({"type":42,"data":message(user_id,"user","non-string type")}),
        ])
    };
    let stable = stabilize_ids(
        &[log(id("fu"), id("fa"), id("ft"), "not-a-uuid")],
        &[log(id("ou"), id("oa"), id("ot"), id("om"))],
    );
    let records = parse(&stable[0]);
    assert_eq!(records[1]["data"]["inserted"][0]["id"], id("ou"));
    assert_eq!(records[1]["data"]["inserted"][1]["id"], id("fu"));
    assert_eq!(records[2]["data"]["id"], id("ou"));
    assert_eq!(records[3]["data"]["message"]["id"], id("oa"));
    assert_eq!(records[4]["data"]["message"]["id"], id("ot"));
    assert_eq!(records[5]["data"]["id"], id("fu"));
    assert_eq!(records[6]["data"]["id"], id("fu"));
    assert_eq!(records[7]["data"]["id"], id("fu"));
    assert_eq!(records[8]["data"]["id"], "not-a-uuid");
}

/// :1033
#[test]
fn matches_cwd_bearing_messages_only_after_the_fresh_log_reaches_fixture_ready_form() {
    let fresh_cwd = "/tmp/acp-snapshot-fresh-cwd";
    let message = |id: &str, path: &str| user(id, &format!("read {path}/input.txt"));
    let fresh = tokenize_session_fixture_cwd(&lines(&[
        json!({"type":"session","id":"fresh","cwd":fresh_cwd}),
        message(FRESH, fresh_cwd),
    ]))
    .unwrap();
    let existing = lines(&[
        json!({"type":"session","id":"old","cwd":"{{cwd}}"}),
        message(OLD, "{{cwd}}"),
    ]);
    assert!(stabilize_ids(&[fresh], &[existing])[0].contains(&format!("\"id\":\"{OLD}\"")));
}

/// :1060
#[test]
fn rejects_a_fingerprint_connected_to_an_id_that_also_identifies_different_content() {
    let conflicting = "22222222-2222-4222-8222-222222222222";
    let competing = "33333333-3333-4333-8333-333333333333";
    let fresh = lines(&[user(FRESH, "shared")]);
    let existing = lines(&[
        user(conflicting, "shared"),
        user(conflicting, "different"),
        user(competing, "shared"),
    ]);
    assert_eq!(
        stabilize_ids(std::slice::from_ref(&fresh), &[existing]),
        vec![fresh]
    );
}

/// :1079
#[test]
fn leaves_fresh_fixtures_unchanged_when_no_committed_counterpart_exists() {
    let fresh = "{\"type\":\"session\",\"id\":\"new\"}\n".to_owned();
    assert_eq!(
        stabilize_ids(std::slice::from_ref(&fresh), &[String::new()]),
        vec![fresh]
    );
}

/// :1086
#[test]
fn maps_fresh_ids_and_cwd_values_to_the_existing_fixture_values_skipping_non_replacements() {
    let logs = strings(&[
        "{\"type\":\"session\",\"id\":\"\",\"cwd\":\"/same\"}\n",
        "{\"type\":\"session\",\"id\":\"new-parent\",\"cwd\":\"/new\"}\n",
        "{\"type\":\"session\",\"id\":\"new-child\",\"cwd\":\"/new\"}\n",
    ]);
    let fixtures = strings(&[
        "{\"type\":\"session\",\"id\":\"\",\"cwd\":\"/same\"}\n",
        "{\"type\":\"session\",\"id\":\"old-parent\",\"cwd\":\"/old\"}\n",
    ]);
    assert_eq!(
        refresh_fixture_replacements(&logs, &fixtures).unwrap(),
        vec![pair("new-parent", "old-parent"), pair("/new", "/old")]
    );
}

/// :1103
#[test]
fn stabilizes_moved_snapshot_spill_paths_by_filename_while_skipping_unchanged_or_unmatched_names() {
    let spill = |session: &str, hash: &str, name: &str| {
        format!("/tmp/dsh-acp-snapshot-spill/session-{session}/{hash}-{name}")
    };
    let record = |text: &str| {
        lines(&[
            json!({"type":"session","id":"same","cwd":"/same"}),
            json!({"type":"tool/result","data":{"content":[{"type":"text","text":format!("stored at: {text} ")}]}}),
        ])
    };
    let fresh_bash = spill("aaaaaaaaaaaa", "bbbbbbbbbbbb", "bash.txt");
    let old_bash = spill("cccccccccccc", "dddddddddddd", "bash.txt");
    let shared = spill("eeeeeeeeeeee", "ffffffffffff", "grep.txt");
    let orphan = spill("111111111111", "222222222222", "orphan.txt");
    let logs = [record(&format!("{fresh_bash} and {shared}"))];
    let fixtures = [record(&format!("{old_bash} and {shared} and {orphan}"))];
    assert_eq!(
        refresh_fixture_replacements(&logs, &fixtures).unwrap(),
        vec![(fresh_bash, old_bash)]
    );
}

/// :1126
#[test]
fn leaves_complete_message_ids_out_of_the_literal_refresh_replacement_list() {
    let log = |session: &str, id: &str| {
        lines(&[
            json!({"type":"session","id":session,"cwd":"/same"}),
            user(id, "same"),
        ])
    };
    let replacements =
        refresh_fixture_replacements(&[log("fresh", FRESH)], &[log("old", OLD)]).unwrap();
    assert_eq!(replacements, vec![pair("fresh", "old")]);
}

fn join(rows: &[&str]) -> String {
    let mut text = rows.join("\n");
    text.push('\n');
    text
}

/// :1152
#[test]
fn preserves_unpacked_member_times_when_refresh_first_packs_a_chunk_run() {
    let fresh = join(&[
        r#"{"type":"session","id":"same","createdAt":200}"#,
        r#"{"type":"reasoning-chunks","seq0":2,"time0":200,"data":{"turn":1,"step":1,"index":0,"dt":[5,7],"texts":["new",""," split"]}}"#,
        r#"{"type":"assistant/message","seq":5,"time":220,"data":{}}"#,
    ]);
    let existing = join(&[
        r#"{"type":"session","id":"same","createdAt":100}"#,
        r#"{"type":"assistant/chunk","seq":2,"time":100,"data":{"turn":1,"step":1,"chunk":{"type":"reasoning-delta","index":0,"text":"old"}}}"#,
        r#"{"type":"assistant/chunk","seq":3,"time":101,"data":{"turn":1,"step":1,"chunk":{"type":"reasoning-delta","index":0,"text":"chunk"}}}"#,
        r#"{"type":"assistant/chunk","seq":4,"time":103,"data":{"turn":1,"step":1,"chunk":{"type":"reasoning-delta","index":0,"text":"shape"}}}"#,
        r#"{"type":"assistant/message","seq":5,"time":104,"data":{}}"#,
    ]);
    assert_eq!(
        stabilize(&fresh, &existing),
        join(&[
            r#"{"type":"session","id":"same","createdAt":100}"#,
            r#"{"type":"reasoning-chunks","seq0":2,"time0":100,"data":{"turn":1,"step":1,"index":0,"dt":[1,2],"texts":["new",""," split"]}}"#,
            r#"{"type":"assistant/message","seq":5,"time":104,"data":{}}"#,
        ])
    );
}

const PACKED_FRESH: &str = r#"{"type":"text-chunks","seq0":2,"time0":200,"data":{"turn":1,"step":1,"index":0,"dt":[5,7],"texts":["new",""," split"]}}"#;

/// :1176
#[test]
fn preserves_packed_member_times_without_flattening_fresh_chunk_boundaries() {
    let fresh = join(&[
        r#"{"type":"session","id":"same","createdAt":200}"#,
        PACKED_FRESH,
    ]);
    let existing = join(&[
        r#"{"type":"session","id":"same","createdAt":100}"#,
        r#"{"type":"text-chunks","seq0":2,"time0":100,"data":{"turn":1,"step":1,"index":0,"dt":[1,2],"texts":["old","chunk","shape"]}}"#,
    ]);
    assert_eq!(
        stabilize(&fresh, &existing),
        join(&[
            r#"{"type":"session","id":"same","createdAt":100}"#,
            r#"{"type":"text-chunks","seq0":2,"time0":100,"data":{"turn":1,"step":1,"index":0,"dt":[1,2],"texts":["new",""," split"]}}"#,
        ])
    );
}

/// :1195
#[test]
fn reads_projected_packed_fixtures_while_retaining_their_data_gaps() {
    let fresh = join(&[
        r#"{"type":"session","id":"same","createdAt":200}"#,
        PACKED_FRESH,
    ]);
    let existing = join(&[
        r#"{"type":"session","id":"same","createdAt":100}"#,
        r#"{"type":"text-chunks","data":{"turn":1,"step":1,"index":0,"dt":[1,2],"texts":["old","chunk","shape"]}}"#,
    ]);
    assert_eq!(
        stabilize(&fresh, &existing),
        join(&[
            r#"{"type":"session","id":"same","createdAt":100}"#,
            r#"{"type":"text-chunks","seq0":2,"time0":0,"data":{"turn":1,"step":1,"index":0,"dt":[1,2],"texts":["new",""," split"]}}"#,
        ])
    );
}

/// :1214 — four rows.
#[test]
fn keeps_fresh_packed_gaps_when_the_old_run_cannot_supply_them() {
    let min_safe = -9_007_199_254_740_991_i64;
    let max_safe = 9_007_199_254_740_991_i64;
    let rows: [(&str, Vec<Value>, i64); 4] = [
        ("the old run is absent", vec![], 200),
        ("the old run is shorter", vec![json!(100), json!(101)], 100),
        (
            "a later old time is invalid",
            vec![json!(100), json!("invalid"), json!(103)],
            100,
        ),
        (
            "an old gap is not exactly representable",
            vec![json!(min_safe), json!(max_safe - 1), json!(max_safe - 1)],
            min_safe,
        ),
    ];
    for (label, times, expected_time0) in rows {
        let fresh_row = json!({"type":"reasoning-chunks","seq0":2,"time0":200,
            "data":{"turn":1,"step":1,"index":0,"dt":[5,7],"texts":["new",""," split"]}});
        let existing_rows: Vec<Value> = times
            .iter()
            .enumerate()
            .map(|(i, t)| json!({"type":"assistant/chunk","seq":i + 2,"time":t,"data":{}}))
            .collect();
        let fresh = lines(&[
            json!({"type":"session","id":"same","createdAt":200}),
            fresh_row.clone(),
        ]);
        let mut existing = lines(&[json!({"type":"session","id":"same","createdAt":100})]);
        existing.push_str(&lines(&existing_rows));
        let output = parse(&stabilize(&fresh, &existing));
        let mut expected = fresh_row;
        expected["time0"] = json!(expected_time0);
        assert_eq!(output[1], expected, "{label}");
    }
}

/// :1240
#[test]
fn aligns_volatile_times_across_a_newly_inserted_log_event() {
    let fresh = join(&[
        r#"{"type":"session","id":"same","createdAt":200}"#,
        r#"{"type":"turn/start","seq":0,"time":21}"#,
        r#"{"type":"user/message","seq":1,"time":22}"#,
        r#"{"type":"session/title","seq":2,"time":999}"#,
        r#"{"type":"step/start","seq":3,"time":1000}"#,
        r#"{"type":"request/header","seq":4,"time":1001}"#,
    ]);
    let existing = join(&[
        r#"{"type":"session","id":"same","createdAt":100}"#,
        r#"{"type":"turn/start","seq":0,"time":11}"#,
        r#"{"type":"user/message","seq":1,"time":12}"#,
        r#"{"type":"step/start","seq":2,"time":13}"#,
        r#"{"type":"request/header","seq":3,"time":14}"#,
    ]);
    assert_eq!(
        stabilize(&fresh, &existing),
        join(&[
            r#"{"type":"session","id":"same","createdAt":100}"#,
            r#"{"type":"turn/start","seq":0,"time":11}"#,
            r#"{"type":"user/message","seq":1,"time":12}"#,
            r#"{"type":"session/title","seq":2,"time":12}"#,
            r#"{"type":"step/start","seq":3,"time":13}"#,
            r#"{"type":"request/header","seq":4,"time":14}"#,
        ])
    );
}

/// :1270
#[test]
fn retains_unchanged_message_ids_across_an_unrelated_inserted_event() {
    let ids = [
        "11111111-1111-4111-8111-111111111111",
        "22222222-2222-4222-8222-222222222222",
        "33333333-3333-4333-8333-333333333333",
        "44444444-4444-4444-8444-444444444444",
    ];
    let assistant = |id: &str| {
        json!({"type":"assistant/message","data":{"turn":1,"step":1,"message":{
            "role":"assistant","content":[{"type":"text","text":"same assistant"}],
            "source":{"kind":"model","provider":"fake","model":"fake"},"id":id}}})
    };
    let user_row = |id: &str| {
        json!({"type":"user/message","data":{"role":"user",
            "content":[{"type":"text","text":"same user"}],"source":{"kind":"user"},"id":id}})
    };
    let log = |rows: Vec<Value>| {
        let mut all = vec![json!({"type":"session","id":"same","createdAt":1,"cwd":"/same"})];
        all.extend(rows);
        lines(&all)
    };
    let fresh = log(vec![
        user_row(ids[0]),
        json!({"type":"session/inherited","data":{}}),
        assistant(ids[2]),
    ]);
    let existing = log(vec![user_row(ids[1]), assistant(ids[3])]);
    let replacements = refresh_fixture_replacements(
        std::slice::from_ref(&fresh),
        std::slice::from_ref(&existing),
    )
    .unwrap();
    let refreshed = stabilize_with(
        &fresh,
        &existing,
        &replacements,
        &fixture_context(&fresh).unwrap(),
    );
    let middle = parse(&refreshed);
    assert_eq!(middle[1]["data"]["id"], ids[0]);
    assert_eq!(middle[3]["data"]["message"]["id"], ids[2]);
    let output = parse(&stabilize_ids(&[refreshed], &[existing])[0]);
    assert_eq!(output[1]["data"]["id"], ids[1]);
    assert_eq!(output[3]["data"]["message"]["id"], ids[3]);
}

/// :1320
#[test]
fn leaves_an_aligned_complete_message_id_to_the_fixture_ready_structural_pass() {
    let log = |id: &str| {
        lines(&[
            json!({"type":"session","id":"same","createdAt":1,"cwd":"/same"}),
            user(id, "same"),
        ])
    };
    let (fresh, existing) = (log(FRESH), log(OLD));
    let refreshed = stabilize(&fresh, &existing);
    assert!(refreshed.contains(&format!("\"id\":\"{FRESH}\"")));
    assert!(stabilize_ids(&[refreshed], &[existing])[0].contains(&format!("\"id\":\"{OLD}\"")));
}

/// :1339
#[test]
fn keeps_volatile_fixture_fields_while_preserving_fresh_meaningful_payloads() {
    let fresh = join(&[
        r#"{"type":"session","id":"new-child","createdAt":200,"cwd":"/new","parentSession":"new-parent","seedLength":1}"#,
        r#"{"type":"hook/result","seq":1,"time":22,"data":{"decision":"block","durationMs":37}}"#,
        r#"{"type":"turn/end","seq":2,"time":33,"data":{"error":"fresh error"}}"#,
        r#"{"type":"tool/result","seq":3,"time":44,"data":{"text":"new-parent in /new"}}"#,
        r#"{"type":"hook/result","seq":4,"time":55,"data":{"decision":"allow","durationMs":5}}"#,
    ]);
    let existing = join(&[
        r#"{"type":"session","id":"old-child","createdAt":100,"cwd":"/old","parentSession":"old-parent","seedLength":5}"#,
        r#"{"type":"hook/result","seq":1,"time":11,"data":{"decision":"stale","durationMs":99}}"#,
        r#"{"type":"turn/end","seq":2,"time":12,"data":{"error":"stale"}}"#,
        r#"{"type":"assistant/message","seq":3,"time":12,"data":{"text":"different type"}}"#,
        r#"{"type":"hook/result","seq":4,"time":13,"data":{"decision":"stale"}}"#,
    ]);
    let replacements = [
        pair("new-parent", "old-parent"),
        pair("new-child", "old-child"),
        pair("/new", "/old"),
    ];
    assert_eq!(
        stabilize_with(
            &fresh,
            &existing,
            &replacements,
            &fixture_context(&fresh).unwrap()
        ),
        join(&[
            r#"{"type":"session","id":"old-child","createdAt":100,"cwd":"/old","parentSession":"old-parent","seedLength":1}"#,
            r#"{"type":"hook/result","seq":1,"time":11,"data":{"decision":"block","durationMs":99}}"#,
            r#"{"type":"turn/end","seq":2,"time":12,"data":{"error":"fresh error"}}"#,
            r#"{"type":"tool/result","seq":3,"time":44,"data":{"text":"old-parent in /old"}}"#,
            r#"{"type":"hook/result","seq":4,"time":13,"data":{"decision":"allow","durationMs":5}}"#,
        ])
    );
}

/// :1371
#[test]
fn keeps_job_identities_consistent_across_registration_tool_text_and_later_calls_during_refresh() {
    let log = |job_ids: &[&str], guidance: &str| {
        let mut rows = vec![
            json!({"type":"session","id":"same","createdAt":1,"cwd":"/same"}),
            json!({"type":"user/message","data":{"content":[{"type":"text","text":guidance}]}}),
        ];
        for job in job_ids {
            rows.push(json!({"type":"job/registered","data":{"jobId":job,"startedAt":1}}));
            rows.push(json!({"type":"tool/result","data":{"text":format!("started background job {job}")}}));
            rows.push(json!({"type":"tool/call","data":{"name":"job_output","arguments":json!({"job_id":job}).to_string()}}));
            rows.push(
                json!({"type":"job/settled","data":{"jobId":job,"startedAt":1,"finishedAt":2}}),
            );
        }
        lines(&rows)
    };
    let fresh = log(
        &["bash-1111111111111111", "bash-2222222222222222"],
        "new guidance",
    );
    let existing = log(&["{{id:1}}", "{{id:2}}"], "old guidance");
    assert_eq!(
        stabilize(&fresh, &existing),
        log(&["{{id:1}}", "{{id:2}}"], "new guidance")
    );
}

/// :1388
#[test]
fn preserves_normalized_volatile_fields_while_accepting_fresh_semantic_fields() {
    let fresh_approval = "11111111-1111-4111-8111-111111111111";
    let existing_approval = "22222222-2222-4222-8222-222222222222";
    let fresh_spill = "/tmp/dsh-acp-snap-012345678/session-111111111111/222222222222-bash.txt";
    let existing_spill = "/tmp/dsh-acp-snap-012345678/session-aaaaaaaaaaaa/bbbbbbbbbbbb-bash.txt";
    let tail = |spill: &str| {
        format!(
            "Full formatted result stored at: {spill}. Use read with offset/limit, or grep this path to search within it."
        )
    };
    let event_read = |time: &str, bytes: &str, spill: &str| {
        [
            "Session main — title".to_owned(),
            "Target event seq 4:".into(),
            "```json".into(),
            "{".into(),
            format!("  \"time\": {time},"),
            "  \"data\": {}".into(),
            "}".into(),
            "```".into(),
            String::new(),
            format!("(Omitted {bytes} bytes. {})", tail(spill)),
        ]
        .join("\n")
    };
    let fresh_event = event_read("1785000000000", "40000", fresh_spill);
    let existing_event = event_read("1784000000000", "30000", existing_spill);
    let fresh = lines(&[
        json!({"type":"session","id":"same","createdAt":200,"cwd":"/old"}),
        json!({"type":"approval/asked","seq":1,"time":22,"data":{"id":fresh_approval,"outcome":"fresh",
            "aliases":[fresh_approval,"fresh"],"resized":[fresh_approval,"new"],
            "shape":{"shared":fresh_approval,"added":true}}}),
        json!({"type":"tool/result","seq":2,"time":23,"data":{"spill":tail(fresh_spill),
            "path":"/private/old/result.txt","eventRead":fresh_event}}),
    ]);
    let existing = lines(&[
        json!({"type":"session","id":"same","createdAt":100,"cwd":"/old"}),
        json!({"type":"approval/asked","seq":1,"time":11,"data":{"id":existing_approval,"outcome":"stale",
            "aliases":[existing_approval,"stale"],"resized":[existing_approval],
            "shape":{"shared":existing_approval}}}),
        json!({"type":"tool/result","seq":2,"time":12,"data":{"spill":tail(existing_spill),
            "path":"/old/result.txt","eventRead":existing_event}}),
    ]);
    let output = parse(&stabilize(&fresh, &existing));
    assert_eq!(
        output,
        vec![
            json!({"type":"session","id":"same","createdAt":100,"cwd":"/old"}),
            json!({"type":"approval/asked","seq":1,"time":11,"data":{"id":existing_approval,"outcome":"fresh",
                "aliases":[existing_approval,"fresh"],"resized":[fresh_approval,"new"],
                "shape":{"shared":existing_approval,"added":true}}}),
            json!({"type":"tool/result","seq":2,"time":12,"data":{"spill":tail(existing_spill),
                "path":"/old/result.txt","eventRead":existing_event}}),
        ]
    );
}

/// :1491
#[test]
fn normalizes_fresh_cwd_aliases_before_reusing_existing_paths() {
    let fresh_cwd = r"C:\Users\RUNNER~1\AppData\Local\Temp\acp-snap-cwd-new";
    let fresh_alias = r"C:\Users\runneradmin\AppData\Local\Temp\acp-snap-cwd-new";
    let existing_cwd = r"C:\Users\RUNNER~1\AppData\Local\Temp\acp-snap-cwd-old";
    let fresh = lines(&[
        json!({"type":"session","id":"same","createdAt":200,"cwd":fresh_cwd}),
        json!({"type":"tool/result","data":{"path":format!("{fresh_alias}\\result.txt")}}),
    ]);
    let existing = lines(&[
        json!({"type":"session","id":"same","createdAt":100,"cwd":existing_cwd}),
        json!({"type":"tool/result","data":{"path":format!("{existing_cwd}\\result.txt")}}),
    ]);
    let mut context = fixture_context(&fresh).unwrap();
    context.aliases = vec![fresh_alias.to_owned()];
    assert_eq!(
        stabilize_with(
            &fresh,
            &existing,
            &[pair(fresh_cwd, existing_cwd)],
            &context
        ),
        lines(&[
            json!({"type":"session","id":"same","createdAt":100,"cwd":existing_cwd}),
            json!({"type":"tool/result","data":{"path":format!("{existing_cwd}\\result.txt")}}),
        ])
    );
}

fn approvals(rows: &[(&str, &str)], created: u32) -> String {
    let mut all = vec![json!({"type":"session","id":"same","createdAt":created,"cwd":"/old"})];
    all.extend(
        rows.iter()
            .map(|(kind, id)| json!({"type":kind,"data":{"id":id}})),
    );
    lines(&all)
}
fn ids_of(output: &str) -> Vec<String> {
    parse(output)[1..]
        .iter()
        .map(|r| r["data"]["id"].as_str().unwrap().to_owned())
        .collect()
}

/// :1519
#[test]
fn preserves_one_correlated_volatile_id_through_a_consistent_log_wide_mapping() {
    let fresh = lines(&[
        json!({"type":"session","id":"same","createdAt":200,"cwd":"/old"}),
        json!({"type":"approval/asked","data":{"id":FRESH}}),
        json!({"type":"approval/decided","data":{"id":FRESH,"outcome":"allowed-once"}}),
    ]);
    let existing = lines(&[
        json!({"type":"session","id":"same","createdAt":100,"cwd":"/old"}),
        json!({"type":"approval/asked","data":{"id":OLD}}),
        json!({"type":"approval/decided","data":{"id":OLD,"outcome":"rejected"}}),
    ]);
    assert_eq!(
        stabilize(&fresh, &existing),
        lines(&[
            json!({"type":"session","id":"same","createdAt":100,"cwd":"/old"}),
            json!({"type":"approval/asked","data":{"id":OLD}}),
            json!({"type":"approval/decided","data":{"id":OLD,"outcome":"allowed-once"}}),
        ])
    );
}

/// :1543
#[test]
fn keeps_fresh_correlated_ids_when_record_alignment_is_structurally_ambiguous() {
    let second = "22222222-2222-4222-8222-222222222222";
    let existing_id = "33333333-3333-4333-8333-333333333333";
    let fresh = approvals(
        &[
            ("approval/asked", FRESH),
            ("approval/asked", second),
            ("approval/decided", FRESH),
            ("approval/decided", second),
        ],
        200,
    );
    let existing = approvals(
        &[
            ("approval/asked", existing_id),
            ("approval/decided", existing_id),
        ],
        100,
    );
    assert_eq!(
        ids_of(&stabilize(&fresh, &existing)),
        vec![FRESH, second, FRESH, second]
    );
}

/// :1567
#[test]
fn keeps_fresh_ids_when_existing_records_remain_unmatched() {
    let fresh = approvals(&[("approval/asked", FRESH)], 200);
    let existing = approvals(&[("approval/asked", OLD), ("approval/decided", OLD)], 100);
    let output = parse(&stabilize(&fresh, &existing));
    assert_eq!(
        output[1],
        json!({"type":"approval/asked","data":{"id":FRESH}})
    );
}

/// :1587 — two rows.
#[test]
fn keeps_fresh_ids_when_the_mapping_would_not_be_a_bijection() {
    let id = |k: &str| match k {
        "a" => "11111111-1111-4111-8111-111111111111",
        "b" => "22222222-2222-4222-8222-222222222222",
        "x" => "33333333-3333-4333-8333-333333333333",
        _ => "44444444-4444-4444-8444-444444444444",
    };
    let types = [
        "approval/asked",
        "approval/asked",
        "approval/decided",
        "approval/decided",
    ];
    let log = |names: &[&str]| {
        let rows: Vec<(&str, &str)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (types[i], id(n)))
            .collect();
        approvals(&rows, 100)
    };
    let rows: [(&str, &[&str], &[&str]); 2] = [
        (
            "one fresh id would map to two existing ids",
            &["a", "b", "b", "a"],
            &["x", "y", "x", "y"],
        ),
        (
            "two fresh ids would map to one existing id",
            &["a", "b"],
            &["x", "x"],
        ),
    ];
    for (name, fresh, existing) in rows {
        let output = ids_of(&stabilize(&log(fresh), &log(existing)));
        let expected: Vec<String> = fresh.iter().map(|n| id(n).to_owned()).collect();
        assert_eq!(output, expected, "{name}");
    }
}

const CHILD: &str = "{\"type\":\"session\",\"id\":\"child\",\"createdAt\":100}\n";
fn parent(time: u32) -> String {
    lines(&[
        json!({"type":"session","id":"parent","createdAt":1}),
        json!({"type":"subagent/catalog","seq":0,"time":2,
            "data":{"version":0,"childId":"child","childCreatedAt":time,"mode":"one-shot"}}),
    ])
}

/// :1626 — the replay path validates the raw clocks.
#[test]
fn rejects_mismatched_raw_clocks_before_normalization_can_erase_them() {
    let error =
        reconcile_catalog_creation_times(&[parent(200), CHILD.to_owned()], ClockPolicy::Validate)
            .unwrap_err();
    assert!(
        error.contains("creation time 200 disagrees with child header 100"),
        "{error}"
    );
    assert_eq!(
        reconcile_catalog_creation_times(&[parent(100), CHILD.to_owned()], ClockPolicy::Validate)
            .unwrap(),
        vec![parent(100), CHILD.to_owned()]
    );
}

/// :1631
#[test]
fn keeps_retained_child_clocks_in_newly_generated_catalog_fixtures() {
    let output = reconcile_catalog_creation_times(
        &[parent(200), CHILD.to_owned()],
        ClockPolicy::PreserveHeaders,
    )
    .unwrap();
    assert_eq!(output, vec![parent(100), CHILD.to_owned()]);
    assert_eq!(
        reconcile_catalog_creation_times(&output, ClockPolicy::Validate).unwrap(),
        output
    );
    assert_eq!(
        reconcile_catalog_creation_times(&output, ClockPolicy::PreserveHeaders).unwrap(),
        output
    );
}

/// :1638
#[test]
fn preserves_partial_corpora_and_unrelated_rows_verbatim() {
    let partial = parent(200) + "{\"type\":\"feedback/record\",\"data\":null}\n";
    assert_eq!(
        reconcile_catalog_creation_times(std::slice::from_ref(&partial), ClockPolicy::Validate)
            .unwrap(),
        vec![partial]
    );
}
