mod common;
use common::*;
use hoh_session::surface::SurfaceManager;
use hoh_session::*;
use serde_json::json;
use std::sync::Arc;
fn replaced(role: &str, n: u64, start: u64, end: u64, sources: &[u64]) -> Arc<SessionEvent> {
    let e = surface_event(role, n, "replacement");
    let e = patch(
        &e,
        "surfaceOp",
        json!({"op":"replace","startSeq":start,"endSeq":end}),
    );
    patch(&e, "sourceEventSeqs", json!(sources))
}
fn fixture() -> Session {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    user(&s, "hello");
    s.append(
        "assistant/message",
        data("assistant", "hi"),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    s
}
#[test]
fn accepts_absent_or_valid_source_event_references_and_complete_replacement_coverage() {
    let f = fold_surface(
        &[
            surface_event("user", 0, "a"),
            surface_event("user", 1, "b"),
            replaced("user", 2, 0, 1, &[0, 1]),
        ],
        &[],
    )
    .unwrap();
    assert_eq!(f.nodes, vec![seq(2)]);
}
#[test]
fn rejects_source_event_references_on_a_non_surface_event() {
    let e = event("turn/start", 0, json!({"turn":1}));
    error(
        fold_surface(&[patch(&e, "sourceEventSeqs", json!([0]))], &[]),
        "cannot carry sourceEventSeqs",
    );
}
#[test]
fn rejects_obsolete_assistant_chunk_references_on_an_assistant_message() {
    error(
        fold_surface(
            &[patch(
                &surface_event("assistant", 0, "a"),
                "sourceEventSeqs",
                json!([]),
            )],
            &[],
        ),
        "embeds its source stream",
    );
}
#[test]
fn rejects_invalid_source_event_references() {
    for (raw, expected) in [
        (json!("invalid"), "must be an array"),
        (json!([]), "must not be empty"),
        (json!([0, 0]), "must not contain duplicates"),
        (json!(["0"]), "safe integers"),
        (json!([0.5]), "safe integers"),
        (json!([-1]), "safe integers"),
        (json!([1]), "earlier events"),
    ] {
        let e = patch(&surface_event("user", 1, "b"), "sourceEventSeqs", raw);
        error(
            fold_surface(&[surface_event("user", 0, "a"), e], &[]),
            expected,
        );
    }
    error(
        fold_surface(
            &[surface_event("user", 0, "a"), surface_event("user", 2, "b")],
            &[],
        ),
        "seq 2 is not contiguous; expected 1",
    );
    error(
        fold_surface(
            &[
                surface_event("user", 0, "a"),
                surface_event("user", 1, "b"),
                replaced("user", 2, 0, 1, &[0]),
            ],
            &[],
        ),
        "missing 1",
    );
}
#[test]
fn rejects_a_replacement_spanning_multiple_current_nodes() {
    error(
        fold_surface(
            &[
                surface_event("user", 0, "a"),
                surface_event("user", 1, "b"),
                replaced("tool", 2, 0, 1, &[0, 1]),
            ],
            &[],
        ),
        "exactly one current node",
    );
}
#[test]
fn rejects_a_replacement_targeting_a_non_result_node() {
    error(
        fold_surface(
            &[
                surface_event("user", 0, "a"),
                replaced("tool", 1, 0, 0, &[0]),
            ],
            &[],
        ),
        "must target a current tool/result",
    );
}
#[test]
fn rejects_changes_outside_tool_result_content() {
    let a = surface_event("tool", 0, "a");
    let b = replaced("tool", 1, 0, 0, &[0]);
    error(fold_surface(&[a, b], &[]), "may change only content");
}
#[test]
fn rejects_a_replacement_that_changes_the_result_block() {
    for key in ["toolCallId", "isError"] {
        let a = surface_event("tool", 0, "a");
        let mut d = a.data().clone();
        d["message"][key] = if key == "isError" {
            json!(true)
        } else {
            json!("changed")
        };
        let b = replaced("tool", 1, 0, 0, &[0]);
        let b = patch(&b, "data", d);
        error(fold_surface(&[a, b], &[]), "may change only content");
    }
}
#[test]
fn compares_array_valued_rest_fields_structurally() {
    for (a, b, valid) in [
        (
            json!({"tags":["a",{"n":1}]}),
            json!({"tags":["a",{"n":1}]}),
            true,
        ),
        (json!({"tags":["a"]}), json!({"tags":["b"]}), false),
        (json!({"tags":["a"]}), json!({"tags":"a"}), false),
        (json!({"left":1}), json!({"right":1}), false),
        (json!({"one":1}), json!({"one":1,"two":2}), false),
    ] {
        let original = surface_event("tool", 0, "result");
        let mut d = original.data().clone();
        d["meta"] = a;
        let original = patch(&original, "data", d.clone());
        d["meta"] = b;
        d["message"]["content"] = json!([{"type":"text","text":"new"}]);
        let replacement = patch(&replaced("tool", 1, 0, 0, &[0]), "data", d);
        let result = fold_surface(&[original, replacement], &[]);
        if valid {
            assert_eq!(result.unwrap().nodes, vec![seq(1)]);
        } else {
            error(result, "may change only content");
        }
    }
}
#[test]
fn folds_a_contiguous_window_without_materializing_earlier_event_sequences() {
    let m = SurfaceManager::new(
        vec![
            surface_event("user", 400000, "a"),
            surface_event("user", 400001, "b"),
            replaced("user", 400002, 400000, 400000, &[400000]),
        ],
        offset(400000),
        Default::default(),
    )
    .unwrap();
    let s = m.surface().unwrap();
    assert_eq!(s.nodes, vec![seq(400002), seq(400001)]);
    assert_eq!(s.replace_generation, 1);
}
#[test]
fn validates_tool_result_rewrites_against_a_nonzero_window_offset() {
    let a = surface_event("tool", 400000, "a");
    let b = patch(
        &replaced("tool", 400001, 400000, 400000, &[400000]),
        "data",
        a.data().clone(),
    );
    let m = SurfaceManager::new(vec![a, b], offset(400000), Default::default()).unwrap();
    assert_eq!(m.surface().unwrap().nodes, vec![seq(400001)]);
}
#[test]
fn rejects_a_replacement_that_crosses_a_loaded_window_head() {
    error(
        SurfaceManager::new(
            vec![
                surface_event("user", 400000, "a"),
                replaced("user", 400001, 399999, 400000, &[399999, 400000]),
            ],
            offset(400000),
            Default::default(),
        ),
        "start seq 399999 not found",
    );
}
#[test]
fn shares_ordered_entries_and_nested_replacement_ranges_with_fold_surface() {
    let s = session();
    user(&s, "a");
    user(&s, "b");
    replace(&s, 0, 0, &[0], "summary").unwrap();
    replace(&s, 2, 1, &[2, 1], "summary 2").unwrap();
    let mut f = fold_surface(&s.snapshot_events(None, None), &[]).unwrap();
    assert_eq!(f.nodes, s.surface().unwrap().nodes().unwrap());
    assert_eq!(f.replacements[0].shadowed_seqs, vec![seq(0)]);
    assert_eq!(f.replacements[1].shadowed_seqs, vec![seq(2), seq(1)]);
    f.nodes[0] = seq(99);
    assert_eq!(nodes(&s), vec![3]);
}
#[test]
fn does_not_retain_fold_only_replacement_history_in_incremental_state() {
    let s = session();
    user(&s, "a");
    replace(&s, 0, 0, &[0], "b").unwrap();
    assert_eq!(nodes(&s), vec![1]);
    assert_eq!(
        fold_surface(&s.snapshot_events(None, None), &[])
            .unwrap()
            .replacements
            .len(),
        1
    );
}
#[test]
fn fold_surface_reports_the_same_invalid_replacement_failures_as_the_incremental_manager() {
    let events = vec![
        surface_event("user", 0, "a"),
        event("turn/start", 1, json!({"turn":1})),
        replaced("user", 2, 1, 0, &[0]),
    ];
    error(fold_surface(&events, &[]), "start seq 1 not found");
    error(
        Session::create("test".into(), Some(&events), None, None, None),
        "start seq 1 not found",
    );
}
#[test]
fn rejects_negative_zero_replacement_event_sequences() {
    let e = patch(
        &surface_event("user", 0, "a"),
        "surfaceOp",
        json!({"op":"replace","startSeq":-0.0,"endSeq":-0.0}),
    );
    error(fold_surface(&[e], &[]), "invalid replace surfaceOp");
}
#[test]
fn leaves_incremental_state_unchanged_when_candidate_validation_fails() {
    let s = session();
    user(&s, "a");
    let before = s.snapshot_events(None, None);
    error(
        s.append(
            "assistant/message",
            data("assistant", "invalid"),
            Some(SurfaceIntent {
                surface_op: Some(SurfaceOp::replace(seq(0), seq(0))),
                source_event_seqs: None,
            }),
        ),
        "missing 0",
    );
    assert!(Arc::ptr_eq(&before, &s.snapshot_events(None, None)));
    assert_eq!(nodes(&s), vec![0]);
    user(&s, "b");
    assert_eq!(nodes(&s), vec![0, 1]);
    assert_eq!(s.surface().unwrap().replace_generation().unwrap(), 0);
}
#[test]
fn fold_surface_rejects_a_surface_eligible_event_without_its_mandatory_marker() {
    let mut e = surface_event("user", 0, "a").as_value().clone();
    e.as_object_mut().unwrap().shift_remove("surfaceOp");
    error(
        fold_surface(&[Arc::new(SessionEvent::from_value(e))], &[]),
        "requires a surfaceOp marker",
    );
}
#[test]
fn fold_surface_rejects_surface_op_on_a_non_surface_event() {
    error(
        fold_surface(
            &[patch(
                &event("turn/start", 0, json!({"turn":1})),
                "surfaceOp",
                json!("append"),
            )],
            &[],
        ),
        "cannot carry surfaceOp",
    );
}
#[test]
fn folds_an_ordered_sequence_list_from_surface_op_append_markers() {
    assert_eq!(nodes(&fixture()), vec![1, 2]);
}
#[test]
fn empty_surface_yields_empty_nodes() {
    let s = session();
    for t in ["turn/start", "step/start", "step/end", "turn/end"] {
        s.append(
            t,
            json!({"turn":1,"step":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    }
    assert!(nodes(&s).is_empty());
    assert!(s.derive_messages().unwrap().is_empty());
}
#[test]
fn picks_up_new_events_incrementally() {
    let s = fixture();
    s.append(
        "tool/result",
        data("tool", "ok"),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    assert_eq!(nodes(&s), vec![1, 2, 4]);
}
#[test]
fn replays_identically_from_a_seeded_log_with_surface_markers() {
    let s = fixture();
    s.append(
        "tool/result",
        data("tool", "ok"),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    let replay = Session::create(
        "replay".into(),
        Some(&s.snapshot_events(None, None)),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(nodes(&replay), vec![1, 2, 4]);
    assert_eq!(
        replay.derive_messages().unwrap(),
        s.derive_messages().unwrap()
    );
}
#[test]
fn rebuild_with_replace_operation_splices_out_shadowed_nodes() {
    let s = fixture();
    replace(&s, 1, 2, &[1, 2], "summary").unwrap();
    assert_eq!(nodes(&s), vec![4]);
}
#[test]
fn replace_with_both_ends_at_real_nodes_splices_only_the_range() {
    let s = session();
    for text in ["a", "b", "c"] {
        user(&s, text);
    }
    replace(&s, 0, 1, &[0, 1], "summary").unwrap();
    assert_eq!(nodes(&s), vec![3, 2]);
}
#[test]
fn single_node_replacement() {
    let s = session();
    user(&s, "a");
    user(&s, "b");
    replace(&s, 1, 1, &[1], "x").unwrap();
    assert_eq!(nodes(&s), vec![0, 2]);
}
#[test]
fn throws_when_replace_start_is_not_found() {
    let s = session();
    user(&s, "a");
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    error(replace(&s, 1, 0, &[0], "y"), "start seq 1 not found");
}
#[test]
fn throws_when_replace_end_is_not_found() {
    let s = session();
    user(&s, "a");
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    error(replace(&s, 0, 1, &[0], "y"), "end seq 1 not found");
}
#[test]
fn throws_when_start_is_after_end() {
    let s = session();
    user(&s, "a");
    user(&s, "b");
    error(replace(&s, 1, 0, &[1, 0], "y"), "is after end seq 0");
}
#[test]
fn source_event_seqs_is_snapshot_so_caller_mutation_does_not_affect_logged_event() {
    let s = session();
    user(&s, "a");
    let mut options = json!({"surfaceOp":"append","sourceEventSeqs":[0]});
    let e = s
        .append_batch(&[
            SessionEventInput::new("user/message", data("user", "b")).with_options(options.clone())
        ])
        .unwrap();
    options["sourceEventSeqs"][0] = json!(99);
    assert_eq!(e[0].source_event_seqs(), Some(&json!([0])));
}
#[test]
fn replace_starting_at_non_head_position_preserves_surrounding_order() {
    let s = session();
    for t in ["a", "b", "c"] {
        user(&s, t);
    }
    replace(&s, 1, 1, &[1], "x").unwrap();
    assert_eq!(nodes(&s), vec![0, 3, 2]);
}
#[test]
fn surface_op_replace_object_is_snapshot_so_caller_mutation_is_isolated() {
    let s = session();
    user(&s, "a");
    let mut opts =
        json!({"surfaceOp":{"op":"replace","startSeq":0,"endSeq":0},"sourceEventSeqs":[0]});
    let e = s
        .append_batch(&[
            SessionEventInput::new("user/message", data("user", "b")).with_options(opts.clone())
        ])
        .unwrap();
    opts["surfaceOp"]["startSeq"] = json!(99);
    assert_eq!(e[0].surface_op().unwrap()["startSeq"], 0);
}
#[test]
fn uses_the_surface_path_when_surface_markers_are_present() {
    assert_eq!(texts(&fixture()), vec!["hello", "hi"]);
}
#[test]
fn surface_path_skips_non_surface_events() {
    let s = fixture();
    s.append(
        "assistant/attempt",
        json!({"turn":1,"step":1,"stream":[]}),
        None,
    )
    .unwrap();
    assert_eq!(texts(&s), vec!["hello", "hi"]);
}
#[test]
fn derive_messages_via_surface_respects_replace() {
    let s = session();
    user(&s, "original");
    replace(&s, 0, 0, &[0], "compacted").unwrap();
    assert_eq!(texts(&s), vec!["compacted"]);
}
#[test]
fn injected_context_and_user_messages_appear_on_surface() {
    let s = session();
    let mut d = data("user", "file changed");
    d["source"] = json!({"kind":"watcher"});
    s.append("user/message", d, Some(SurfaceIntent::append()))
        .unwrap();
    user(&s, "focus");
    assert_eq!(texts(&s), vec!["file changed", "focus"]);
}
#[test]
fn records_source_event_seqs_and_surface_op_on_a_source_derived_event() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    s.append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    let e = s
        .append(
            "user/message",
            data("user", "h"),
            Some(SurfaceIntent {
                source_event_seqs: Some(vec![seq(0), seq(1)]),
                surface_op: Some(SurfaceOp::append()),
            }),
        )
        .unwrap();
    assert_eq!(e.source_event_seqs(), Some(&json!([0, 1])));
    assert_eq!(e.surface_op(), Some(&json!("append")));
}
#[test]
fn derive_messages_skips_a_surface_node_that_derives_to_null() {
    let s = session();
    s.append(
        "assistant/message",
        data("assistant", ""),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    assert_eq!(nodes(&s), vec![0]);
    assert!(s.derive_messages().unwrap().is_empty());
}
#[test]
fn a_non_surface_event_carries_no_surface_fields() {
    let s = session();
    let e = s.append("turn/start", json!({"turn":1}), None).unwrap();
    assert!(e.surface_op().is_none());
    assert!(e.source_event_seqs().is_none());
}
#[test]
fn surface_op_primitives_are_not_cloned() {
    let s = session();
    assert_eq!(user(&s, "a").surface_op(), Some(&json!("append")));
}
#[test]
fn is_surface_event_rejects_a_surface_eligible_type_missing_its_marker() {
    let e = event("user/message", 0, data("user", "hi"));
    assert!(!is_surface_event(&e));
    assert!(!is_surface_event(&event(
        "turn/start",
        1,
        json!({"turn":1})
    )));
    assert!(is_surface_event(&surface_event("user", 0, "hi")));
}
#[test]
fn type_guards_is_surface_event_rejects_a_surface_eligible_type_missing_its_marker() {
    let e = event("user/message", 0, data("user", "hi"));
    assert!(is_surface_eligible_type(e.event_type()));
    assert!(!is_surface_event(&e));
}
#[test]
fn is_surface_eligible_type_is_true_only_for_message_producing_types() {
    for t in [
        "user/message",
        "assistant/message",
        "tool/result",
        "developer/message",
        "system/message",
    ] {
        assert!(is_surface_eligible_type(t));
    }
    for t in ["turn/start", "assistant/attempt"] {
        assert!(!is_surface_eligible_type(t));
    }
}
#[test]
fn is_surface_event_narrows_a_fully_formed_surface_event() {
    assert!(is_surface_event(&surface_event("user", 0, "hello")));
}
#[test]
fn is_surface_event_rejects_a_non_surface_eligible_type() {
    assert!(!is_surface_event(&event(
        "turn/start",
        0,
        json!({"turn":1})
    )));
}
#[test]
fn splits_surface_events_into_append_origin_and_replacement_by_their_marker() {
    let a = surface_event("user", 0, "a");
    let b = replaced("user", 1, 0, 0, &[0]);
    assert!(is_append_surface_event(&a));
    assert!(!is_replacement_surface_event(&a));
    assert!(!is_append_surface_event(&b));
    assert!(is_replacement_surface_event(&b));
}
#[test]
fn rejects_log_only_and_markerless_events_from_both_marker_guards() {
    for e in [
        event("turn/start", 0, json!({})),
        event("user/message", 0, data("user", "hi")),
    ] {
        assert!(!is_append_surface_event(&e));
        assert!(!is_replacement_surface_event(&e));
    }
}
#[test]
fn folds_the_pending_log_delta_on_access_and_counts_replaces() {
    let s = fixture();
    assert_eq!(s.surface().unwrap().replace_generation().unwrap(), 0);
    replace(&s, 1, 2, &[1, 2], "summary").unwrap();
    assert_eq!(s.surface().unwrap().replace_generation().unwrap(), 1);
}
#[test]
fn projects_a_system_node_as_the_leading_system_role_message_and_an_empty_one_as_no_message() {
    let s = session();
    let head = s
        .append(
            "system/message",
            data("system", "be brief"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    user(&s, "hi");
    assert_eq!(s.derive_messages().unwrap()[0]["role"], "system");
    s.append(
        "system/message",
        data("system", ""),
        Some(SurfaceIntent::replace(
            head.seq(),
            head.seq(),
            vec![head.seq()],
        )),
    )
    .unwrap();
    assert_eq!(texts(&s), vec!["hi"]);
    assert_eq!(nodes(&s).len(), 2);
}
#[test]
fn replaces_node_zero_with_a_new_system_node_and_rejects_other_rewrites_of_the_head() {
    let base = vec![
        surface_event("system", 0, "v1"),
        surface_event("user", 1, "u"),
    ];
    let mut ok = base.clone();
    ok.push(replaced("system", 2, 0, 0, &[0]));
    assert_eq!(fold_surface(&ok, &[]).unwrap().nodes, vec![seq(2), seq(1)]);
    for role in ["user", "system"] {
        let mut bad = base.clone();
        bad.push(replaced(role, 2, 0, 1, &[0, 1]));
        error(fold_surface(&bad, &[]), "node 0 holds the system prompt");
    }
}
#[test]
fn leaves_later_system_nodes_and_a_non_system_head_unprotected() {
    let f = fold_surface(
        &[
            surface_event("system", 0, "v1"),
            surface_event("user", 1, "u"),
            surface_event("system", 2, "v2"),
            surface_event("user", 3, "u"),
            replaced("user", 4, 1, 3, &[1, 2, 3]),
        ],
        &[],
    )
    .unwrap();
    assert_eq!(f.nodes, vec![seq(0), seq(4)]);
    assert_eq!(
        fold_surface(
            &[
                surface_event("user", 0, "a"),
                surface_event("user", 1, "b"),
                replaced("user", 2, 0, 1, &[0, 1])
            ],
            &[]
        )
        .unwrap()
        .nodes,
        vec![seq(2)]
    );
}
#[test]
fn rejects_a_seeded_system_message_with_a_non_system_role_or_non_system_prompt_source() {
    for (field, value, expected) in [
        ("role", json!("user"), "role \"system\""),
        ("source", json!({"kind":"user"}), "system-prompt source"),
    ] {
        let e = surface_event("system", 0, "v1");
        let mut d = e.data().clone();
        d["message"][field] = value;
        error(
            Session::create(
                "test".into(),
                Some(&[patch(&e, "data", d)]),
                None,
                None,
                None,
            ),
            expected,
        );
    }
}
#[test]
fn rejects_a_mismatched_developer_role_at_event_admission() {
    for role in ["user", "developer"] {
        let mut d = data(role, "");
        if role == "user" {
            d["role"] = json!("developer");
        } else {
            d["message"]["role"] = json!("user");
        }
        error(
            adopt_session_event((*patch(&surface_event(role, 0, ""), "data", d)).clone()),
            "must occur together",
        );
    }
}
#[test]
fn retains_an_empty_developer_node_while_projecting_no_message() {
    let s = session();
    s.append(
        "developer/message",
        data("developer", ""),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    assert_eq!(nodes(&s), vec![0]);
    assert!(s.derive_messages().unwrap().is_empty());
    let r = Session::from_restore(
        s.id().clone(),
        &s.snapshot_events(None, None),
        (*s.header()).clone(),
        offset(0),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert!(r.derive_messages().unwrap().is_empty());
}
#[test]
fn rejects_tool_changes_in_user_messages_at_session_admission() {
    for t in ["tool-addition", "tool-removal"] {
        let s = session();
        let mut d = data("user", "");
        d["content"] = json!([{"type":t,"toolName":"search"}]);
        error(
            s.append("user/message", d, Some(SurfaceIntent::append())),
            "developer role",
        );
    }
}
#[test]
fn retains_developer_blocks_through_append_replacement_and_restoration() {
    let s = session();
    let h = request(&s, vec![tool("search")], "initial");
    let first = developer(
        &s,
        json!([{"type":"tool-addition","toolName":"search"}]),
        Some(h.seq()),
    )
    .unwrap();
    let mut d = data("developer", "");
    d["message"]["content"] = json!([{"type":"tool-removal","toolName":"search"}]);
    let e = s
        .append(
            "developer/message",
            d,
            Some(SurfaceIntent::replace(
                first.seq(),
                first.seq(),
                vec![first.seq()],
            )),
        )
        .unwrap();
    assert_eq!(*s.derive_messages().unwrap()[0], e.data()["message"]);
    let r = Session::from_restore(
        s.id().clone(),
        &s.snapshot_events(None, None),
        (*s.header()).clone(),
        offset(0),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(r.derive_messages().unwrap(), s.derive_messages().unwrap());
}
