//! `normalize.spec.ts` cases of the pure normalizers: `normalizeStdout` is `Normalizer::frames`,
//! `normalizeSessionLog` is `Normalizer::legacy_log`, `normalizeSessionSnapshot` is `Normalizer::logs`.
use hoh_conformance::{
    jsonl,
    normalize::{Context, Normalizer},
};
use serde_json::{Value, json};

fn n() -> Normalizer {
    Normalizer::new().unwrap()
}
fn ctx() -> Context {
    Context {
        cwd: "/tmp/acp-snap-cwd-abc123".into(),
        aliases: vec![],
        session_ids: vec!["11111111-2222-3333-4444-555555555555".into()],
    }
}
fn context(cwd: &str, aliases: &[&str]) -> Context {
    Context {
        cwd: cwd.into(),
        aliases: aliases.iter().map(|s| (*s).into()).collect(),
        session_ids: vec![],
    }
}
/// `normalizeStdout` of one frame.
fn stdout(frame: Value, ctx: &Context) -> Value {
    n().frames(&jsonl(&[frame]), ctx).unwrap().remove(0)
}
fn text(rows: &[Value]) -> String {
    rows.iter().map(|r| format!("{r}\n")).collect()
}
/// `normalizeSessionLog`.
fn log(rows: &[Value], ctx: &Context) -> String {
    text(&n().legacy_log(&jsonl(rows), ctx).unwrap())
}
fn header(over: Value) -> Value {
    let mut h = json!({"type":"session","version":0,"id":"s","createdAt":123});
    for (k, v) in over.as_object().unwrap() {
        h[k] = v.clone();
    }
    h
}
fn event(over: Value) -> Value {
    let mut e = json!({"type":"turn/start","seq":1,"time":999,"data":{"turn":1}});
    for (k, v) in over.as_object().unwrap() {
        e[k] = v.clone();
    }
    e
}
fn tool_result(data: Value) -> Value {
    json!({"type":"tool/result","seq":2,"time":5,"data":data})
}

mod normalize_stdout {
    use super::*;

    /// :96
    #[test]
    fn scrubs_cwd_at_file_uri_and_chained_punctuation_boundaries() {
        let c = ctx();
        let frame = stdout(
            json!({"jsonrpc":"2.0","method":"session/update","params":{
                "uri":format!("file://{}/proof.txt", c.cwd),
                "punctuated":format!("{}.,", c.cwd),
                "dottedSegment":format!("{}.backup", c.cwd),
                "dashedSegment":format!("{}-backup", c.cwd)}}),
            &c,
        );
        assert_eq!(
            frame["params"],
            json!({"uri":"file://{{cwd}}/proof.txt","punctuated":"{{cwd}}.,",
                "dottedSegment":format!("{}.backup", c.cwd),"dashedSegment":format!("{}-backup", c.cwd)})
        );
    }

    /// :118
    #[test]
    fn scrubs_every_filesystem_spelling_of_the_cwd_longest_first() {
        let long = r"C:\Users\runneradmin\AppData\Local\Temp\acp-snapshot";
        let c = context(
            r"C:\Users\RUNNER~1\AppData\Local\Temp\acp-snapshot",
            &[long, r"C:\Users\runneradmin\AppData\Local\Temp\acp"],
        );
        let frame = stdout(
            json!({"cwd":long,"path":format!("{long}\\nested\\proof.txt")}),
            &c,
        );
        assert_eq!(
            frame,
            json!({"cwd":"{{cwd}}","path":"{{cwd}}/nested/proof.txt"})
        );
    }

    /// :160
    #[test]
    fn canonicalizes_generated_relative_path_fields_and_text_markers_without_rewriting_other_text()
    {
        let frame = stdout(
            json!({"path":r"nested\AGENTS.md",
                "content":"<path>.\\nested\\task.txt</path>\nAdditional instructions from: nested\\AGENTS.md",
                "regex":r"\d+\w+"}),
            &context("/unused", &[]),
        );
        assert_eq!(
            frame,
            json!({"path":"nested/AGENTS.md",
                "content":"<path>./nested/task.txt</path>\nAdditional instructions from: nested/AGENTS.md",
                "regex":r"\d+\w+"})
        );
    }

    /// :186
    #[test]
    fn scrubs_a_stray_uuid_not_in_the_known_list() {
        let frame = stdout(
            json!({"jsonrpc":"2.0","method":"x","params":{"id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}}),
            &ctx(),
        );
        assert!(frame.to_string().contains("{{sessionId}}"), "{frame}");
    }

    /// :191
    #[test]
    fn leaves_notification_frames_without_an_id_untouched_in_id_space() {
        let frame = stdout(
            json!({"jsonrpc":"2.0","method":"session/update","params":{}}),
            &ctx(),
        );
        assert!(!frame.to_string().contains("\"id\""));
        assert!(frame.get("id").is_none());
    }

    /// :197
    #[test]
    fn stabilizes_only_the_top_level_event_timestamp_and_spill_byte_count_in_event_read_text() {
        let body = "Session prior — title\nTarget event seq 4:\n```json\n{\n  \"seq\": 4,\n  \"time\": 1784876275593,\n  \"data\": {\n    \"time\": 31337,\n    \"note\": \"model-visible\"\n  }\n}\n```\n\nAfter:\n  \"time\": 424242,\n  neighbor semantic text\n\n(Omitted 39387 bytes. Full formatted result stored at: /tmp/result.txt.)";
        let frame = stdout(
            json!({"jsonrpc":"2.0","method":"session/update","params":{"update":{
                "sessionUpdate":"tool_call_update",
                "content":[{"type":"content","content":{"type":"text","text":body}}]}}}),
            &ctx(),
        );
        let out = frame.to_string();
        assert!(out.contains("\\\"time\\\": {{eventTime}}"), "{out}");
        assert!(out.contains("\\\"time\\\": 31337"));
        assert!(out.contains("\\\"time\\\": 424242"));
        assert!(out.contains("Omitted {{eventOmittedBytes}} bytes"));
        assert!(!out.contains("1784876275593"));
        assert!(!out.contains("39387"));
    }
}

mod normalize_session_log {
    use super::*;

    /// :259 (both rows of the table)
    #[test]
    fn keeps_full_and_short_uuids_distinct_in_single_log_and_single_snapshot_paths() {
        for ids in [
            [
                "bash-aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                "bash-bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            ],
            ["bash-aaaaaaaaaaaaaaaa", "bash-bbbbbbbbbbbbbbbb"],
        ] {
            let c = ctx();
            let mut rows = vec![
                json!({"type":"session","version":0,"id":c.session_ids[0],"cwd":c.cwd,"createdAt":100}),
                json!({"type":"user/message","data":{"text":"Keep {{id:1}} literal."}}),
            ];
            for id in ids {
                rows.push(json!({"type":"job/registered","data":{"jobId":id,"startedAt":100}}));
                rows.push(json!({"type":"job/settled","data":{"jobId":id,"startedAt":100,"finishedAt":200}}));
            }
            rows.push(json!({"type":"tool/call","data":{"arguments":json!({"job_id":ids[0]}).to_string()}}));
            rows.push(json!({"type":"tool/result","data":{"message":{"content":[{"type":"text","text":format!("read {}", ids[0])}]}}}));
            let output = n().legacy_log(&jsonl(&rows), &c).unwrap();
            let jobs: Vec<&Value> = output
                .iter()
                .filter(|r| r["type"].as_str().unwrap().starts_with("job/"))
                .map(|r| &r["data"]["jobId"])
                .collect();
            assert_eq!(jobs, ["{{id:2}}", "{{id:2}}", "{{id:3}}", "{{id:3}}"]);
            let call = output.iter().find(|r| r["type"] == "tool/call").unwrap();
            let arguments: Value =
                serde_json::from_str(call["data"]["arguments"].as_str().unwrap()).unwrap();
            assert_eq!(arguments, json!({"job_id":"{{id:2}}"}));
            let all = text(&output);
            assert!(all.contains("read {{id:2}}") && all.contains("Keep {{id:1}} literal."));
            assert_eq!(text(&n().legacy_log(&all, &c).unwrap()), all);
        }
    }

    /// :286
    #[test]
    fn normalizes_only_message_feedback_item_clocks() {
        let item = json!({"messageId":"answer","version":"version","createdAt":123,"updatedAt":456,"note":"keep 123"});
        let rows: Vec<Value> = ["feedback/message-put", "tool/result"]
            .iter()
            .map(|t| json!({"type":t,"data":{"item":item}}))
            .collect();
        let out = log(&rows, &ctx());
        assert!(
            out.contains("\"createdAt\":0,\"updatedAt\":0,\"note\":\"keep 123\""),
            "{out}"
        );
        assert!(
            out.contains("\"createdAt\":123,\"updatedAt\":456,\"note\":\"keep 123\""),
            "{out}"
        );
    }

    /// :297 (both normalizers)
    #[test]
    fn keeps_unexpected_request_header_fields_observable_in_comparisons() {
        let request = |system: bool| {
            let mut data = json!({"header":{"config":{"model":"mock"}}});
            if system {
                data["header"]["system"] = json!("unexpected prompt");
            }
            event(json!({"type":"request/header","data":data}))
        };
        let c = ctx();
        let actual = log(&[header(json!({})), request(true)], &c);
        assert!(actual.contains("\"system\":\"unexpected prompt\""));
        assert_ne!(actual, log(&[header(json!({})), request(false)], &c));
        let snapshot = |system| {
            n().logs(&[jsonl(&[header(json!({})), request(system)])], &c)
                .unwrap()
        };
        assert!(format!("{:?}", snapshot(true)).contains("unexpected prompt"));
        assert_ne!(snapshot(true), snapshot(false));
    }

    /// :315 — `Normalizer::legacy_log` keeps `seq` and zeroes `time`.
    #[test]
    fn preserves_event_sequence_and_zeroes_event_time() {
        let out = log(
            &[header(json!({})), event(json!({"seq":7,"time":999}))],
            &ctx(),
        );
        assert!(out.contains("\"time\":0") && out.contains("\"seq\":7"));
        assert!(!out.contains("999"));
    }

    /// :322
    #[test]
    fn normalizes_a_projected_event_without_adding_a_persistence_envelope() {
        let projected = json!({"type":"turn/start","data":{"turn":1}});
        let rows = n()
            .legacy_log(&jsonl(&[header(json!({})), projected.clone()]), &ctx())
            .unwrap();
        assert_eq!(rows[1], projected);
    }

    /// :331
    #[test]
    fn scrubs_cwd_and_session_id_deep_inside_event_data() {
        let c = ctx();
        let ev = tool_result(
            json!({"content":[{"type":"text","text":format!("wrote {}/proof.txt", c.cwd)}]}),
        );
        let out = log(&[header(json!({"cwd":c.cwd})), ev], &c);
        assert!(out.contains("{{cwd}}") && !out.contains(&c.cwd));
    }

    /// :341
    #[test]
    fn scrubs_cwd_at_file_uri_and_chained_punctuation_boundaries_in_event_data() {
        let c = ctx();
        let ev = tool_result(
            json!({"uri":format!("file://{}/proof.txt", c.cwd),"punctuated":format!("{}.,", c.cwd)}),
        );
        let out = log(&[header(json!({"cwd":c.cwd})), ev], &c);
        assert!(out.contains("file://{{cwd}}/proof.txt") && out.contains("{{cwd}}.,"));
        assert!(!out.contains(&format!("file://{}", c.cwd)));
    }

    fn spill_text(path: &str) -> Value {
        tool_result(json!({"content":[{"type":"text","text":format!(
            "Full formatted result stored at: {path}/session-c22bc3f1d2af/8a7b6c5d4e3f-bash.txt. Use read with offset/limit, or grep this path to search within it.")}]}))
    }

    /// :373
    #[test]
    fn scrubs_macos_private_aliases_for_local_spill_paths() {
        let c = ctx();
        let ev = spill_text(&format!("/private{}/.spill", c.cwd));
        let out = log(&[header(json!({"cwd":c.cwd})), ev], &c);
        assert!(out.contains("{{spillLocator:bash.txt}}"), "{out}");
        assert!(!out.contains("/private{{spillLocator"));
    }

    /// :403
    #[test]
    fn scrubs_fixed_snapshot_spill_paths() {
        let c = ctx();
        let ev = spill_text("/tmp/dsh-acp-snapshot-spill");
        let out = log(&[header(json!({"cwd":c.cwd})), ev], &c);
        assert!(out.contains("{{spillLocator:bash.txt}}"), "{out}");
        assert!(!out.contains("/tmp/dsh-acp-snapshot-spill"));
    }

    /// :460
    #[test]
    fn scrubs_the_session_id_in_the_header() {
        let c = ctx();
        let out = log(&[header(json!({"id":c.session_ids[0]}))], &c);
        assert!(out.contains("{{sessionId}}"), "{out}");
    }

    /// :490
    #[test]
    fn normalizes_timing_inside_an_embedded_assistant_stream_and_ignores_opaque_members() {
        let stream = json!([
            null,
            "opaque",
            {"type":"chunk","time":8,"chunk":{"type":"finish","reason":{"kind":"stop"}}},
            {"type":"usage","time":7,"time0":6,"dt":[5,4],"usage":{"inputTokens":1,"outputTokens":2}},
            {"type":"chunk","time":8,"chunk":{"type":"finish","reason":{"kind":"stop"}}}
        ]);
        let ev = json!({"type":"assistant/attempt","seq":2,"time":9,"data":{"turn":1,"step":1,"stream":stream}});
        let rows = n()
            .legacy_log(&jsonl(&[header(json!({})), ev]), &ctx())
            .unwrap();
        let normalized = &rows[1];
        assert_eq!(normalized["time"], 0);
        let s = &normalized["data"]["stream"];
        assert_eq!(s[0], Value::Null);
        assert_eq!(s[1], "opaque");
        assert_eq!(s[2]["time"], 0);
        assert_eq!(
            s[3],
            json!({"type":"usage","time":0,"time0":0,"dt":[0,0],"usage":{"inputTokens":1,"outputTokens":2}})
        );
        assert_eq!(
            s[4],
            json!({"type":"chunk","time":0,"chunk":{"type":"finish","reason":{"kind":"stop"}}})
        );
    }

    /// :527
    #[test]
    fn normalizes_a_headerless_packed_like_stream_record_without_decoding_it() {
        let row = json!({"type":"text-chunks","seq0":1,"time0":999,"data":"not-an-object"});
        let out = log(&[row], &ctx());
        assert!(
            out.contains("\"seq0\":1") && out.contains("\"time0\":0"),
            "{out}"
        );
    }

    /// :540
    #[test]
    fn normalizes_goal_lifecycle_clocks_without_scrubbing_unrelated_payload_timestamps() {
        let goal = json!({"type":"goal/change","seq":2,"time":5,"data":{"operation":"create","createdAt":123,"updatedAt":124}});
        let tool = json!({"type":"tool/result","seq":3,"time":6,"data":{"createdAt":125}});
        let without = json!({"type":"goal/change","seq":4,"time":7,"data":{"operation":"resume"}});
        let out = log(&[header(json!({})), goal, tool, without], &ctx());
        assert!(
            out.contains("\"operation\":\"create\",\"createdAt\":0,\"updatedAt\":0"),
            "{out}"
        );
        assert!(out.contains("\"createdAt\":125"));
        assert!(out.contains("\"operation\":\"resume\""));
    }

    /// :555
    #[test]
    fn normalizes_job_lifecycle_clocks_while_preserving_user_timestamps_and_uncertainty() {
        let rows = [
            header(json!({})),
            json!({"type":"job/registered","data":{"startedAt":100,"jobId":"bash-1"}}),
            json!({"type":"job/settled","data":{"startedAt":100,"finishedAt":200,"producerReported":false}}),
            json!({"type":"job/registered","data":{}}),
            json!({"type":"job/settled","data":null}),
            json!({"type":"job/settled","data":"invalid fixture remains invalid"}),
            json!({"type":"tool/result","data":{"startedAt":300,"finishedAt":400}}),
        ];
        let out = n().legacy_log(&jsonl(&rows), &ctx()).unwrap();
        assert_eq!(out[1]["data"], json!({"startedAt":0,"jobId":"bash-1"}));
        assert_eq!(
            out[2]["data"],
            json!({"startedAt":0,"finishedAt":0,"producerReported":false})
        );
        assert_eq!(out[3]["data"], json!({}));
        assert_eq!(out[4]["data"], Value::Null);
        assert_eq!(out[5]["data"], "invalid fixture remains invalid");
        assert_eq!(out[6]["data"], json!({"startedAt":300,"finishedAt":400}));
    }
}

mod normalize_session_snapshot {
    use super::*;
    use hoh_conformance::{
        compare,
        normalize::{normalize_session_format_metadata, scrub_session_snapshot},
    };

    fn snapshot(rows: &[Value]) -> Vec<Value> {
        n().logs(&[jsonl(rows)], &ctx()).unwrap().remove(0)
    }

    /// :609 (`logs` omits the header generation, as `normalizeSessionSnapshots`).
    #[test]
    fn normalizes_scrubs_and_projects_each_parsed_body_record() {
        let c = ctx();
        let out = snapshot(&[
            json!({"type":"session","version":0,"createdAt":123,"cwd":c.cwd}),
            json!({"type":"system/message","seq":6,"time":998,"data":{"turn":1,"step":1,
                "message":{"role":"system","content":[{"type":"text","text":format!("work in {}", c.cwd)}]}}}),
            json!({"type":"request/header","seq":7,"time":999,"data":{"header":{"tools":[{"name":"tool"}]}}}),
        ]);
        assert_eq!(
            out,
            [
                json!({"type":"session","createdAt":0,"cwd":"{{cwd}}"}),
                json!({"type":"system/message","data":{"turn":1,"step":1,
                    "message":{"role":"system","content":[{"type":"text","text":"{{system}}"}]}}}),
                json!({"type":"request/header","data":{"header":{"tools":"{{tools}}"}}}),
            ]
        );
    }

    fn packed(texts: [&str; 3], dt: [u64; 2]) -> Value {
        json!({"type":"text-chunks","data":{"turn":1,"step":1,"index":0,"dt":dt,"texts":texts}})
    }

    /// :646
    #[test]
    fn retains_historical_packed_row_boundaries_while_normalizing_their_timing() {
        let out = snapshot(&[
            json!({"type":"session","version":0}),
            packed(["a", "b", "c"], [4, 5]),
            packed(["d", "e", "f"], [6, 7]),
        ]);
        assert_eq!(
            out,
            [
                json!({"type":"session"}),
                packed(["a", "b", "c"], [0, 0]),
                packed(["d", "e", "f"], [0, 0]),
            ]
        );
    }

    /// :672
    #[test]
    fn preserves_adjacent_catalog_facts_in_parent_event_order() {
        let catalog = |id: &str, at: u64| json!({"type":"subagent/catalog","data":{"version":0,"childId":id,"childCreatedAt":at,"mode":"one-shot"}});
        let out = snapshot(&[
            json!({"type":"session","version":0}),
            json!({"type":"tool/call","data":{"callId":"parallel"}}),
            catalog("{{session:3}}", 123),
            catalog("{{session:2}}", 124),
            json!({"type":"tool/result","data":{"callId":"parallel"}}),
        ]);
        let text = text(&out);
        assert!(text.find("{{session:3}}").unwrap() < text.find("{{session:2}}").unwrap());
        assert!(text.contains("\"childCreatedAt\":0"));
    }

    /// :691
    #[test]
    fn preserves_malformed_catalog_payloads() {
        let out = text(&snapshot(&[
            json!({"type":"session","version":0}),
            json!({"type":"subagent/catalog","data":{"version":0,"childId":"{{session:2}}","childCreatedAt":1,"mode":"one-shot"}}),
            json!({"type":"subagent/catalog","data":{"version":0,"childId":3,"childCreatedAt":3,"mode":"one-shot"}}),
        ]));
        assert!(
            out.contains("{{session:2}}") && out.contains("\"childId\":3"),
            "{out}"
        );
    }

    /// :708 (both source lists)
    #[test]
    fn preserves_source_references_and_catalog_order() {
        for sources in [json!([0, 1]), json!([0, 2])] {
            let catalog = |id: &str, at: u64| json!({"type":"subagent/catalog","data":{"childId":id,"childCreatedAt":at,"version":0,"mode":"one-shot"}});
            let rows = [
                json!({"type":"session","version":2}),
                json!({"type":"tool/call","data":{"callId":"parallel"}}),
                catalog("child-z", 1),
                catalog("child-a", 2),
                json!({"type":"tool/result","data":{"callId":"parallel"},"sourceEventSeqs":sources,"surfaceOp":"append"}),
            ];
            let normalized = snapshot(&rows);
            assert_eq!(
                normalized,
                [
                    json!({"type":"session"}),
                    rows[1].clone(),
                    catalog("child-z", 0),
                    catalog("child-a", 0),
                    rows[4].clone(),
                ]
            );
            assert_eq!(
                n().logs(&[text(&normalized)], &ctx()).unwrap()[0],
                normalized
            );
        }
    }

    /// :730 — migration is `compare::normalized_session` (strict catalog), then the normalizer.
    #[test]
    fn migrates_and_re_packs_multi_session_fixtures_after_relationship_preserving_id_redaction() {
        let chunk = |dt: [u64; 2], texts: [&str; 3]| json!({"type":"reasoning-chunks","data":{"turn":1,"step":1,"index":0,"dt":dt,"texts":texts}});
        let raw = jsonl(&[
            json!({"type":"session","version":0,"id":"{{session:1}}","createdAt":0,"delegationDepth":0}),
            json!({"type":"turn/start","data":{"turn":1}}),
            json!({"type":"step/start","data":{"turn":1,"step":1}}),
            chunk([1, 2], ["a", "b", "c"]),
            chunk([3, 4], ["d", "e", "f"]),
        ]);
        let normalized = compare::normalized_session(&raw).unwrap();
        let rows = normalized[0].as_array().unwrap();
        assert_eq!(
            rows[0],
            json!({"type":"session","id":"{{session:1}}","createdAt":0,"isSeeded":false,"delegationDepth":0})
        );
        assert_eq!(rows[1], json!({"type":"turn/start","data":{"turn":1}}));
        assert_eq!(
            rows[2],
            json!({"type":"step/start","data":{"turn":1,"step":1}})
        );
        assert_eq!(
            rows[3],
            json!({"type":"system/message","data":{"turn":1,"step":1,"message":{
                "id":"v2-to-v3-system-590b72aa4994fd6d3c6e61bb4bf5bf2f80bae0bc7564d388378ba4f51b816fd6",
                "role":"system","source":{"kind":"system-prompt"},"content":[]}},"surfaceOp":"append"})
        );
        assert_eq!(
            rows[4],
            json!({"type":"assistant/attempt","data":{"turn":1,"step":1,"stream":[{
                "type":"reasoning-chunks","time0":0,"index":0,"dt":[0,0,0,0,0],
                "texts":["a","b","c","d","e","f"]}]}})
        );
        assert_eq!(rows.len(), 5);
    }

    /// :782
    #[test]
    fn normalizes_an_already_projected_snapshot_without_a_released_format_field() {
        let raw = jsonl(&[
            json!({"type":"session","id":"11111111-2222-3333-4444-555555555555","createdAt":9}),
        ]);
        let out = n().logs(&[raw], &context("/unused", &[])).unwrap();
        assert_eq!(
            out,
            [[json!({"type":"session","id":"{{session:1}}","createdAt":0})]]
        );
    }

    fn delivery_log(version: u64, delivery: u64, id: &str) -> String {
        jsonl(&[
            json!({"type":"session","version":version,"id":id,"createdAt":1,"isSeeded":false,"delegationDepth":0}),
            json!({"type":"turn/start","seq":0,"time":1,"data":{"turn":1}}),
            json!({"type":"session-log-deepseek/delivery-accepted","seq":1,"time":2,
                "data":{"sessionId":id,"throughSeq":0,"sessionFormatVersion":delivery}}),
        ])
    }

    /// :816
    #[test]
    fn compares_each_strictly_restored_input_using_its_own_native_delivery_generation() {
        let c = ctx();
        let logs = n()
            .logs(
                &[
                    delivery_log(3, 3, "delivery"),
                    delivery_log(4, 4, "delivery"),
                ],
                &c,
            )
            .unwrap();
        assert_eq!(logs[0], logs[1]);
        assert!(
            text(&logs[1]).contains("\"sessionFormatVersion\":\"{{sourceSessionFormatVersion}}\"")
        );
        let current = n()
            .logs_with(&[delivery_log(4, 4, "delivery")], &c, false)
            .unwrap();
        assert!(text(&current[0]).contains("\"sessionFormatVersion\":4"));
    }

    /// :826
    #[test]
    fn keeps_source_versus_migrated_artifact_delivery_comparison_generation_exact_by_default() {
        let source = delivery_log(3, 3, "delivery");
        let target = compare::prepare_fixture_for_comparison(&source).unwrap();
        let logs = n().logs_with(&[source, target], &ctx(), false).unwrap();
        assert_eq!(logs[0], logs[1]);
        assert!(text(&logs[1]).contains("\"sessionFormatVersion\":3"));
    }

    /// :847
    #[test]
    fn retains_future_delivery_generations_without_renaming_their_event_identity() {
        let source = delivery_log(3, 99, "recorded-session");
        let target = compare::prepare_fixture_for_comparison(&source).unwrap();
        let marker: Value =
            serde_json::from_str(target.trim_end().lines().last().unwrap()).unwrap();
        assert_eq!(
            marker,
            json!({"type":"session-log-deepseek/delivery-accepted","seq":1,"time":2,
                "data":{"sessionId":"recorded-session","throughSeq":0,"sessionFormatVersion":99}})
        );
        let logs = n().logs_with(&[source, target], &ctx(), true).unwrap();
        assert_eq!(logs[0], logs[1]);
        let all = text(&logs[0]);
        assert!(
            all.contains("\"sessionFormatVersion\":99")
                && !all.contains("{{sourceSessionFormatVersion}}")
        );
    }

    /// :861
    #[test]
    fn keeps_captured_generations_and_delivery_lookalikes_numeric_in_versioned_inputs() {
        let log = delivery_log(4, 4, "delivery")
            + &jsonl(&[
                json!({"type":"custom/event","seq":2,"time":3,"ignorable":true,
                "data":{"capturedFormatVersion":4,"sessionFormatVersion":4}}),
            ]);
        let out = n().logs(&[log], &ctx()).unwrap();
        assert!(
            text(&out[0])
                .contains(&json!({"capturedFormatVersion":4,"sessionFormatVersion":4}).to_string())
        );
    }

    /// :872
    #[test]
    fn preserves_delivery_and_captured_source_generations_without_a_validated_source_generation() {
        let event = |version: u64| {
            json!({"type":"session-log-deepseek/delivery-accepted",
                "data":{"sessionId":"s","throughSeq":4,"sessionFormatVersion":version}})
            .to_string()
        };
        assert_eq!(
            normalize_session_format_metadata(&event(0), None).unwrap(),
            event(0)
        );
        assert_ne!(
            normalize_session_format_metadata(&event(3), Some(3)).unwrap(),
            normalize_session_format_metadata(&event(0), Some(3)).unwrap()
        );
    }

    /// :881
    #[test]
    fn preserves_opaque_generation_qualifiers_and_their_lookalikes() {
        let rows = [
            json!({"type":"session","id":"11111111-2222-3333-4444-555555555555","createdAt":0}),
            json!({"type":"session-log-deepseek/delivery-accepted","data":{"sessionFormatVersion":1,"throughSeq":21,"otherVersion":8}}),
            json!({"type":"user/message","data":{"role":"user","content":[],"source":{
                "kind":"session-reference","form":"recall","version":1,
                "references":[null,"opaque",[{"capturedFormatVersion":6}],{"capturedFormatVersion":1,"otherVersion":9}]}}}),
            json!({"type":"assistant/message","data":{"message":{"role":"assistant","content":[],"source":[{"capturedFormatVersion":7}]}}}),
            json!({"type":"custom/event","data":{"capturedFormatVersion":5,"sessionFormatVersion":4},"ignorable":true}),
        ];
        let out = n()
            .logs(&[jsonl(&rows)], &context("/unused", &[]))
            .unwrap()
            .remove(0);
        assert_eq!(
            out[1]["data"],
            json!({"sessionFormatVersion":1,"throughSeq":21,"otherVersion":8})
        );
        assert_eq!(
            out[2]["data"]["source"]["references"],
            rows[2]["data"]["source"]["references"]
        );
        assert_eq!(out[3]["data"], rows[3]["data"]);
        assert_eq!(
            out[4]["data"],
            json!({"capturedFormatVersion":5,"sessionFormatVersion":4})
        );
    }

    /// :955
    #[test]
    fn preserves_an_unexpected_session_reference_payload_instead_of_omitting_its_fields() {
        let raw = json!({"type":"user/message","data":{"source":{
            "kind":"session-reference","form":"recall","version":1,"references":{}}}})
        .to_string();
        assert_eq!(normalize_session_format_metadata(&raw, None).unwrap(), raw);
    }

    /// :963
    #[test]
    fn keeps_session_reference_lookalikes_outside_message_source_positions_unchanged() {
        let lookalike = format!(
            "{}\n{}\n",
            json!({"type":"session","version":1,"id":"s","createdAt":0,"delegationDepth":0}),
            json!({"type":"custom/event","data":{"meta":{"kind":"session-reference","form":"recall",
                "version":1,"references":[{"capturedFormatVersion":7}]}},"ignorable":true})
        );
        let normalized = normalize_session_format_metadata(&lookalike, None).unwrap();
        let lines: Vec<&str> = normalized.split('\n').collect();
        let header: Value = serde_json::from_str(lines[0]).unwrap();
        assert!(header.get("version").is_none());
        assert_eq!(lines[1], lookalike.split('\n').nth(1).unwrap());
    }

    /// :1006 variants are the projected ranges: persisted `[1,3],5` back to logical seq arrays.
    #[test]
    fn projects_persisted_source_event_ranges_back_to_logical_seq_arrays() {
        let out = text(&snapshot(&[
            json!({"type":"session","version":0}),
            json!({"type":"assistant/message","sourceEventSeqs":[[1,3],5],"surfaceOp":"append","data":{"turn":1,"step":1}}),
        ]));
        assert!(out.contains("\"sourceEventSeqs\":[1,2,3,5]"), "{out}");
    }

    fn system_event(text: &str, seq: u64) -> String {
        json!({"type":"system/message","seq":seq,"time":9,"data":{"turn":1,"step":1,"message":{
            "id":"11111111-1111-4111-8111-111111111111","role":"system",
            "content": if text.is_empty() { json!([]) } else { json!([{"type":"text","text":text}]) },
            "source":{"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt"}}}})
        .to_string()
    }
    const HEADER_LINE: &str = r#"{"type":"session","version":0,"id":"s","createdAt":1,"cwd":"/w"}"#;
    fn header_event(header: Value) -> String {
        json!({"type":"request/header","seq":3,"time":9,"data":{"header":header,"reason":"initial"}}).to_string()
    }

    /// :1163
    #[test]
    fn replaces_system_message_text_and_header_tools_with_tokens_keeping_config_and_reason() {
        let ev = header_event(json!({"config":{"model":"m"},
            "tools":[{"name":"read","description":"Read a file.","parameters":{"type":"object"}}]}));
        let raw = format!(
            "{HEADER_LINE}\n{}\n{ev}\n",
            system_event("You are an agent.\nBe brief.", 2)
        );
        let out = hoh_conformance::normalize::scrub_model_request_bulk(&raw).unwrap();
        assert!(out.contains("\"content\":[{\"type\":\"text\",\"text\":\"{{system}}\"}]"));
        assert!(out.contains("\"tools\":\"{{tools}}\""));
        assert!(
            out.contains("\"config\":{\"model\":\"m\"}") && out.contains("\"reason\":\"initial\"")
        );
        assert!(!out.contains("You are an agent") && !out.contains("Read a file"));
    }

    /// :1195
    #[test]
    fn passes_every_other_line_through_byte_for_byte_and_is_idempotent() {
        let other = json!({"type":"assistant/chunk","seq":4,"time":9,"data":{"turn":1,"step":1,
            "chunk":{"type":"text-delta","index":0,"text":"hi"}}})
        .to_string();
        let raw = format!(
            "{HEADER_LINE}\n{}\n{}\n{other}\n",
            system_event("s", 2),
            header_event(json!({"config":{"model":"m"},"tools":[]}))
        );
        let once = hoh_conformance::normalize::scrub_model_request_bulk(&raw).unwrap();
        let lines: Vec<&str> = once.split('\n').collect();
        assert_eq!(lines[0], HEADER_LINE);
        assert_eq!(lines[3], other);
        assert_eq!(
            hoh_conformance::normalize::scrub_model_request_bulk(&once).unwrap(),
            once
        );
    }

    /// :1206 (expanded references, idempotent)
    #[test]
    fn writes_expanded_source_event_references_and_remains_idempotent() {
        let input = [
            json!({"type":"session","id":"s"}),
            json!({"type":"assistant/message","sourceEventSeqs":[[1,3],5],"surfaceOp":"append","data":{"turn":1,"step":1}}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        let output = scrub_session_snapshot(&input).unwrap();
        assert!(output.contains("\"sourceEventSeqs\":[1,2,3,5]"));
        assert_eq!(scrub_session_snapshot(&output).unwrap(), output);
    }

    /// scrubSessionSnapshot: stable feedback clocks.
    #[test]
    fn writes_stable_feedback_clocks_while_retaining_notes_and_version_identity() {
        let input = [
            json!({"type":"session","id":"s"}),
            json!({"type":"feedback/message-put","data":{"item":{"version":"opaque-version","createdAt":12,"updatedAt":34,"note":"keep 12"}}}),
            json!({"type":"feedback/message-put","data":null}),
            json!({"type":"feedback/message-put","data":{"item":null}}),
            json!({"type":"feedback/message-put","data":{"item":{}}}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        let output = scrub_session_snapshot(&input).unwrap();
        assert!(output.contains(
            "\"version\":\"opaque-version\",\"createdAt\":0,\"updatedAt\":0,\"note\":\"keep 12\""
        ));
        assert_eq!(scrub_session_snapshot(&output).unwrap(), output);
    }

    /// scrubSessionSnapshot: Timeline clocks.
    #[test]
    fn normalizes_timeline_task_clocks_and_receipts_without_changing_descriptions() {
        let input = [
            json!({"type":"session","id":"s"}),
            json!({"type":"plugin:timeline/tree","data":{"tree":{"createdAt":12,"lastTaskUpdate":15,
                "tasks":[{"title":"Keep 12","createdAt":12,"updatedAt":15}]},"receipt":{"at":15,"previousAt":null}}}),
            json!({"type":"plugin:timeline/tree","data":{}}),
            json!({"type":"tool/result","data":{"meta":{"timeline":{"at":15,"previousAt":12},"createdAt":12}}}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
        let output = scrub_session_snapshot(&input).unwrap();
        assert!(output.contains("\"createdAt\":0,\"lastTaskUpdate\":0,\"tasks\":[{\"title\":\"Keep 12\",\"createdAt\":0,\"updatedAt\":0}]"));
        assert!(output.contains("\"receipt\":{\"at\":0,\"previousAt\":null}"));
        assert!(output.contains("\"timeline\":{\"at\":0,\"previousAt\":0},\"createdAt\":12"));
        assert_eq!(scrub_session_snapshot(&output).unwrap(), output);
    }

    /// :1247
    #[test]
    fn preserves_the_header_while_projecting_and_scrubbing_each_body_record() {
        let header = r#"  {"type":"session","version":0,"id":"s","createdAt":7}  "#;
        let request = json!({"type":"request/header","seq":1,"time":9,
            "data":{"header":{"tools":[{"name":"read"}]},"reason":"initial"}})
        .to_string();
        let event =
            json!({"type":"turn/start","seq":2,"time":10,"data":{"turn":1,"seq":41,"time":42}})
                .to_string();
        let out = scrub_session_snapshot(&format!(
            "{header}\n{}\n{request}\n{event}\n",
            system_event("secret", 0)
        ))
        .unwrap();
        assert_eq!(
            out,
            [
                header,
                r#"{"type":"system/message","data":{"turn":1,"step":1,"message":{"id":"11111111-1111-4111-8111-111111111111","role":"system","content":[{"type":"text","text":"{{system}}"}],"source":{"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt"}}}}"#,
                r#"{"type":"request/header","data":{"header":{"tools":"{{tools}}"},"reason":"initial"}}"#,
                r#"{"type":"turn/start","data":{"turn":1,"seq":41,"time":42}}"#,
                "",
            ]
            .join("\n")
        );
    }

    /// :1275
    #[test]
    fn scrubs_only_system_message_text_while_keeping_header_tools_verbatim() {
        use hoh_conformance::normalize::scrub_system_prompts;
        let header = json!({"type":"request/header","seq":1,"time":2,
            "data":{"header":{"tools":[{"name":"read","description":"full schema"}]},"reason":"initial"}})
        .to_string();
        let replaced = json!({"type":"system/message","seq":3,"time":4,
            "surfaceOp":{"op":"replace","start":0,"end":0},"sourceEventSeqs":[0],
            "data":{"turn":1,"step":2,"message":{"role":"system","content":[{"type":"text","text":"new prompt"}]}}})
        .to_string();
        let out = scrub_system_prompts(&format!(
            "{}\n{header}\n{replaced}\n",
            system_event("full prompt", 0)
        ))
        .unwrap();
        assert_eq!(out.matches("\"text\":\"{{system}}\"").count(), 2);
        assert!(!out.contains("full prompt") && !out.contains("new prompt"));
        assert!(out.contains("\"surfaceOp\":{\"op\":\"replace\",\"start\":0,\"end\":0}"));
        assert_eq!(out.split('\n').nth(1).unwrap(), header);
        assert_eq!(scrub_system_prompts(&out).unwrap(), out);
    }

    /// scrubToolSchemas: developer messages keep tool names.
    #[test]
    fn retains_historical_tool_names_when_developer_messages_reference_declarations() {
        use hoh_conformance::normalize::scrub_tool_schemas;
        let header = json!({"type":"request/header","data":{"header":{"tools":[
            {"name":"search","description":"Full schema","parameters":{}},
            {"name":"fetch","description":"Fetch schema","parameters":{}}]}}})
        .to_string();
        let update = json!({"type":"developer/message","data":{"headerSeq":0,
            "message":{"content":[{"type":"tool-addition","toolName":"fetch"}]}}})
        .to_string();
        let out = scrub_tool_schemas(&format!("{header}\n{update}\n")).unwrap();
        let first: Value = serde_json::from_str(out.split('\n').next().unwrap()).unwrap();
        assert_eq!(
            first,
            json!({"type":"request/header","data":{"header":{"tools":["search","fetch"]}}})
        );
        assert_eq!(out.split('\n').nth(1).unwrap(), update);
        assert!(!out.contains("Full schema"));
        assert_eq!(scrub_tool_schemas(&out).unwrap(), out);
    }

    /// :1315
    #[test]
    fn scrubs_only_tool_schema_payloads_while_keeping_prompts_verbatim() {
        use hoh_conformance::normalize::scrub_tool_schemas;
        let request = |seq: u64, tools: Value, reason: &str| {
            json!({"type":"request/header","seq":seq,"time":seq+1,"data":{"header":tools,"reason":reason}}).to_string()
        };
        let header = request(
            1,
            json!({"tools":[{"name":"read","description":"full schema","parameters":{"type":"object"}}]}),
            "initial",
        );
        let changed = request(
            2,
            json!({"tools":[{"name":"grep","description":"new schema"}]}),
            "change",
        );
        let toolless = request(3, json!({"config":{"model":"m"}}), "resume");
        let out = scrub_tool_schemas(&format!(
            "{}\n{header}\n{changed}\n{toolless}\n",
            system_event("full prompt", 0)
        ))
        .unwrap();
        assert_eq!(out.matches("\"tools\":\"{{tools}}\"").count(), 2);
        assert!(!out.contains("full schema") && !out.contains("new schema"));
        assert!(out.contains("full prompt"));
        assert_eq!(out.split('\n').nth(3).unwrap(), toolless);
        assert_eq!(scrub_tool_schemas(&out).unwrap(), out);
    }
}

mod native_paths_and_fixture_tools {
    use super::*;
    use hoh_conformance::normalize::{extract_snapshot_spill_paths, tokenize_session_fixture_cwd};

    /// :179
    #[test]
    fn can_preserve_native_cwd_rooted_separators_for_a_platform_golden() {
        let c = context(r"C:\work\snapshot", &[]);
        let raw = jsonl(&[json!({"path":format!("{}\\nested\\proof.txt", c.cwd)})]);
        let frame = n()
            .with_native_cwd_paths()
            .frames(&raw, &c)
            .unwrap()
            .remove(0);
        assert_eq!(frame["path"], r"{{cwd}}\nested\proof.txt");
    }

    /// :448
    #[test]
    fn shares_cwd_rooted_path_handling_with_stdout_normalization() {
        let c = context(r"C:\work\snapshot", &[]);
        let rows = [
            header(json!({"cwd":c.cwd})),
            tool_result(json!({"path":format!("{}\\nested\\proof.txt", c.cwd)})),
        ];
        assert!(log(&rows, &c).contains("{{cwd}}/nested/proof.txt"));
        let native = text(
            &n().with_native_cwd_paths()
                .legacy_log(&jsonl(&rows), &c)
                .unwrap(),
        );
        assert!(native.contains(r"{{cwd}}\\nested\\proof.txt"), "{native}");
    }

    /// :1112 (both modes)
    #[test]
    fn normalizes_nested_windows_local_spill_locators_with_canonical_and_native_paths() {
        for native in [false, true] {
            let locator =
                r"{{cwd}}\.spill\session-123456abcdef\abcdef123456-session-reference-1.txt";
            let notice =
                json!({"locator":locator,"unrelated":r"C:\work\literal\file.txt","regex":r"\d+\w"});
            let row = json!({"type":"user/message","data":{"text":notice.to_string()}});
            let normalizer = if native {
                n().with_native_cwd_paths()
            } else {
                n()
            };
            let out = normalizer.legacy_log(&jsonl(&[row]), &ctx()).unwrap();
            let mut expected = notice.clone();
            expected["locator"] = json!("{{spillLocator:session-reference-1.txt}}");
            assert_eq!(
                out,
                [json!({"type":"user/message","data":{"text":expected.to_string()}})],
                "native={native}"
            );
        }
    }

    /// :1092 (three root/separator pairs)
    #[test]
    fn recognizes_locators_with_each_separator_in_nested_json_omissions_without_scrubbing_byte_counts()
     {
        for (root, separator) in [("/tmp", "/"), ("/tmp", "\\"), ("C:/t", "\\")] {
            let locator = format!("{root}/dsh-acp-snap-123456789/session-123456abcdef/abcdef123456-session-reference-1.txt")
                .replace('/', separator);
            let notice = json!({"sessionId":"source","omittedBytes":42,
                "fullSnapshot":{"status":"saved","locator":locator,"bytes":1234}});
            let row = json!({"type":"user/message","data":{"content":[{"type":"text","text":json!([notice]).to_string()}]}});
            let log_text = row.to_string();
            let encoded = json!(locator).to_string();
            let encoded = encoded[1..encoded.len() - 1].replace('\\', "\\\\");
            let encoded_in_log = {
                // The locator as it appears inside the doubly encoded text.
                let once = json!(locator).to_string();
                let inner = &once[1..once.len() - 1];
                let twice = json!(inner).to_string();
                twice[1..twice.len() - 1].to_owned()
            };
            let _ = encoded;
            let found = extract_snapshot_spill_paths(&log_text).unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found["session-reference-1.txt"], encoded_in_log);
            let normalized = n().legacy_log(&log_text, &ctx()).unwrap();
            let mut expected = notice.clone();
            expected["fullSnapshot"]["locator"] = json!("{{spillLocator:session-reference-1.txt}}");
            let expected_row = json!({"type":"user/message","data":{"content":[{"type":"text","text":json!([expected]).to_string()}]}});
            assert_eq!(normalized, [expected_row]);
            let unrelated =
                "/tmp/unrelated/session-123456abcdef/abcdef123456-session-reference-1.txt";
            let swapped = log_text.replace(&encoded_in_log, unrelated);
            assert!(text(&n().legacy_log(&swapped, &ctx()).unwrap()).contains(unrelated));
        }
    }

    /// :1122
    #[test]
    fn maps_each_spill_filename_to_its_full_matched_path_last_match_wins_per_name() {
        let log = [
            "Full formatted result stored at: /tmp/dsh-acp-snapshot-spill/session-c22bc3f1d2af/8a7b6c5d4e3f-bash.txt. Use read with offset/limit, or grep this path to search within it.",
            "stale copy at /tmp/dsh-acp-snap-012345678/session-aaaaaaaaaaaa/bbbbbbbbbbbb-grep.txt then",
            "fresh copy at /tmp/dsh-acp-snap-012345678/session-cccccccccccc/dddddddddddd-grep.txt then",
        ]
        .join("\n");
        let found = extract_snapshot_spill_paths(&log).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(
            found["bash.txt"],
            "/tmp/dsh-acp-snapshot-spill/session-c22bc3f1d2af/8a7b6c5d4e3f-bash.txt"
        );
        assert_eq!(
            found["grep.txt"],
            "/tmp/dsh-acp-snap-012345678/session-cccccccccccc/dddddddddddd-grep.txt"
        );
    }

    /// :1134
    #[test]
    fn returns_an_empty_map_when_the_log_carries_no_snapshot_spill_paths() {
        assert!(
            extract_snapshot_spill_paths("no spill paths here, only /tmp/other.txt\n")
                .unwrap()
                .is_empty()
        );
    }

    /// :1006 (macOS, Linux, Windows)
    #[test]
    fn stores_temporary_workspaces_with_one_portable_root_token() {
        for (cwd, reported) in [
            (
                "/var/folders/2g/snapshot/T/acp-snap-cwd-abc123",
                "/private/var/folders/2g/snapshot/T/acp-snap-cwd-abc123",
            ),
            ("/tmp/acp-snap-cwd-abc123", "/tmp/acp-snap-cwd-abc123"),
            (
                r"C:\Users\runner\AppData\Local\Temp\acp-snap-cwd-abc123",
                r"C:\Users\runner\AppData\Local\Temp\acp-snap-cwd-abc123",
            ),
        ] {
            let raw = format!(
                "{}\n{}\n",
                json!({"type":"session","id":"s","createdAt":1,"cwd":cwd}),
                json!({"type":"tool/result","seq":1,"time":2,"data":{"content":[{"type":"text","text":format!(
                    "wrote {reported}/proof.txt. alias /different/root/acp-snap-cwd-abc123/alias.txt. cwd {cwd}. Next; kept {cwd}-backup, {cwd}.backup, and /tmp/authored.txt")}]}})
            );
            let out = tokenize_session_fixture_cwd(&raw).unwrap();
            let result: Value = serde_json::from_str(out.split('\n').nth(1).unwrap()).unwrap();
            let body = result["data"]["content"][0]["text"].as_str().unwrap();
            assert!(out.contains("\"cwd\":\"{{cwd}}\""), "{out}");
            assert!(body.contains("wrote {{cwd}}/proof.txt"), "{body}");
            assert!(body.contains("alias {{cwd}}/alias.txt"), "{body}");
            assert!(body.contains("cwd {{cwd}}. Next"), "{body}");
            assert!(
                body.contains(&format!("{cwd}-backup")) && body.contains(&format!("{cwd}.backup"))
            );
            assert!(body.contains("/tmp/authored.txt"));
            assert!(!body.contains(&format!("{reported}/proof.txt")));
            assert_eq!(tokenize_session_fixture_cwd(&out).unwrap(), out);
        }
    }

    /// :1066
    #[test]
    fn collapses_a_residual_macos_realpath_prefix_around_an_existing_cwd_token() {
        let raw = format!(
            "{}\n{}\n",
            json!({"type":"session","id":"s","createdAt":1,"cwd":"{{cwd}}"}),
            json!({"type":"tool/result","seq":1,"time":2,"data":{"content":[{"type":"text","text":"wrote /private{{cwd}}/proof.txt"}]}})
        );
        let out = tokenize_session_fixture_cwd(&raw).unwrap();
        assert!(
            out.contains("wrote {{cwd}}/proof.txt") && !out.contains("/private{{cwd}}"),
            "{out}"
        );
        assert_eq!(tokenize_session_fixture_cwd(&out).unwrap(), out);
    }

    /// :1084
    #[test]
    fn rejects_a_log_without_a_session_cwd() {
        assert_eq!(
            tokenize_session_fixture_cwd("").unwrap_err(),
            "acp-snapshot: cannot tokenize a cwd without a basename"
        );
    }
}
