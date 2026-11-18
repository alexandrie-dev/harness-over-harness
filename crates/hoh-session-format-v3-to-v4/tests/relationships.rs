mod common;
use common::*;
fn result_row() -> SessionFormatEvent {
    result(
        json!({"id":"result","role":"tool","source":{"kind":"tool","callId":"call"},"toolCallId":"call","content":[{"type":"text","text":"done"}]}),
    )
}
fn tool_call() -> SessionFormatEvent {
    ev(
        "tool/call",
        json!({"turn":1,"step":1,"callId":"call","name":"read","arguments":"{}"}),
    )
}
fn chain(parts: Vec<Vec<SessionFormatEvent>>) -> Vec<SessionFormatEvent> {
    parts.into_iter().flatten().collect()
}
fn exchange() -> Vec<SessionFormatEvent> {
    chain(vec![
        opening(),
        vec![
            user("input"),
            request(),
            assistant(json!([call_block()])),
            tool_call(),
            result_row(),
        ],
        closing(),
    ])
}
fn compact() -> Vec<SessionFormatEvent> {
    let mut checkpoint = user("checkpoint");
    checkpoint["data"]["source"] =
        json!({"kind":"compact-checkpoint","compactionId":"c","sourceCommandId":"cmd"});
    checkpoint.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":1,"endSeq":1}),
    );
    checkpoint.insert("sourceEventSeqs".into(), json!([1, 2, 3]));
    vec![
        ev("turn/start", json!({"turn":1})),
        user("input"),
        ev(
            "compaction/start",
            json!({"compactionId":"c","turn":1,"sourceCommandId":"cmd"}),
        ),
        ev(
            "compaction/summary",
            json!({"compactionId":"c","sourceCommandId":"cmd","summary":[{"type":"text","text":"summary"}],"shadowedRange":{"start":1,"end":1},"shadowedSeqs":[1],"shadowedTokenCount":1,"provider":"mock","model":"mock"}),
        ),
        checkpoint,
        ev(
            "compaction/end",
            json!({"compactionId":"c","sourceCommandId":"cmd","turn":1}),
        ),
        ev("turn/end", json!({"turn":1,"reason":{"kind":"completed"}})),
    ]
}
fn changed(
    mut rows: Vec<SessionFormatEvent>,
    index: usize,
    data: Value,
) -> Vec<SessionFormatEvent> {
    rows[index]["data"]
        .as_object_mut()
        .unwrap()
        .extend(obj(data));
    rows
}
fn retry(start: bool, overrides: Value) -> SessionFormatEvent {
    let mut data = json!({"turn":1,"step":1,"retryId":"retry","retry":1});
    if !start {
        data["provider"] = "mock".into();
        data["policyKey"] = "policy".into();
    }
    data.as_object_mut().unwrap().extend(obj(overrides));
    ev(
        if start {
            "llm/retry-started"
        } else {
            "llm/retry"
        },
        data,
    )
}
fn dispatch(start: bool, overrides: Value) -> SessionFormatEvent {
    let mut data = json!({"rootCallId":"root","parentCallId":"root","subCallId":"child","name":"read","arguments":{"a":1,"b":2}});
    data.as_object_mut().unwrap().extend(obj(overrides));
    ev(
        if start {
            "tool/ptc-dispatch-start"
        } else {
            "tool/ptc-dispatch"
        },
        data,
    )
}
#[test]
fn retains_valid_ordinary_tool_exchanges_and_open_interrupted_tails() {
    let rows = exchange();
    assert!(native(rows.clone()).is_ok());
    for len in 0..rows.len() {
        assert!(native(rows[..len].to_vec()).is_ok(), "prefix {len}");
    }
}
#[test]
fn rejects_invalid_lifecycle_sequences() {
    let mut changed_name = tool_call();
    changed_name["data"]["name"] = "other".into();
    let mut changed_args = tool_call();
    changed_args["data"]["arguments"] = "[]".into();
    let adv = assistant(json!([call_block()]));
    let cases = vec![
        ("turn order", vec![ev("turn/start", json!({"turn":2}))]),
        (
            "duplicate turn",
            vec![opening()[0].clone(), opening()[0].clone()],
        ),
        ("unopened turn end", vec![ev("turn/end", json!({"turn":1}))]),
        (
            "open step at turn end",
            chain(vec![opening(), vec![ev("turn/end", json!({"turn":1}))]]),
        ),
        ("unopened step", vec![opening()[1].clone()]),
        (
            "step sequence",
            vec![
                opening()[0].clone(),
                ev("step/start", json!({"turn":1,"step":2})),
            ],
        ),
        (
            "mismatched step end",
            chain(vec![
                opening(),
                vec![ev("step/end", json!({"turn":2,"step":1}))],
            ]),
        ),
        ("assistant outside step", vec![assistant(json!([]))]),
        (
            "assistant attempt outside step",
            vec![ev(
                "assistant/attempt",
                json!({"turn":1,"step":1,"stream":[]}),
            )],
        ),
        (
            "tool without advertisement",
            chain(vec![opening(), vec![tool_call()]]),
        ),
        (
            "result without advertisement",
            chain(vec![opening(), vec![result_row()]]),
        ),
        (
            "repeated advertisement",
            chain(vec![
                opening(),
                vec![assistant(json!([call_block(), call_block()]))],
            ]),
        ),
        (
            "changed tool name",
            chain(vec![opening(), vec![adv.clone(), changed_name]]),
        ),
        (
            "changed tool arguments",
            chain(vec![opening(), vec![adv.clone(), changed_args]]),
        ),
        (
            "repeated start",
            chain(vec![opening(), vec![adv.clone(), tool_call(), tool_call()]]),
        ),
        (
            "unstarted success",
            chain(vec![opening(), vec![adv.clone(), result_row()]]),
        ),
        (
            "repeated result",
            chain(vec![
                opening(),
                vec![adv.clone(), tool_call(), result_row(), result_row()],
            ]),
        ),
        (
            "unresolved advertised call",
            chain(vec![opening(), vec![adv.clone()], closing()]),
        ),
        (
            "unresolved started call",
            chain(vec![opening(), vec![adv, tool_call()], closing()]),
        ),
        ("header outside turn", vec![request()]),
        (
            "context outside turn",
            vec![ev(
                "request/context",
                json!({"provider":"mock","model":"mock"}),
            )],
        ),
    ];
    for (name, rows) in cases {
        assert!(native(rows).is_err(), "{name}");
    }
}
#[test]
fn accepts_exact_not_started_repairs_with_preserved_historical_identity_suffixes() {
    let mut repaired = result_row();
    repaired["data"]["message"]["id"] = "interrupted-tool-result-call-200".into();
    repaired["data"]["message"]["isError"] = true.into();
    repaired["data"]["message"]["content"] = json!([{"type":"text","text":"The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."}]);
    repaired["data"]["error"] = json!({"name":"ToolNotStartedError","code":"TOOL_NOT_STARTED"});
    assert!(
        native(chain(vec![
            opening(),
            vec![assistant(json!([call_block()])), repaired.clone()],
            closing()
        ]))
        .is_ok()
    );
    for extra in [
        json!({"id":"made-up"}),
        json!({"id":"interrupted-tool-result-call-01"}),
        json!({"id":"interrupted-tool-result-call-9007199254740992"}),
        json!({"content":[]}),
        json!({"content":[{"type":"text","text":"invented repair"}]}),
    ] {
        let mut invalid = repaired.clone();
        invalid["data"]["message"]
            .as_object_mut()
            .unwrap()
            .extend(obj(extra));
        assert!(
            native(chain(vec![
                opening(),
                vec![assistant(json!([call_block()])), invalid]
            ]))
            .is_err()
        );
    }
    let mut invalid = repaired.clone();
    invalid.insert("sourceEventSeqs".into(), json!([2]));
    assert!(
        native(chain(vec![
            opening(),
            vec![assistant(json!([call_block()])), invalid]
        ]))
        .is_err()
    );
    repaired["data"]["error"]["name"] = "OtherError".into();
    assert!(
        native(chain(vec![
            opening(),
            vec![assistant(json!([call_block()])), repaired]
        ]))
        .is_err()
    );
}
#[test]
fn preserves_ignorable_payloads_and_unknown_attribution_metadata() {
    let mut u = user("input");
    u["data"]["source"] = json!({"kind":"external","__proto__":{"x":1},"meta":{"a":2}});
    let mut opaque = ev("external/event", json!({"arbitrary":true}));
    opaque.insert("ignorable".into(), true.into());
    let rows = dense(vec![opaque.clone(), u.clone()]);
    let k = ["user/message"].into_iter().map(str::to_owned).collect();
    let a = restore_released_v4_artifact(artifact(rows, h(4, "native-relations"), 0), &k).unwrap();
    assert_eq!(a.events[0]["data"], opaque["data"]);
    assert_eq!(a.events[1]["data"]["source"], u["data"]["source"]);
}
#[test]
fn retains_matching_checkpoint_sources_and_inherited_unfinished_compactions() {
    let a = native(compact()).unwrap();
    assert_eq!(
        a.events[4]["data"]["source"],
        json!({"kind":"compact-checkpoint","compactionId":"c","sourceCommandId":"cmd"})
    );
    let mut rows = vec![ev(
        "compaction/start",
        json!({"compactionId":"old","turn":null}),
    )];
    rows.extend(opening());
    rows.extend(closing());
    rows.push(ev("session/end-seed", json!({"inherited":true})));
    let rows = dense(rows);
    let mut header = h(4, "native-relations");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "parent".into());
    assert!(restore_released_v4_artifact(artifact(rows.clone(), header, 5), &known(&rows)).is_ok());
    assert!(
        native(vec![
            ev("compaction/start", json!({"compactionId":"c","turn":null})),
            ev(
                "compaction/end",
                json!({"compactionId":"c","turn":null,"error":"cancelled"})
            )
        ])
        .is_ok()
    );
    assert!(native(compact()[..5].to_vec()).is_ok());
}
#[test]
fn rejects_invalid_compaction_ownership() {
    let cases = vec![
        (
            "checkpoint identity",
            changed(
                compact(),
                4,
                json!({"source":{"kind":"compact-checkpoint","compactionId":"WRONG","sourceCommandId":"cmd"}}),
            ),
        ),
        (
            "checkpoint command",
            changed(
                compact(),
                4,
                json!({"source":{"kind":"compact-checkpoint","compactionId":"c","sourceCommandId":"WRONG"}}),
            ),
        ),
        (
            "summary identity",
            changed(compact(), 3, json!({"compactionId":"WRONG"})),
        ),
        (
            "end identity",
            changed(compact(), 5, json!({"compactionId":"WRONG"})),
        ),
        ("end turn", changed(compact(), 5, json!({"turn":null}))),
        ("start turn", changed(compact(), 2, json!({"turn":null}))),
        (
            "wrong span",
            changed(compact(), 3, json!({"shadowedSeqs":[]})),
        ),
        (
            "missing span",
            changed(
                compact(),
                3,
                json!({"shadowedRange":{"start":0,"end":0},"shadowedSeqs":[0]}),
            ),
        ),
        (
            "overlap",
            chain(vec![
                compact()[..3].to_vec(),
                vec![ev(
                    "compaction/start",
                    json!({"compactionId":"second","turn":1}),
                )],
            ]),
        ),
        (
            "repeated summary",
            chain(vec![compact()[..4].to_vec(), vec![compact()[3].clone()]]),
        ),
        (
            "success without summary",
            chain(vec![compact()[..3].to_vec(), vec![compact()[5].clone()]]),
        ),
        (
            "turn closes transaction",
            chain(vec![compact()[..3].to_vec(), vec![compact()[6].clone()]]),
        ),
        (
            "unowned end",
            vec![ev(
                "compaction/end",
                json!({"compactionId":"c","turn":null}),
            )],
        ),
    ];
    for (name, rows) in cases {
        assert!(native(rows).is_err(), "{name}");
    }
}
#[test]
fn accepts_ptc_nesting_and_settlements_with_equal_argument_objects() {
    let rows = chain(vec![
        opening(),
        vec![
            dispatch(true, json!({})),
            dispatch(
                true,
                json!({"subCallId":"grandchild","parentCallId":"child"}),
            ),
            dispatch(
                false,
                json!({"subCallId":"grandchild","parentCallId":"child"}),
            ),
            dispatch(false, json!({"arguments":{"b":2,"a":1}})),
        ],
        closing(),
    ]);
    assert!(native(rows).is_ok());
}
#[test]
fn rejects_invalid_ptc_relationships() {
    let cases = vec![
        ("outside turn", vec![dispatch(true, json!({}))]),
        (
            "without start",
            chain(vec![opening(), vec![dispatch(false, json!({}))]]),
        ),
        (
            "duplicate start",
            chain(vec![
                opening(),
                vec![dispatch(true, json!({})), dispatch(true, json!({}))],
            ]),
        ),
        (
            "duplicate settlement",
            chain(vec![
                opening(),
                vec![
                    dispatch(true, json!({})),
                    dispatch(false, json!({})),
                    dispatch(false, json!({})),
                ],
            ]),
        ),
        (
            "unknown parent",
            chain(vec![
                opening(),
                vec![dispatch(true, json!({"parentCallId":"unknown"}))],
            ]),
        ),
        (
            "root changes",
            chain(vec![
                opening(),
                vec![
                    dispatch(true, json!({})),
                    dispatch(false, json!({"rootCallId":"different"})),
                ],
            ]),
        ),
        (
            "arguments change",
            chain(vec![
                opening(),
                vec![
                    dispatch(true, json!({})),
                    dispatch(false, json!({"arguments":{}})),
                ],
            ]),
        ),
        (
            "name changes",
            chain(vec![
                opening(),
                vec![
                    dispatch(true, json!({})),
                    dispatch(false, json!({"name":"different"})),
                ],
            ]),
        ),
    ];
    for (name, rows) in cases {
        assert!(native(rows).is_err(), "{name}");
    }
}
#[test]
fn accepts_retry_chains_including_retries_scheduled_after_the_step_ends() {
    assert!(
        native(chain(vec![
            opening(),
            vec![
                request(),
                retry(false, json!({})),
                retry(true, json!({})),
                retry(false, json!({"retry":2})),
                retry(true, json!({"retry":2})),
                ev("step/end", json!({"turn":1,"step":1})),
                retry(false, json!({"retry":3})),
                ev("turn/end", json!({"turn":1,"reason":{"kind":"completed"}}))
            ]
        ]))
        .is_ok()
    );
}
#[test]
fn rejects_invalid_retry_relationships() {
    let prefix = chain(vec![opening(), vec![request()]]);
    let cases = vec![
        ("outside turn", vec![retry(false, json!({}))]),
        (
            "wrong step",
            chain(vec![prefix.clone(), vec![retry(false, json!({"step":2}))]]),
        ),
        (
            "wrong provider",
            chain(vec![
                prefix.clone(),
                vec![retry(false, json!({"provider":"other"}))],
            ]),
        ),
        (
            "skip attempt",
            chain(vec![prefix.clone(), vec![retry(false, json!({"retry":2}))]]),
        ),
        (
            "changed identity",
            chain(vec![
                prefix.clone(),
                vec![
                    retry(false, json!({})),
                    retry(false, json!({"retry":2,"retryId":"other"})),
                ],
            ]),
        ),
        (
            "reused identity",
            chain(vec![
                prefix.clone(),
                vec![
                    retry(false, json!({})),
                    retry(false, json!({"policyKey":"other"})),
                ],
            ]),
        ),
        ("unpaired start", vec![retry(true, json!({}))]),
        (
            "changed start coordinates",
            chain(vec![
                prefix.clone(),
                vec![retry(false, json!({})), retry(true, json!({"step":2}))],
            ]),
        ),
        (
            "duplicate start",
            chain(vec![
                prefix,
                vec![
                    retry(false, json!({})),
                    retry(true, json!({})),
                    retry(true, json!({})),
                ],
            ]),
        ),
    ];
    for (name, rows) in cases {
        assert!(native(rows).is_err(), "{name}");
    }
}
#[test]
fn requires_command_completion_to_cite_its_run_and_an_earlier_non_command_success_event() {
    let run = ev(
        "command/run",
        json!({"commandId":"cmd","name":"test","source":{"kind":"user"}}),
    );
    let done = ev(
        "command/done",
        json!({"commandId":"cmd","kind":"success","sourceEventSeq":1}),
    );
    assert!(
        native(vec![
            run.clone(),
            user("input"),
            done.clone(),
            ev("command/done", json!({"commandId":"cmd","kind":"error"}))
        ])
        .is_ok()
    );
    for rows in [
        vec![done.clone()],
        vec![run.clone(), run.clone()],
        vec![run.clone(), done.clone()],
        changed(
            vec![run.clone(), user("input"), done.clone()],
            2,
            json!({"kind":"error"}),
        ),
        changed(
            vec![run, user("input"), done],
            2,
            json!({"sourceEventSeq":10}),
        ),
    ] {
        assert!(native(rows).is_err());
    }
}
#[test]
fn requires_titles_and_durable_title_requests_to_cite_earlier_human_messages() {
    let title = ev(
        "session/title",
        json!({"title":"Example","source":{"kind":"generated"},"messageSeqs":[0]}),
    );
    let mut m = user_data("input");
    m["source"] = json!({"kind":"dsh-session-title-llm"});
    m["content"] = json!([{"type":"text","text":"historical frame with original coordinates"}]);
    let request_title = ev(
        "session/title-llm-request",
        json!({"messageSeqs":[0],"messages":[m]}),
    );
    assert!(
        native(vec![
            user("input"),
            title.clone(),
            request_title.clone(),
            ev(
                "session/title",
                json!({"title":"Manual","source":{"kind":"user"},"messageSeqs":[]})
            )
        ])
        .is_ok()
    );
    let mut external = user("input");
    external["data"]["source"] = json!({"kind":"external"});
    let cases = vec![
        vec![external, title.clone()],
        changed(
            vec![user("input"), title.clone()],
            1,
            json!({"source":{"kind":"user"}}),
        ),
        changed(
            vec![user("input"), title.clone()],
            1,
            json!({"messageSeqs":[0,0]}),
        ),
        changed(vec![user("input"), title], 1, json!({"messageSeqs":[1]})),
        changed(
            vec![user("input"), request_title.clone()],
            1,
            json!({"messages":[user_data("input")]}),
        ),
        changed(
            vec![user("input"), request_title],
            1,
            json!({"messageSeqs":[]}),
        ),
    ];
    for rows in cases {
        assert!(native(rows).is_err());
    }
}
#[test]
fn keeps_the_first_system_head_protected_across_exact_replacements() {
    let mut replacement = system();
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":2,"endSeq":2}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([2]));
    assert!(
        native(chain(vec![
            opening(),
            vec![system(), user("input"), replacement.clone(), system()],
            closing()
        ]))
        .is_ok()
    );
    let prune = ev(
        "compaction/prune",
        json!({"shadowedRange":{"start":3,"end":3},"shadowedSeqs":[3],"shadowedTokenCount":1}),
    );
    assert!(
        native(chain(vec![
            opening(),
            vec![system(), user("input"), prune],
            closing()
        ]))
        .is_ok()
    );
    let mut replace_user = user("input");
    replace_user.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":2,"endSeq":2}),
    );
    replace_user.insert("sourceEventSeqs".into(), json!([2]));
    let mut wide = replacement.clone();
    wide.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":2,"endSeq":3}),
    );
    wide.insert("sourceEventSeqs".into(), json!([2, 3]));
    let mut missing = replacement.clone();
    missing.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":0,"endSeq":0}),
    );
    let mut backwards = replacement.clone();
    backwards.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":3,"endSeq":2}),
    );
    let mut no_sources = replacement.clone();
    no_sources.insert("sourceEventSeqs".into(), json!([0]));
    for rows in [
        vec![user("input"), system()],
        vec![user("input"), replacement],
        vec![system(), replace_user],
        vec![system(), user("input"), wide],
        vec![system(), user("input"), missing],
        vec![system(), user("input"), backwards],
        vec![system(), user("input"), no_sources],
        vec![
            system(),
            ev(
                "compaction/prune",
                json!({"shadowedRange":{"start":2,"end":2},"shadowedSeqs":[2]}),
            ),
        ],
    ] {
        assert!(native(chain(vec![opening(), rows])).is_err());
    }
}
#[test]
fn rejects_malformed_relationship_values_and_mismatched_inherited_transaction_ownership() {
    assert!(native(vec![ev("command/run", json!({"commandId":""}))]).is_err());
    assert!(native(vec![ev("session/title", json!({"messageSeqs":null}))]).is_err());
    let mut rows = vec![
        ev(
            "compaction/start",
            json!({"compactionId":"old","turn":null}),
        ),
        ev("turn/start", json!({"turn":1})),
        user("input"),
        ev(
            "compaction/summary",
            json!({"compactionId":"old","shadowedRange":{"start":2,"end":2},"shadowedSeqs":[2]}),
        ),
        ev("turn/end", json!({"turn":1,"reason":{"kind":"completed"}})),
        ev("session/end-seed", json!({"inherited":true})),
    ];
    rows = dense(rows);
    let mut header = h(4, "native-relations");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "parent".into());
    err(
        restore_released_v4_artifact(artifact(rows.clone(), header, 5), &known(&rows)),
        "does not match the open turn",
    );
}
