mod support;
use futures_util::StreamExt;
use hoh_llm::*;
use hoh_llm_replay::*;
use hoh_session::SessionEvent;
use hoh_timeout::AbortController;
use serde_json::{Value, json};
#[tokio::test]
async fn binds_by_call_order_even_when_second_stream_is_polled_first() {
    let mut files = Files::new(&[chunks("parent")]);
    files.child("child", 200, &[chunks("child")]);
    let (kernel, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &files.config).unwrap();
    let first = llm.stream(&ctx, options(Some("first")));
    let second = llm.stream(&ctx, options(Some("second")));
    assert_eq!(drain(second).await.unwrap(), chunks("child"));
    assert_eq!(drain(first).await.unwrap(), chunks("parent"));
    kernel.shutdown().await;
}
use support::*;
#[test]
fn reports_invalid_json_at_its_physical_source_line() {
    assert!(
        parse_session_log(&(jsonl(&[header("p", 0)]) + "{\"type\":\n"))
            .unwrap_err()
            .to_string()
            .contains("line 2 contains invalid JSON")
    );
}
#[test]
fn skips_the_header_line_and_parses_each_event() {
    let e = json!({"type":"turn/start","seq":0,"time":0,"data":{"turn":1}});
    assert_eq!(
        parse_session_log(&jsonl(&[header("p", 0), e.clone()])).unwrap()[0].as_value(),
        &e
    );
}
#[test]
fn ignores_blank_lines() {
    let raw = fixture("p", 0, &[]).replace('\n', "\n\n");
    assert_eq!(parse_session_log(&raw).unwrap().len(), 2);
}
#[test]
fn rejects_non_object_body_rows_with_their_source_line() {
    for row in ["null", "[]", "42"] {
        let e = parse_session_log(&(jsonl(&[header("p", 0)]) + row)).unwrap_err();
        assert!(e.to_string().contains("line 2 must be a JSON object"));
    }
}
#[test]
fn rejects_partial_and_mixed_projected_envelopes_at_their_source_lines() {
    for (rows, reason) in [
        (
            vec![json!({"type":"turn/start","seq":0,"data":{"turn":1}})],
            "line 2 must contain both seq and time",
        ),
        (
            vec![
                json!({"type":"turn/start","data":{"turn":1}}),
                json!({"type":"permission/preset","seq":1,"time":0,"data":{"preset":"auto"}}),
            ],
            "line 3 cannot mix projected and complete",
        ),
    ] {
        let mut all = vec![header("p", 0)];
        all.extend(rows);
        assert!(
            parse_session_log(&jsonl(&all))
                .unwrap_err()
                .to_string()
                .contains(reason)
        );
    }
}
#[test]
fn rejects_an_empty_fixture_instead_of_inventing_header_identity() {
    assert!(
        parse_session_header("")
            .unwrap_err()
            .to_string()
            .contains("must start with a session header")
    );
}
#[test]
fn rejects_a_projected_header_that_lacks_required_identity_fields() {
    assert!(
        parse_session_header("{\"type\":\"session\",\"version\":4}")
            .unwrap_err()
            .to_string()
            .contains("id")
    );
}
#[test]
fn keeps_arbitrary_future_request_header_fields_observable_after_current_format_restoration() {
    let mut rows: Vec<Value> = fixture("p", 0, &[chunks("hi")])
        .lines()
        .map(|s| hoh_util_values::js_json_parse(s).unwrap())
        .collect();
    rows.insert(3, json!({"type":"request/header","data":{"reason":"initial","header":{"config":{"provider":"m","model":"m"},"futureHeader":"unexpected value"}}}));
    let first = prepare_session_snapshot_fixture_for_comparison(&jsonl(&rows)).unwrap();
    assert!(first.contains("futureHeader"));
    rows[3]["data"]["header"]
        .as_object_mut()
        .unwrap()
        .shift_remove("futureHeader");
    assert_ne!(
        first,
        prepare_session_snapshot_fixture_for_comparison(&jsonl(&rows)).unwrap()
    );
}
#[test]
fn encodes_a_fixture_without_inventing_a_trailing_newline_and_retains_its_cwd_token() {
    let raw = fixture("p", 0, &[chunks("hi")]);
    let raw = raw.trim_end_matches('\n');
    let encoded = prepare_session_snapshot_fixture_for_comparison(raw).unwrap();
    assert!(!encoded.ends_with('\n'));
    assert!(encoded.contains("{{cwd}}"));
    assert_eq!(
        parse_session_log(raw).unwrap(),
        parse_session_log(&encoded).unwrap()
    );
}
#[test]
fn comparison_uses_the_catalogs_canonical_physical_header_order() {
    let raw = r#"{"type":"session","version":4,"id":"s","createdAt":0,"delegationDepth":0,"isSeeded":false}"#;
    assert_eq!(
        prepare_session_snapshot_fixture_for_comparison(raw).unwrap(),
        r#"{"type":"session","version":4,"id":"s","createdAt":0,"isSeeded":false,"delegationDepth":0}"#,
    );
}
#[test]
fn comparison_reencodes_decoded_source_event_sequences_as_physical_ranges() {
    let mut rows = vec![header("s", 0)];
    for seq in 0..3 {
        rows.push(json!({"type":"extension/fact","seq":seq,"time":0,"ignorable":true,"data":{"value":seq}}));
    }
    rows.push(json!({"type":"extension/fact","seq":3,"time":0,"ignorable":true,"sourceEventSeqs":[0,1,2],"data":{"value":"combined"}}));
    let text = jsonl(&rows);
    let expected = text.replace("\"sourceEventSeqs\":[0,1,2]", "\"sourceEventSeqs\":[[0,2]]");
    assert_ne!(expected, text);
    let prepared = prepare_session_snapshot_fixture_for_comparison(&text).unwrap();
    assert_eq!(prepared, expected);
    assert_eq!(
        parse_session_log(&prepared).unwrap(),
        parse_session_log(&text).unwrap(),
    );
}
#[test]
fn derives_current_replay_calls_from_a_fixture_with_tokenized_request_tools() {
    let raw = fixture("p", 0, &[chunks("hi")]);
    let mut rows: Vec<Value> = raw
        .lines()
        .map(|s| hoh_util_values::js_json_parse(s).unwrap())
        .collect();
    rows.insert(3,json!({"type":"request/header","data":{"reason":"initial","header":{"config":{"provider":"m","model":"m"},"tools":"{{tools}}"}}}));
    let text = jsonl(&rows);
    assert_eq!(
        derive_replay_script(&parse_session_log(&text).unwrap()).unwrap(),
        vec![ReplayEntry::Chunks {
            chunks: chunks("hi")
        }]
    );
    assert!(
        prepare_session_snapshot_fixture_for_comparison(&text)
            .unwrap()
            .contains("{{tools}}")
    );
}
#[test]
fn applies_current_session_validation_before_comparison() {
    let rows = vec![
        header("p", 0),
        json!({"type":"user/message","data":{"id":"u","role":"assistant","content":[],"source":{"kind":"user"}},"surfaceOp":"append"}),
    ];
    assert!(
        prepare_session_snapshot_fixture_for_comparison(&jsonl(&rows))
            .unwrap_err()
            .to_string()
            .contains("role \"user\"")
    );
}
#[test]
fn expands_one_finished_embedded_assistant_stream_into_one_replay_entry() {
    assert_eq!(
        derive_replay_script(&[stream_event(0, 1, 1, &chunks("hi"))]).unwrap(),
        vec![ReplayEntry::Chunks {
            chunks: chunks("hi")
        }]
    );
}
#[test]
fn separates_retry_calls_that_share_one_turn_and_step() {
    let failed = vec![StreamChunk::finish(FinishReason::failed(
        "error",
        LlmFailure::new("empty", "EMPTY_RESPONSE"),
    ))];
    assert_eq!(
        derive_replay_script(&[
            stream_event(0, 1, 1, &failed),
            stream_event(1, 1, 1, &chunks("hi"))
        ])
        .unwrap(),
        vec![
            ReplayEntry::Chunks { chunks: failed },
            ReplayEntry::Chunks {
                chunks: chunks("hi")
            }
        ]
    );
}
#[test]
fn produces_one_entry_per_distinct_turn_step_in_log_order() {
    assert_eq!(
        derive_replay_script(&[
            stream_event(0, 1, 1, &chunks("one")),
            stream_event(1, 1, 2, &chunks("two"))
        ])
        .unwrap(),
        vec![
            ReplayEntry::Chunks {
                chunks: chunks("one")
            },
            ReplayEntry::Chunks {
                chunks: chunks("two")
            }
        ]
    );
}
#[test]
fn separates_calls_across_turns_too() {
    assert_eq!(
        derive_replay_script(&[
            stream_event(0, 1, 1, &chunks("one")),
            stream_event(1, 2, 1, &chunks("two"))
        ])
        .unwrap()
        .len(),
        2
    );
}
#[test]
fn ignores_non_assistant_stream_events() {
    let e = SessionEvent::from_value(json!({"type":"turn/start","data":{"turn":1}}));
    assert_eq!(
        derive_replay_script(&[e, stream_event(1, 1, 1, &chunks("hi"))])
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn returns_an_empty_script_for_a_log_with_no_assistant_stream_events() {
    assert!(derive_replay_script(&[]).unwrap().is_empty());
    assert!(
        derive_replay_script(&[stream_event(0, 1, 1, &[])])
            .unwrap()
            .is_empty()
    );
}
#[test]
fn keeps_a_finish_error_chunk_in_the_derived_entry() {
    let cs = vec![StreamChunk::finish(FinishReason::failed(
        "error",
        LlmFailure::new("boom", "X"),
    ))];
    assert_eq!(
        derive_replay_script(&[stream_event(0, 1, 1, &cs)]).unwrap(),
        vec![ReplayEntry::Chunks { chunks: cs }]
    );
}
#[test]
fn inserts_compaction_summary_output_between_the_calls_surrounding_it() {
    let overflow = vec![StreamChunk::finish(FinishReason::failed(
        "error",
        LlmFailure::new("too large", "CONTEXT_WINDOW_EXCEEDED"),
    ))];
    let summary = SessionEvent::from_value(
        json!({"type":"compaction/summary","data":{"llmStreamCall":true,"rawOutput":[{"type":"text","text":"durable checkpoint"}],"usage":{"inputTokens":9,"outputTokens":2}}}),
    );
    let result = derive_replay_script(&[
        stream_event(0, 1, 2, &overflow),
        SessionEvent::from_value(
            json!({"type":"compaction/start","data":{"compactionId":"replay-compaction","turn":1}}),
        ),
        summary,
        stream_event(3, 1, 2, &chunks("hi")),
    ])
    .unwrap();
    let summary_chunks: Vec<StreamChunk> = serde_json::from_value(json!([
        {"type":"block-start","index":0,"blockType":"text"},
        {"type":"block-end","index":0,"block":{"type":"text","text":"durable checkpoint"}},
        {"type":"usage","usage":{"inputTokens":9,"outputTokens":2}},
        {"type":"finish","reason":{"kind":"stop"}}
    ]))
    .unwrap();
    let expected = vec![
        ReplayEntry::Chunks { chunks: overflow },
        ReplayEntry::Chunks {
            chunks: summary_chunks,
        },
        ReplayEntry::Chunks {
            chunks: chunks("hi"),
        },
    ];
    assert_eq!(result, expected);
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}
#[test]
fn does_not_infer_an_llm_call_from_summary_without_raw_output() {
    let e = SessionEvent::from_value(json!({"type":"compaction/summary","data":{"summary":[]}}));
    assert!(derive_replay_script(&[e]).unwrap().is_empty());
}
#[test]
fn does_not_infer_a_local_llm_call_from_external_compact_output() {
    let e = SessionEvent::from_value(
        json!({"type":"compaction/summary","data":{"rawOutput":[{"type":"text","text":"remote"}]}}),
    );
    assert!(derive_replay_script(&[e]).unwrap().is_empty());
}
#[test]
fn rejects_a_durable_compact_stream_marker_whose_complete_output_is_absent() {
    let e = SessionEvent::from_value(
        json!({"type":"compaction/summary","data":{"llmStreamCall":true}}),
    );
    assert!(
        derive_replay_script(&[e])
            .unwrap_err()
            .to_string()
            .contains("without rawOutput")
    );
}
#[test]
fn derives_a_compaction_summary_stream_when_usage_is_unavailable() {
    let e = SessionEvent::from_value(
        json!({"type":"compaction/summary","data":{"llmStreamCall":true,"rawOutput":[{"type":"text","text":"summary"}]}}),
    );
    let ReplayEntry::Chunks { chunks } = derive_replay_script(&[e]).unwrap().remove(0) else {
        panic!()
    };
    assert_eq!(chunks.len(), 3);
}
fn unfinished() -> SessionEvent {
    stream_event(0, 2, 3, &chunks("hi")[..2])
}
#[test]
fn throws_on_a_group_that_lacks_a_terminal_finish_chunk() {
    assert!(
        derive_replay_script(&[unfinished()])
            .unwrap_err()
            .to_string()
            .contains("without a finish chunk")
    );
}
#[test]
fn names_the_offending_turn_step_when_a_group_is_incomplete() {
    assert!(
        derive_replay_script(&[unfinished()])
            .unwrap_err()
            .to_string()
            .contains("2/3")
    );
}
#[test]
fn rejects_an_unfinished_call_before_consuming_chunks_from_a_new_step() {
    assert!(
        derive_replay_script(&[unfinished(), stream_event(1, 2, 4, &chunks("later"))]).is_err()
    );
}
#[test]
fn rejects_an_unfinished_call_at_a_compact_summary_boundary() {
    let e = SessionEvent::from_value(json!({"type":"compaction/summary","data":{}}));
    assert!(derive_replay_script(&[unfinished(), e]).is_err());
}
#[test]
fn derives_from_the_session_jsonl_when_no_override_is_present() {
    let f = Files::new(&[chunks("hi")]);
    assert_eq!(
        load_replay_script(&f.config).unwrap(),
        vec![ReplayEntry::Chunks {
            chunks: chunks("hi")
        }]
    );
}
#[test]
fn never_rewrites_a_projected_source_fixture() {
    let f = Files::new(&[chunks("hi")]);
    let before = std::fs::read(&f.config.file).unwrap();
    load_replay_script(&f.config).unwrap();
    assert_eq!(std::fs::read(&f.config.file).unwrap(), before);
}
#[test]
fn uses_the_sidecar_override_when_present_ignoring_the_derived_script() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"throw","chunks":[],"message":"401","code":"AUTH"}]));
    assert!(
        matches!(&load_replay_script(&f.config).unwrap()[0],ReplayEntry::Throw{code,..}if code=="AUTH")
    );
}
#[test]
fn uses_a_whole_script_override_reused_as_the_primary_fixture_path() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"hang"}]));
    f.config.file = f.config.override_file.clone().unwrap();
    assert!(matches!(
        &load_replay_script(&f.config).unwrap()[0],
        ReplayEntry::Hang { .. }
    ));
}
#[test]
fn falls_back_to_the_jsonl_when_the_override_path_is_set_but_absent() {
    let mut f = Files::new(&[chunks("hi")]);
    f.config.override_file = Some(f.root.path().join("missing").to_string_lossy().into_owned());
    assert_eq!(load_replay_script(&f.config).unwrap().len(), 1);
}
#[test]
fn fails_loud_when_the_fixture_is_missing() {
    let f = ReplayConfig {
        file: "/nonexistent/hoh-llm-replay-fixture".into(),
        ..Default::default()
    };
    assert!(
        load_replay_script(&f)
            .unwrap_err()
            .to_string()
            .contains("fixture not found")
    );
}
#[test]
fn rejects_an_override_document_that_is_neither_supported_form() {
    let mut f = Files::new(&[]);
    f.override_doc(json!({"not":"array"}));
    assert!(
        load_replay_script(&f.config)
            .unwrap_err()
            .to_string()
            .contains("document must be")
    );
}
#[test]
fn patches_form_swaps_the_named_call_index_and_keeps_derived_siblings() {
    let mut f = Files::new(&[chunks("one"), chunks("two")]);
    f.override_doc(json!({"patches":[{"at":0,"entry":{"kind":"hang"}}]}));
    assert_eq!(
        load_replay_script(&f.config).unwrap()[1],
        ReplayEntry::Chunks {
            chunks: chunks("two")
        }
    );
}
#[test]
fn patches_form_at_derived_length_appends() {
    let mut f = Files::new(&[chunks("one")]);
    f.override_doc(json!({"patches":[{"at":1,"entry":{"kind":"hang"}}]}));
    assert_eq!(load_replay_script(&f.config).unwrap().len(), 2);
}
#[test]
fn patches_form_out_of_range_index_fails_loud_with_derived_length() {
    let mut f = Files::new(&[chunks("one")]);
    f.override_doc(json!({"patches":[{"at":2,"entry":{"kind":"hang"}}]}));
    assert!(
        load_replay_script(&f.config)
            .unwrap_err()
            .to_string()
            .contains("derived script has 1 call(s)")
    );
}
#[test]
fn rejects_duplicate_patch_indexes() {
    let mut f = Files::new(&[chunks("one")]);
    f.override_doc(
        json!({"patches":[{"at":0,"entry":{"kind":"hang"}},{"at":0,"entry":{"kind":"hang"}}]}),
    );
    assert!(
        load_replay_script(&f.config)
            .unwrap_err()
            .to_string()
            .contains("duplicate override patch index 0")
    );
}
#[test]
fn validates_patch_and_entry_shapes_at_the_file_boundary() {
    let mut f = Files::new(&[]);
    for (doc, reason) in [
        (Value::Null, "document must be"),
        (json!({"patches":[null]}), "exactly at and entry"),
        (
            json!({"patches":[{"at":-1,"entry":{"kind":"hang"}}]}),
            "non-negative safe integer",
        ),
        (json!([42]), "must be an object"),
        (
            json!([{"kind":"chunks","chunks":"bad"}]),
            "chunks must be an array",
        ),
        (
            json!([{"kind":"chunks","chunks":[],"extra":true}]),
            "invalid chunks-entry fields",
        ),
        (
            json!([{"kind":"chunks","chunks":[{"type":"bogus"}]}]),
            "known StreamChunk type",
        ),
        (
            json!([{"kind":"throw","chunks":[],"message":"","code":"X"}]),
            "message must be",
        ),
        (
            json!([{"kind":"throw","chunks":[],"message":"x","code":""}]),
            "code must be",
        ),
        (
            json!([{"kind":"hang","extra":true}]),
            "invalid hang-entry fields",
        ),
        (json!([{"kind":"hang","readyFile":1}]), "readyFile must be"),
        (json!([{"kind":"bogus"}]), "unknown kind"),
        (
            json!([{"kind":"throw","chunks":[],"message":"x","code":"X","accepted":"yes"}]),
            "accepted must be a boolean",
        ),
    ] {
        f.override_doc(doc);
        assert!(
            load_replay_script(&f.config)
                .unwrap_err()
                .to_string()
                .contains(reason),
            "{reason}"
        );
    }
    let doc = json!([{"kind":"chunks","chunks":[{"type":"finish"}]}]);
    f.override_doc(doc.clone());
    f.config.file = f.config.override_file.clone().unwrap();
    assert_eq!(
        serde_json::to_value(load_replay_script(&f.config).unwrap()).unwrap(),
        doc
    );
}
#[test]
fn sidecars_admit_known_chunk_tags_without_checking_other_chunk_fields() {
    let mut f = Files::new(&[]);
    let raw = json!([
        {"opaque":1,"type":"block-start"},
        {"type":"text-delta","index":"invalid","text":null},
        {"type":"reasoning-delta"},
        {"type":"tool-call-delta","id":false},
        {"type":"block-end","block":42},
        {"type":"usage","usage":{"cost":0.5}},
        {"type":"finish"}
    ]);
    for entry in [
        json!({"kind":"chunks","chunks":raw}),
        json!({"kind":"throw","chunks":raw,"message":"boom","code":"X","accepted":false}),
    ] {
        let doc = json!([entry]);
        f.override_doc(doc.clone());
        let loaded = serde_json::to_value(load_replay_script(&f.config).unwrap()).unwrap();
        assert_eq!(
            hoh_util_values::js_json_stringify(&loaded),
            hoh_util_values::js_json_stringify(&doc)
        );
        let patch = json!({"patches":[{"at":0,"entry":entry}]});
        f.override_doc(patch);
        let loaded = serde_json::to_value(load_replay_script(&f.config).unwrap()).unwrap();
        assert_eq!(
            hoh_util_values::js_json_stringify(&loaded),
            hoh_util_values::js_json_stringify(&doc)
        );
    }
}
#[test]
fn from_request_substitutions_cover_last_capture_whole_match_and_brace_quantifier() {
    let messages = message("old goal-dead new goal-42ab");
    for (template, expected) in [
        ("{{fromRequest:(goal-[0-9a-z]+)}}", "goal-42ab"),
        ("{{fromRequest:goal-[0-9a-z]+}}", "goal-42ab"),
        ("{{fromRequest:goal-[0-9a-z]{4}}}", "goal-42ab"),
    ] {
        let e = ReplayEntry::Chunks {
            chunks: chunks(template),
        };
        assert_eq!(
            resolve_scripted_entry(&e, &messages).unwrap().into_owned(),
            ReplayEntry::Chunks {
                chunks: chunks(expected)
            }
        );
    }
}
#[test]
fn from_request_errors_are_loud_for_missing_invalid_and_unterminated_patterns() {
    for (text, reason) in [
        ("{{fromRequest:task-[0-9]+}}", "matched nothing"),
        ("{{fromRequest:(goal-}}", "invalid pattern"),
        ("{{fromRequest:goal-1", "unterminated"),
    ] {
        assert!(
            resolve_scripted_entry(
                &ReplayEntry::Chunks {
                    chunks: chunks(text)
                },
                &message("goal-42ab")
            )
            .unwrap_err()
            .to_string()
            .contains(reason)
        );
    }
}
#[test]
fn documents_the_deferred_from_request_lookahead_error_exactly() {
    let entry = ReplayEntry::Chunks {
        chunks: chunks("{{fromRequest:goal-(?=42)42}}"),
    };
    assert_eq!(
        resolve_scripted_entry(&entry, &message("goal-42"))
            .unwrap_err()
            .to_string(),
        concat!(
            "llm-replay: fromRequest has an invalid pattern \"goal-(?=42)42\": SyntaxError: regex parse error:\n",
            "    goal-(?=42)42\n",
            "         ^^^\n",
            "error: look-around, including look-ahead and look-behind, is not supported",
        ),
    );
}
#[test]
fn returns_the_exact_same_entry_when_no_placeholder_appears() {
    let e = ReplayEntry::Chunks {
        chunks: chunks("hi"),
    };
    let resolved = resolve_scripted_entry(&e, &[]).unwrap();
    assert!(matches!(resolved, std::borrow::Cow::Borrowed(_)));
    assert!(std::ptr::eq(resolved.as_ref(), &e));
}
#[test]
fn skips_non_string_request_leaves_when_building_the_corpus() {
    let mut messages = message("goal-42ab");
    let RequestMessage::Input(input) = &mut messages[0] else {
        panic!()
    };
    input.content[0].data.insert("seq".into(), json!(7));
    let entry = ReplayEntry::Chunks {
        chunks: chunks("{{fromRequest:goal-42[a-z]+}}"),
    };
    assert_eq!(
        resolve_scripted_entry(&entry, &messages)
            .unwrap()
            .into_owned(),
        ReplayEntry::Chunks {
            chunks: chunks("goal-42ab")
        }
    );
}
#[test]
fn returns_one_primary_script_for_a_single_session() {
    let f = Files::new(&[chunks("hi")]);
    let s = load_session_scripts(&f.config).unwrap();
    assert!(s[0].primary);
    assert_eq!(s[0].recorded_id, "p");
    assert_eq!(s[0].created_at, 100);
}
#[test]
fn orders_parent_children_by_created_at_with_primary_first_on_tie() {
    let mut f = Files::new(&[chunks("p")]);
    f.child("later", 200, &[chunks("late")]);
    f.child("tie", 100, &[chunks("tie")]);
    let s = load_session_scripts(&f.config).unwrap();
    assert_eq!(
        s.iter().map(|s| s.recorded_id.as_str()).collect::<Vec<_>>(),
        ["p", "tie", "later"]
    );
}
#[test]
fn throws_when_a_declared_child_fixture_is_missing() {
    let mut f = Files::new(&[]);
    f.config.child_files = Some(vec![
        f.root.path().join("missing").to_string_lossy().into_owned(),
    ]);
    assert!(
        load_session_scripts(&f.config)
            .unwrap_err()
            .to_string()
            .contains("child fixture not found")
    );
}
#[test]
fn uses_the_override_for_primary_and_still_derives_children() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"hang"}]));
    f.child("c", 200, &[chunks("child")]);
    let s = load_session_scripts(&f.config).unwrap();
    assert!(matches!(s[0].entries[0], ReplayEntry::Hang { .. }));
    assert_eq!(
        s[1].entries,
        vec![ReplayEntry::Chunks {
            chunks: chunks("child")
        }]
    );
}
#[test]
fn defaults_primary_header_when_only_override_exists() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"hang"}]));
    std::fs::remove_file(&f.config.file).unwrap();
    let s = load_session_scripts(&f.config).unwrap();
    assert_eq!(s[0].recorded_id, "");
    assert_eq!(s[0].created_at, 0);
}
#[test]
fn does_not_parse_a_whole_script_override_reused_as_primary_path() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"hang"}]));
    f.config.file = f.config.override_file.clone().unwrap();
    assert_eq!(load_session_scripts(&f.config).unwrap()[0].recorded_id, "");
}
#[test]
fn orders_two_same_created_at_children_deterministically_after_primary() {
    for (first, second, ordered) in [
        ("c2", "c1", ["c1", "c2"]),
        ("B", "a", ["a", "B"]),
        ("z", "é", ["é", "z"]),
    ] {
        let mut f = Files::new(&[]);
        f.child(first, 100, &[chunks(first)]);
        f.child(second, 100, &[chunks(second)]);
        let s = load_session_scripts(&f.config).unwrap();
        assert_eq!(
            s.iter().map(|s| s.recorded_id.as_str()).collect::<Vec<_>>(),
            ["p", ordered[0], ordered[1]]
        );
        for (script, id) in s[1..].iter().zip(ordered) {
            assert_eq!(
                script.entries,
                vec![ReplayEntry::Chunks { chunks: chunks(id) }]
            );
        }
    }
}
#[test]
fn keeps_primary_first_even_when_child_sorts_before_it() {
    let mut f = Files::new(&[]);
    f.child("early", 1, &[]);
    assert!(load_session_scripts(&f.config).unwrap()[0].primary);
}
#[test]
fn derives_a_fork_child_script_from_its_own_events_only() {
    let mut f = Files::new(&[chunks("parent")]);
    let child = f.child("c", 200, &[]);
    let mut rows: Vec<Value> = fixture("c", 200, &[chunks("parent")])
        .lines()
        .map(|l| hoh_util_values::js_json_parse(l).unwrap())
        .collect();
    rows[0]["isSeeded"] = json!(true);
    let cut = rows.len() - 1;
    rows.push(json!({"type":"session/end-seed","data":{"inherited":true}}));
    let own: Vec<Value> = fixture("c", 200, &[chunks("child")])
        .lines()
        .skip(1)
        .map(|l| hoh_util_values::js_json_parse(l).unwrap())
        .collect();
    for mut row in own {
        row["data"]["turn"] = json!(2);
        if row["type"] == "assistant/message" {
            row["data"]["message"]["id"] = json!("child-own");
        }
        rows.push(row);
    }
    std::fs::write(child, jsonl(&rows)).unwrap();
    let scripts = load_session_scripts(&f.config).unwrap();
    assert_eq!(
        scripts[1].entries,
        vec![ReplayEntry::Chunks {
            chunks: chunks("child")
        }]
    );
    assert_eq!(
        parse_session_header(&jsonl(&rows))
            .unwrap()
            .inherited_event_count
            .get(),
        cut as u64
    );
}
#[test]
fn rejects_retired_request_header_system_before_current_comparison() {
    let rows = vec![
        header("p", 0),
        json!({"type":"request/header","data":{"reason":"initial","header":{"config":{"provider":"m","model":"m"},"system":"retired prompt"}}}),
    ];
    assert!(
        prepare_session_snapshot_fixture_for_comparison(&jsonl(&rows))
            .unwrap_err()
            .to_string()
            .contains("header.system")
    );
}
#[test]
fn rejects_genuine_empty_current_tools_instead_of_treating_them_as_placeholder() {
    let mut v3 = header("p", 0);
    v3["version"] = json!(3);
    let rows = vec![
        v3,
        json!({"type":"turn/start","data":{"turn":1}}),
        json!({"type":"request/header","data":{"reason":"initial","header":{"config":{"provider":"m","model":"m"},"tools":[]}}}),
    ];
    for error in [
        parse_session_log(&jsonl(&rows)).unwrap_err(),
        prepare_session_snapshot_fixture_for_comparison(&jsonl(&rows)).unwrap_err(),
    ] {
        let text = error.to_string();
        assert!(text.starts_with("session snapshot line 3:"), "{text}");
        assert!(
            text.contains("empty optional header fields must be omitted"),
            "{text}"
        );
    }
}

#[tokio::test]
async fn serves_derived_chunks_back_short_circuiting_the_adapter() {
    let f = Files::new(&[chunks("hi")]);
    let (k, ctx, llm) = runtime().await;
    let h = install_llm_replay(&ctx, &f.config).unwrap();
    assert_eq!(
        drain(llm.stream(&ctx, options(None))).await.unwrap(),
        chunks("hi")
    );
    h.assert_consumed().unwrap();
    k.shutdown().await;
}
#[tokio::test]
async fn replays_recorded_read_write_and_bash_calls_through_the_real_runtime() {
    for (scenario, tool) in [
        ("fs-read", "read"),
        ("fs-write", "write"),
        ("bash-tool-turn", "bash"),
    ] {
        let file = format!(
            "/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/snapshots/session/{scenario}/session.v4.jsonl"
        );
        let config = ReplayConfig {
            file,
            ..Default::default()
        };
        let expected = load_replay_script(&config).unwrap();
        assert!(expected.iter().any(|entry|matches!(entry,ReplayEntry::Chunks{chunks} if chunks.iter().any(|c|matches!(c,StreamChunk::BlockEnd{block,..} if block.r#type=="tool-call" && block.data.get("name").is_some_and(|name|name==tool))))),"missing recorded {tool} call");
        let (k, ctx, llm) = runtime().await;
        let handle = install_llm_replay(&ctx, &config).unwrap();
        for entry in expected {
            let ReplayEntry::Chunks { chunks } = entry else {
                panic!("ordinary fixture must derive chunks")
            };
            assert_eq!(
                drain(llm.stream(&ctx, options(None))).await.unwrap(),
                chunks,
                "{scenario}"
            );
        }
        handle.assert_consumed().unwrap();
        k.shutdown().await;
    }
}
#[tokio::test]
async fn serves_the_nth_call_the_nth_derived_entry() {
    let f = Files::new(&[chunks("one"), chunks("two")]);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    for text in ["one", "two"] {
        assert_eq!(
            drain(llm.stream(&ctx, options(None))).await.unwrap(),
            chunks(text)
        );
    }
    k.shutdown().await;
}
#[tokio::test]
async fn routes_each_live_session_to_its_script_by_first_call_order() {
    let mut f = Files::new(&[chunks("parent")]);
    f.child("c", 200, &[chunks("child")]);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    for (id, text) in [("A", "parent"), ("B", "child")] {
        assert_eq!(
            drain(llm.stream(&ctx, options(Some(id)))).await.unwrap(),
            chunks(text)
        );
    }
    assert!(
        drain(llm.stream(&ctx, options(Some("A"))))
            .await
            .unwrap_err()
            .to_string()
            .contains("exhausted")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn keeps_each_sessions_cursor_independent_interleaved_calls() {
    let mut f = Files::new(&[chunks("p1"), chunks("p2")]);
    f.child("c", 200, &[chunks("c1"), chunks("c2")]);
    let (k, ctx, llm) = runtime().await;
    let h = install_llm_replay(&ctx, &f.config).unwrap();
    for (id, text) in [("A", "p1"), ("B", "c1"), ("A", "p2"), ("B", "c2")] {
        assert_eq!(
            drain(llm.stream(&ctx, options(Some(id)))).await.unwrap(),
            chunks(text)
        );
    }
    h.assert_consumed().unwrap();
    k.shutdown().await;
}
#[tokio::test]
async fn materializes_typed_session_tokens_after_matching_live_child_binds() {
    let mut f = Files::new(&[chunks("hi"), chunks("{{session:2}}")]);
    f.child("c", 200, &[chunks("child")]);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    drain(llm.stream(&ctx, options(Some("parent"))))
        .await
        .unwrap();
    drain(llm.stream(&ctx, options(Some("live-child"))))
        .await
        .unwrap();
    assert_eq!(
        drain(llm.stream(&ctx, options(Some("parent"))))
            .await
            .unwrap(),
        chunks("live-child")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_a_session_token_before_that_recorded_child_binds() {
    let mut f = Files::new(&[chunks("{{session:2}}")]);
    f.child("c", 200, &[chunks("child")]);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    assert!(
        drain(llm.stream(&ctx, options(Some("p"))))
            .await
            .unwrap_err()
            .to_string()
            .contains("was used before")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn treats_a_call_with_no_session_id_as_single_anonymous_primary() {
    let f = Files::new(&[chunks("hi")]);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    assert_eq!(
        drain(llm.stream(&ctx, options(None))).await.unwrap(),
        chunks("hi")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn fails_loud_when_more_distinct_live_sessions_call_than_recorded() {
    let f = Files::new(&[chunks("hi")]);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    drain(llm.stream(&ctx, options(Some("first"))))
        .await
        .unwrap();
    assert!(
        drain(llm.stream(&ctx, options(Some("second"))))
            .await
            .unwrap_err()
            .to_string()
            .contains("unrecorded session (#2)")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn fails_loud_when_the_script_is_exhausted() {
    let f = Files::new(&[chunks("hi")]);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    drain(llm.stream(&ctx, options(None))).await.unwrap();
    assert!(
        drain(llm.stream(&ctx, options(None)))
            .await
            .unwrap_err()
            .to_string()
            .contains("script exhausted")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn assert_consumed_passes_only_after_every_recorded_call_replayed() {
    let f = Files::new(&[chunks("one"), chunks("two")]);
    let (k, ctx, llm) = runtime().await;
    let h = install_llm_replay(&ctx, &f.config).unwrap();
    drain(llm.stream(&ctx, options(None))).await.unwrap();
    assert!(
        h.assert_consumed()
            .unwrap_err()
            .to_string()
            .contains("consumed 1/2")
    );
    drain(llm.stream(&ctx, options(None))).await.unwrap();
    h.assert_consumed().unwrap();
    k.shutdown().await;
}
#[tokio::test]
async fn assert_consumed_names_an_underrunning_identified_session() {
    let f = Files::new(&[chunks("one"), chunks("two")]);
    let (k, ctx, llm) = runtime().await;
    let h = install_llm_replay(&ctx, &f.config).unwrap();
    drain(llm.stream(&ctx, options(Some("live-underrun"))))
        .await
        .unwrap();
    assert!(
        h.assert_consumed()
            .unwrap_err()
            .to_string()
            .contains("session live-underrun consumed 1/2")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn assert_consumed_reports_scripts_no_live_session_ever_bound() {
    let mut f = Files::new(&[chunks("one")]);
    f.child("c", 200, &[chunks("child")]);
    let (k, ctx, llm) = runtime().await;
    let h = install_llm_replay(&ctx, &f.config).unwrap();
    drain(llm.stream(&ctx, options(None))).await.unwrap();
    assert!(
        h.assert_consumed()
            .unwrap_err()
            .to_string()
            .contains("1 recorded script(s) never bound")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn replays_a_sidecar_throw_as_llm_error_after_prefix_chunks() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"throw","chunks":[{"type":"block-start","index":0,"blockType":"text"}],"message":"unauthorized","code":"AUTH"}]));
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let mut s = llm.stream(&ctx, options(None));
    assert_eq!(s.next().await.unwrap().unwrap().kind(), "block-start");
    let e = s.next().await.unwrap().unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(e.as_ref()).unwrap().code(),
        "AUTH"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn replays_a_hang_entry_that_surfaces_abort_when_signal_fires() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"hang"}]));
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let c = AbortController::new();
    let mut o = options(None);
    o.signal = Some(c.signal());
    let mut s = llm.stream(&ctx, o);
    assert_eq!(s.next().await.unwrap().unwrap().kind(), "block-start");
    assert_eq!(s.next().await.unwrap().unwrap().kind(), "text-delta");
    c.abort();
    assert_eq!(s.next().await.unwrap().unwrap_err().to_string(), "aborted");
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_a_hang_entry_when_signal_fires_during_wait() {
    let mut f = Files::new(&[]);
    let ready = f.root.path().join("ready");
    f.override_doc(json!([{"kind":"hang","readyFile":ready}]));
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let c = AbortController::new();
    let mut o = options(None);
    o.signal = Some(c.signal());
    let mut s = llm.stream(&ctx, o);
    s.next().await.unwrap().unwrap();
    s.next().await.unwrap().unwrap();
    let pending = s.next();
    tokio::pin!(pending);
    loop {
        tokio::select! {biased;v=&mut pending=>panic!("premature hang end {v:?}"),_=tokio::task::yield_now()=>{if ready.exists(){break;}}}
    }
    c.abort();
    assert_eq!(pending.await.unwrap().unwrap_err().to_string(), "aborted");
    k.shutdown().await;
}
#[tokio::test]
async fn surfaces_an_already_aborted_signal_on_hang_before_waiting() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"hang"}]));
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let c = AbortController::new();
    c.abort();
    let mut o = options(None);
    o.signal = Some(c.signal());
    let mut s = llm.stream(&ctx, o);
    s.next().await.unwrap().unwrap();
    s.next().await.unwrap().unwrap();
    assert_eq!(s.next().await.unwrap().unwrap_err().to_string(), "aborted");
    k.shutdown().await;
}
#[tokio::test]
async fn aborts_mid_replay_when_signal_already_set() {
    let f = Files::new(&[chunks("hi")]);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let c = AbortController::new();
    c.abort();
    let mut o = options(None);
    o.signal = Some(c.signal());
    assert_eq!(
        drain(llm.stream(&ctx, o)).await.unwrap_err().to_string(),
        "aborted"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn aborts_mid_replay_of_a_throw_entry_prefix() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"throw","chunks":chunks("hi"),"message":"x","code":"X"}]));
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let c = AbortController::new();
    c.abort();
    let mut o = options(None);
    o.signal = Some(c.signal());
    assert_eq!(
        drain(llm.stream(&ctx, o)).await.unwrap_err().to_string(),
        "aborted"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_pace_ms_that_is_not_non_negative_integer() {
    let mut f = Files::new(&[]);
    let (k, ctx, _) = runtime().await;
    for pace in [-1.0, 1.5, f64::NAN, f64::INFINITY] {
        f.config.pace_ms = Some(pace);
        assert!(
            install_llm_replay(&ctx, &f.config)
                .err()
                .unwrap()
                .to_string()
                .contains("paceMs")
        );
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn paces_chunk_yields_when_pace_ms_set() {
    let mut f = Files::new(&[chunks("hi")]);
    f.config.pace_ms = Some(10.0);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let start = tokio::time::Instant::now();
    assert_eq!(
        drain(llm.stream(&ctx, options(None))).await.unwrap(),
        chunks("hi")
    );
    assert!(start.elapsed() >= std::time::Duration::from_millis(50));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn aborting_during_a_pace_wait_cancels_promptly() {
    let mut f = Files::new(&[chunks("hi")]);
    f.config.pace_ms = Some(60_000.0);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let c = AbortController::new();
    let mut o = options(None);
    o.signal = Some(c.signal());
    let mut s = llm.stream(&ctx, o);
    let pending = s.next();
    tokio::pin!(pending);
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
            pending.as_mut(),
            cx
        )))
        .await
        .is_pending()
    );
    c.abort();
    assert_eq!(pending.await.unwrap().unwrap_err().to_string(), "aborted");
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn paces_a_throw_entry_prefix_too() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"throw","chunks":[{"type":"block-start","index":0,"blockType":"text"}],"message":"boom","code":"STREAM_CLOSED"}]));
    f.config.pace_ms = Some(10.0);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let start = tokio::time::Instant::now();
    assert_eq!(
        drain(llm.stream(&ctx, options(None)))
            .await
            .unwrap_err()
            .to_string(),
        "boom"
    );
    assert!(start.elapsed() >= std::time::Duration::from_millis(10));
    k.shutdown().await;
}
#[test]
fn exposes_namespace_plugin_shape() {
    use hoh_kernel::{Inject, Plugin};
    assert_eq!(LlmReplay::NAME, "llm-replay");
    assert_eq!(NAME, "llm-replay");
    assert_eq!(INJECT, ["llm"]);
    assert_eq!(Deps::deps()[0].name, "llm");
}
#[test]
fn config_environment_defaults_and_explicit_overrides() {
    let env = |k: &str| match k {
        "DSH_SNAPSHOT_FILE" => Some("env.jsonl".into()),
        "DSH_SNAPSHOT_OVERRIDE" => Some("env.json".into()),
        "DSH_SNAPSHOT_CHILD_FILES" => Some(if cfg!(windows) { "a;b" } else { "a:b" }.into()),
        _ => None,
    };
    let resolved = Config::default().resolve_with_env(env).unwrap();
    assert_eq!(resolved.file, "env.jsonl");
    assert_eq!(resolved.override_file.as_deref(), Some("env.json"));
    assert_eq!(resolved.child_files.unwrap(), ["a", "b"]);
    let resolved = Config {
        file: Some("explicit".into()),
        override_file: Some("".into()),
        child_files: Some(vec![]),
        ..Default::default()
    }
    .resolve_with_env(env)
    .unwrap();
    assert_eq!(resolved.file, "explicit");
    assert!(resolved.override_file.is_none());
    assert!(resolved.child_files.is_none());
}
#[test]
fn throws_when_no_fixture_path_given_by_config_or_env() {
    assert!(
        Config::default()
            .resolve_with_env(|_| None)
            .unwrap_err()
            .to_string()
            .contains("fixture path is required")
    );
}
#[test]
fn treats_empty_string_fixture_path_as_missing() {
    assert!(
        Config {
            file: Some("".into()),
            ..Default::default()
        }
        .resolve_with_env(|_| Some("env".into()))
        .is_err()
    );
}
#[test]
fn ignores_empty_child_files_env() {
    let c = Config {
        file: Some("f".into()),
        ..Default::default()
    }
    .resolve_with_env(|_| Some("".into()))
    .unwrap();
    assert!(c.child_files.is_none());
    assert!(c.override_file.is_none());
}
#[test]
fn rejects_invalid_model_capabilities_during_load() {
    for (field, value, reason) in [
        (
            "inputModalities",
            json!("image"),
            "inputModalities must be an array",
        ),
        (
            "inputModalities",
            json!(["audio"]),
            "inputModalities must be an array",
        ),
        ("imageRequestTokens", json!(0), "positive safe integer"),
        ("imageRequestTokens", json!(1.5), "positive safe integer"),
        ("imageRequestTokens", json!(384), "requires inputModalities"),
        (
            "systemPromptUpdate",
            json!("leading"),
            "systemPromptUpdate must be",
        ),
        ("toolUpdate", json!("always"), "toolUpdate must be"),
        ("badge", json!("1017"), "badge needs non-blank text"),
        (
            "badge",
            json!({"text":" ","tone":"info"}),
            "badge needs non-blank text",
        ),
        (
            "badge",
            json!({"text":{"zh":"新"},"tone":"info"}),
            "badge needs non-blank text",
        ),
        (
            "badge",
            json!({"text":{"en":"New","zh":""},"tone":"info"}),
            "badge needs non-blank text",
        ),
        (
            "badge",
            json!({"text":"New","tone":"accent"}),
            "badge needs non-blank text",
        ),
    ] {
        let mut model = json!({"id":"m"});
        model[field] = value;
        let config = Config {
            file: Some("f".into()),
            providers: Some(json!([{"id":"m","models":[model]}])),
            ..Default::default()
        };
        assert!(
            config
                .resolve_with_env(|_| None)
                .unwrap_err()
                .to_string()
                .contains(reason),
            "{field}"
        );
    }
}
#[tokio::test]
async fn registers_a_replay_only_provider_catalog_when_configured() {
    let mut f = Files::new(&[chunks("hi")]);
    f.config.providers=Some(serde_json::from_value(json!([
    {"id":"deepseek","name":"DeepSeek","retryPolicy":{"mode":"normal","maxRetries":2,"backoff":{"initialDelayMs":1,"maxDelayMs":1,"jitterRatio":0}},"models":[
        {"id":"flash","contextWindow":128000,"inputModalities":["text","image"],"defaultMaxTokens":64000,"reasoningEfforts":["off","max"],"defaultReasoningEffort":"max","systemPromptUpdate":"in-history","toolUpdate":"addition-only"},
        {"id":"pro","name":"Pro","description":"Larger model","reasoningEfforts":["high"],"badge":{"text":{"en":"New","zh":"新"},"tone":"info"}}
    ]},{"id":"empty"}])).unwrap());
    let (k, ctx, llm) = runtime().await;
    let h = install_llm_replay(&ctx, &f.config).unwrap();
    assert_eq!(llm.list_providers()[0].name, "DeepSeek");
    let models = llm.list_models("deepseek").await.unwrap();
    assert_eq!(models.len(), 2);
    assert!(models[1].badge.is_some());
    assert!(llm.list_models("empty").await.unwrap().is_empty());
    let model = llm
        .resolve_model_info("deepseek", "flash", None)
        .await
        .unwrap();
    assert_eq!(model.context.unwrap().context_window, 128000);
    assert_eq!(model.default_max_tokens, Some(64000));
    assert_eq!(model.reasoning.unwrap().efforts[0].id.as_str(), "off");
    assert_eq!(model.tool_update.as_deref(), Some("addition-only"));
    for (p, m) in [
        ("deepseek", "pro"),
        ("deepseek", "unlisted"),
        ("empty", "unlisted"),
    ] {
        assert!(
            llm.resolve_model_info(p, m, None)
                .await
                .unwrap()
                .context
                .is_none()
        );
    }
    assert_eq!(
        serde_json::to_value(llm.provider_retry_policy("deepseek").unwrap()).unwrap()["maxRetries"],
        2
    );
    let config = serde_json::from_value(json!({"provider":"deepseek","model":"flash"})).unwrap();
    let prepared = llm.prepare_call(&ctx, config, None, None).await.unwrap();
    assert_eq!(
        drain(
            prepared
                .stream(options(None).with_call_config(&prepared.config))
                .unwrap()
        )
        .await
        .unwrap(),
        chunks("hi")
    );
    h.dispose();
    assert!(llm.list_providers().is_empty());
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_invalid_replay_provider_retry_policy_during_registration() {
    let mut f = Files::new(&[]);
    f.config.providers = Some(
        serde_json::from_value(
            json!([{"id":"deepseek","retryPolicy":{"mode":"normal","maxRetries":-1}}]),
        )
        .unwrap(),
    );
    let (k, ctx, _) = runtime().await;
    assert!(
        install_llm_replay(&ctx, &f.config)
            .err()
            .unwrap()
            .to_string()
            .contains("retryPolicy.maxRetries")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn declares_flat_image_request_pricing_only_for_configured_models() {
    let mut f = Files::new(&[]);
    f.config.providers=Some(serde_json::from_value(json!([{"id":"deepseek","models":[{"id":"vision","inputModalities":["image"],"imageRequestTokens":384},{"id":"plain"}]}])).unwrap());
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let image:ImageBlock=serde_json::from_value(json!({"type":"image","attachment":{"attachmentId":"sha256:aaaaaaaa","mediaType":"image/png","bytes":10,"width":640,"height":480}})).unwrap();
    let pricing = llm.image_request_pricing("deepseek", "vision").unwrap();
    let mut offloaded = image.clone();
    offloaded.offloaded = Some(true);
    let prices = pricing.price_images(&[image, offloaded]);
    assert_eq!(
        prices.iter().map(|p| p.visual_tokens).collect::<Vec<_>>(),
        [384, 0]
    );
    assert!(prices[0].text.contains("640x480px"));
    assert!(
        prices[1]
            .text
            .contains("image omitted to fit request image limits")
    );
    assert!(llm.image_request_pricing("deepseek", "plain").is_none());
    k.shutdown().await;
}
#[tokio::test]
async fn installs_replay_and_catalog_from_explicit_plugin_config() {
    let f = Files::new(&[chunks("hi")]);
    let (k, ctx, llm) = runtime().await;
    let fiber=ctx.plugin::<LlmReplay>(json!({"file":f.config.file,"providers":[{"id":"m","models":[{"id":"m","inputModalities":["image"],"reasoningEfforts":["low"]}]},{"id":"empty"}],"paceMs":1})).unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(llm.list_providers().len(), 2);
    assert_eq!(
        drain(llm.stream(&ctx, options(None))).await.unwrap(),
        chunks("hi")
    );
    fiber.dispose().await;
    assert!(llm.list_providers().is_empty());
    k.shutdown().await;
}
#[tokio::test]
async fn loads_child_fixtures_from_plugin_config() {
    let mut f = Files::new(&[chunks("parent")]);
    let child = f.child("c", 200, &[chunks("child")]);
    let (k, ctx, llm) = runtime().await;
    let fiber = ctx
        .plugin::<LlmReplay>(json!({"file":f.config.file,"childFiles":[child]}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(
        drain(llm.stream(&ctx, options(Some("A")))).await.unwrap(),
        chunks("parent")
    );
    assert_eq!(
        drain(llm.stream(&ctx, options(Some("B")))).await.unwrap(),
        chunks("child")
    );
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_malformed_sidecar_kind_before_installing_replay() {
    let mut f = Files::new(&[]);
    f.override_doc(json!([{"kind":"bogus"}]));
    let (k, ctx, llm) = runtime().await;
    assert!(
        install_llm_replay(&ctx, &f.config)
            .err()
            .unwrap()
            .to_string()
            .contains("unknown kind")
    );
    assert!(llm.list_providers().is_empty());
    k.shutdown().await;
}
#[tokio::test]
async fn accepts_anonymous_official_extensions_with_registry() {
    use hoh_deepseek_llm_api_extensions::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let f = Files::new(&[chunks("hi")]);
    let (k, ctx, llm) = runtime().await;
    ctx.plugin::<DeepSeekLlmApiExtensionRegistry>(Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let accepted = count.clone();
    ctx.get::<DeepSeekLlmApiExtensions>()
        .unwrap()
        .register(
            &ctx,
            "test_replay",
            move |request: Arc<DeepSeekLlmApiExtensionRequest>| {
                assert!(request.session_id.is_none());
                let accepted = accepted.clone();
                async move {
                    let accept: AcceptanceCallback = Arc::new(move || {
                        accepted.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async { Ok(()) })
                    });
                    Ok(Some(PreparedDeepSeekLlmApiExtension {
                        value: json!(1),
                        accept: Some(accept),
                    }))
                }
            },
        )
        .unwrap();
    install_llm_replay(&ctx, &f.config).unwrap();
    let mut o = options(None);
    o.provider = "deepseek-official".into();
    drain(llm.stream(&ctx, o)).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    k.shutdown().await;
}
#[tokio::test]
async fn removes_waterfall_listener_when_owning_fiber_disposed() {
    struct Fallthrough;
    impl LlmAdapter for Fallthrough {
        fn stream(&self, _: GenerateOptions) -> hoh_llm_replay::Result<LlmStream> {
            Ok(LlmStream::chunks([StreamChunk::finish(FinishReason::new(
                "stop",
            ))]))
        }
    }
    let f = Files::new(&[chunks("hi"), chunks("hi")]);
    let (k, ctx, llm) = runtime().await;
    llm.register_adapter(&ctx, vec!["m".into()], std::sync::Arc::new(Fallthrough))
        .unwrap();
    let fiber = ctx
        .plugin::<LlmReplay>(json!({"file":f.config.file}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(
        drain(llm.stream(&ctx, options(None))).await.unwrap(),
        chunks("hi")
    );
    fiber.dispose().await;
    assert_eq!(
        drain(llm.stream(&ctx, options(None))).await.unwrap(),
        vec![StreamChunk::finish(FinishReason::new("stop"))]
    );
    k.shutdown().await;
}
#[tokio::test]
async fn settles_official_deepseek_request_extensions_before_chunks() {
    use hoh_deepseek_llm_api_extensions::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let f = Files::new(&[chunks("hi"), chunks("two"), chunks("three")]);
    let (k, ctx, llm) = runtime().await;
    ctx.plugin::<DeepSeekLlmApiExtensionRegistry>(Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    let requests = Arc::new(std::sync::Mutex::new(vec![]));
    let seen = requests.clone();
    ctx.get::<DeepSeekLlmApiExtensions>()
        .unwrap()
        .register(
            &ctx,
            "test_replay",
            move |request: Arc<DeepSeekLlmApiExtensionRequest>| {
                let count = count.clone();
                seen.lock().unwrap().push((
                    request.body.clone(),
                    request.session_id.clone(),
                    request.purpose.clone(),
                ));
                async move {
                    let accept: AcceptanceCallback = Arc::new(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async { Ok(()) })
                    });
                    Ok(Some(PreparedDeepSeekLlmApiExtension {
                        value: json!({"version":1}),
                        accept: Some(accept),
                    }))
                }
            },
        )
        .unwrap();
    install_llm_replay(&ctx, &f.config).unwrap();
    let mut o = options(Some("session"));
    o.provider = "deepseek-official".into();
    let mut stream = llm.stream(&ctx, o.clone());
    stream.next().await.unwrap().unwrap();
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    drain(stream).await.unwrap();
    o.purpose = Some("compaction".into());
    drain(llm.stream(&ctx, o.clone())).await.unwrap();
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
    o.provider = "other".into();
    drain(llm.stream(&ctx, o)).await.unwrap();
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
    {
        let requests = requests.lock().unwrap();
        assert_eq!(
            requests[0].0,
            json!({"messages":[]}).as_object().unwrap().clone()
        );
        assert_eq!(requests[0].1.as_deref(), Some("session"));
        assert_eq!(requests[1].2.as_deref(), Some("compaction"));
    }
    k.shutdown().await;
}
#[tokio::test]
async fn accepts_only_throw_entries_that_reached_post_2xx_point() {
    use hoh_deepseek_llm_api_extensions::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut f = Files::new(&[]);
    f.override_doc(json!([
    {"kind":"throw","chunks":[{"type":"block-start","index":0,"blockType":"text"}],"message":"partial","code":"STREAM_CLOSED"},
    {"kind":"throw","chunks":[],"message":"unauthorized","code":"AUTH"},
    {"kind":"throw","chunks":[],"message":"empty body","code":"EMPTY_RESPONSE","accepted":true}
    ]));
    let (k, ctx, llm) = runtime().await;
    ctx.plugin::<DeepSeekLlmApiExtensionRegistry>(Value::Null)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    ctx.get::<DeepSeekLlmApiExtensions>()
        .unwrap()
        .register(
            &ctx,
            "test_replay",
            move |_: Arc<DeepSeekLlmApiExtensionRequest>| {
                let count = count.clone();
                async move {
                    let accept: AcceptanceCallback = Arc::new(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async { Ok(()) })
                    });
                    Ok(Some(PreparedDeepSeekLlmApiExtension {
                        value: json!(1),
                        accept: Some(accept),
                    }))
                }
            },
        )
        .unwrap();
    install_llm_replay(&ctx, &f.config).unwrap();
    let mut o = options(None);
    o.provider = "deepseek-official".into();
    for (expected, message) in [(1, "partial"), (1, "unauthorized"), (2, "empty body")] {
        assert_eq!(
            drain(llm.stream(&ctx, o.clone()))
                .await
                .unwrap_err()
                .to_string(),
            message
        );
        assert_eq!(accepted.load(Ordering::SeqCst), expected);
    }
    k.shutdown().await;
}
#[tokio::test]
async fn accepts_anonymous_official_extensions_and_tolerates_absent_registry() {
    let f = Files::new(&[chunks("hi")]);
    let (k, ctx, llm) = runtime().await;
    install_llm_replay(&ctx, &f.config).unwrap();
    let mut o = options(None);
    o.provider = "deepseek-official".into();
    assert_eq!(drain(llm.stream(&ctx, o)).await.unwrap(), chunks("hi"));
    k.shutdown().await;
}
