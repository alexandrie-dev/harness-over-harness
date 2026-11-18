#![allow(dead_code)]
pub use hoh_session_format::*;
pub use hoh_session_format_v3_to_v4::*;
pub use serde_json::{Value, json};
pub use std::collections::HashSet;
pub type R<T = ()> = Result<T, SessionFormatError>;
pub fn obj(value: Value) -> SessionFormatJsonObject {
    value.as_object().unwrap().clone()
}
pub fn h(version: u64, id: &str) -> SessionFormatHeader {
    obj(json!({"version":version,"id":id,"createdAt":1,"isSeeded":false,"delegationDepth":0}))
}
pub fn e(tag: &str, seq: u64, data: Value) -> SessionFormatEvent {
    obj(json!({"type":tag,"seq":seq,"time":seq+1,"data":data}))
}
pub fn ev(tag: &str, data: Value) -> SessionFormatEvent {
    e(tag, 0, data)
}
pub fn surface(tag: &str, data: Value) -> SessionFormatEvent {
    let mut v = ev(tag, data);
    v.insert("surfaceOp".into(), "append".into());
    v
}
pub fn dense(mut events: Vec<SessionFormatEvent>) -> Vec<SessionFormatEvent> {
    for (i, e) in events.iter_mut().enumerate() {
        e.insert("seq".into(), i.into());
    }
    events
}
pub fn numbered(events: Vec<SessionFormatEvent>, time0: u64) -> Vec<SessionFormatEvent> {
    let mut events = dense(events);
    for (i, e) in events.iter_mut().enumerate() {
        e.insert("time".into(), (time0 + i as u64).into());
    }
    events
}
pub fn opening() -> Vec<SessionFormatEvent> {
    vec![
        ev("turn/start", json!({"turn":1})),
        ev("step/start", json!({"turn":1,"step":1})),
    ]
}
pub fn closing() -> Vec<SessionFormatEvent> {
    vec![
        ev("step/end", json!({"turn":1,"step":1})),
        ev("turn/end", json!({"turn":1,"reason":{"kind":"completed"}})),
    ]
}
pub fn user_data(id: &str) -> Value {
    json!({"id":id,"role":"user","source":{"kind":"user"},"content":[{"type":"text","text":id}]})
}
pub fn user(id: &str) -> SessionFormatEvent {
    surface("user/message", user_data(id))
}
pub fn system() -> SessionFormatEvent {
    surface(
        "system/message",
        json!({"turn":1,"step":1,"message":{"id":"system","role":"system","source":{"kind":"system-prompt"},"content":[{"type":"text","text":"system"}]}}),
    )
}
pub fn call_block() -> Value {
    json!({"type":"tool-call","id":"call","name":"read","arguments":"{}"})
}
pub fn assistant(content: Value) -> SessionFormatEvent {
    surface(
        "assistant/message",
        json!({"turn":1,"step":1,"stream":[],"message":{"id":"assistant","role":"assistant","source":{"kind":"model","provider":"mock","model":"mock"},"content":content}}),
    )
}
pub fn native_message(call: &str, text: &str, flag: Option<Value>) -> Value {
    let mut m = json!({"id":format!("tool-result-{call}"),"role":"tool","source":{"kind":"tool","callId":call},"toolCallId":call,"content":[{"type":"text","text":text}]});
    if let Some(flag) = flag {
        m["isError"] = flag;
    }
    m
}
pub fn wrapper_message() -> Value {
    json!({"id":"tool-result-call-1","role":"user","source":{"kind":"tool","callId":"call-1"},"content":[{"type":"tool-result","toolCallId":"call-1","isError":false,"content":[{"type":"text","text":"result"}]}]})
}
pub fn result(message: Value) -> SessionFormatEvent {
    surface("tool/result", json!({"turn":1,"step":1,"message":message}))
}
pub fn request() -> SessionFormatEvent {
    ev(
        "request/header",
        json!({"reason":"initial","header":{"config":{"provider":"mock","model":"mock"}}}),
    )
}
pub fn developer_message() -> Value {
    json!({"id":"developer-1","role":"developer","source":{"kind":"tool-registry"},"content":[{"type":"tool-addition","toolName":"search"},{"type":"tool-removal","toolName":"old"}]})
}
pub fn developer_events() -> Vec<SessionFormatEvent> {
    let mut events = opening();
    let mut header = request();
    header["data"]["header"]["tools"] =
        json!([{"name":"search","description":"Search","parameters":{},"deferLoading":true}]);
    events.push(header);
    let mut developer = surface(
        "developer/message",
        json!({"turn":1,"step":1,"headerSeq":2,"message":developer_message()}),
    );
    developer.insert("seq".into(), 3.into());
    events.push(developer);
    events.extend(closing());
    numbered(events, 1)
}
pub fn artifact(
    events: Vec<SessionFormatEvent>,
    header: SessionFormatHeader,
    cut: u64,
) -> SessionFormatArtifact {
    SessionFormatArtifact {
        header,
        inherited_event_count: cut,
        events,
    }
}
pub fn known(events: &[SessionFormatEvent]) -> HashSet<String> {
    events
        .iter()
        .filter_map(|e| e.get("type").and_then(Value::as_str).map(str::to_owned))
        .collect()
}
pub fn native(events: Vec<SessionFormatEvent>) -> R<SessionFormatArtifact> {
    let events = dense(events);
    let known = known(&events);
    restore_released_v4_artifact(artifact(events, h(4, "native"), 0), &known)
}
pub fn stage(
    header: &SessionFormatHeader,
    cut: Option<u64>,
    children: Vec<Value>,
) -> R<Box<dyn SessionFormatMigrationStage>> {
    let migration = create_session_format_v3_to_v4(children);
    let target = migration.migrate_header(header.clone())?;
    migration.create_stage(SessionFormatMigrationStageInput {
        source_header: header,
        target_header: &target,
        source_inherited_event_count: cut,
        source_kind: SessionFormatSourceKind::Decoded,
    })
}
pub fn migrate(
    events: Vec<SessionFormatEvent>,
    header: SessionFormatHeader,
    cut: Option<u64>,
    children: Vec<Value>,
) -> R<SessionFormatArtifact> {
    let mut stage = stage(&header, cut, children)?;
    let mut output = SessionFormatEventCollector::new();
    for event in events {
        stage.transform_event(event, &mut output)?;
    }
    let cut = stage.finish(&mut output)?;
    let header = SESSION_FORMAT_V3_TO_V4.migrate_header(header)?;
    Ok(artifact(output.values, header, cut))
}
pub fn round_trip(
    events: &[SessionFormatEvent],
    header: &SessionFormatHeader,
    cut: u64,
) -> R<(Vec<SessionFormatEvent>, u64)> {
    let physical = RELEASED_V4_SESSION_FORMAT_CODEC.encode_header(header, cut)?;
    let mut decoder = RELEASED_V4_SESSION_FORMAT_CODEC
        .create_decoder(&physical.into(), SessionFormatRecovery::Strict)?;
    let mut output = SessionFormatEventCollector::new();
    for event in events {
        decoder.decode_row(
            RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(event)?.into(),
            &mut output,
        )?;
    }
    let cut = decoder.finish(&mut output)?;
    Ok((output.values, cut))
}
pub fn err<T>(value: R<T>, message: &str) {
    let error = match value {
        Ok(_) => panic!("expected error containing {message}"),
        Err(e) => e,
    };
    assert!(
        error.to_string().contains(message),
        "{error:?} does not contain {message:?}"
    );
}
pub struct Run(pub Vec<SessionFormatEvent>);
pub fn complex_restart_source() -> (Vec<SessionFormatEvent>, Value) {
    let start = |n: u64| ev("turn/start", json!({"turn":n}));
    let end = |n: u64| ev("turn/end", json!({"turn":n,"reason":{"kind":"completed"}}));
    let splice = || {
        ev(
            "agent/inbox/spliced",
            json!({"target":"next-turn","inserted":[user_data("next")]}),
        )
    };
    let mut checkpoint = user("checkpoint");
    checkpoint["data"]["source"] = json!({"kind":"plugin","plugin":"compact","compactionId":"c"});
    checkpoint.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":1,"endSeq":4}),
    );
    checkpoint.insert("sourceEventSeqs".into(), json!([1, 4, 5, 6]));
    let mut title_message = user_data("captured seq 4");
    title_message["source"] = json!({"kind":"dsh-session-title-llm"});
    let mut capture = user("capture");
    capture["data"]["source"] = json!({"kind":"plugin","plugin":"session-reference","sessionId":"other","seq":4,"sessionFormatVersion":3});
    let mut opaque = ev("external/opaque", json!({"seq":4}));
    opaque.insert("ignorable".into(), true.into());
    opaque.insert("sourceEventSeqs".into(), json!([4]));
    opaque.insert("surfaceOp".into(), json!({"opaque":4}));
    let rows = numbered(
        vec![
            start(1),
            user("before"),
            splice(),
            start(2),
            user("after"),
            ev("compaction/start", json!({"compactionId":"c","turn":2})),
            ev(
                "compaction/summary",
                json!({"compactionId":"c","summary":[{"type":"text","text":"summary"}],"shadowedRange":{"start":1,"end":4},"shadowedSeqs":[1,4],"shadowedTokenCount":1,"provider":"mock","model":"mock"}),
            ),
            checkpoint,
            ev("compaction/end", json!({"compactionId":"c","turn":2})),
            ev(
                "command/run",
                json!({"commandId":"cmd","name":"test","source":{"kind":"user"}}),
            ),
            ev(
                "command/done",
                json!({"commandId":"cmd","kind":"success","sourceEventSeq":7}),
            ),
            ev(
                "session/title",
                json!({"title":"title","source":{"kind":"generated"},"messageSeqs":[1,4]}),
            ),
            ev(
                "session/title-llm-request",
                json!({"messageSeqs":[1,4],"messages":[title_message]}),
            ),
            ev(
                "session-log-deepseek/delivery-accepted",
                json!({"sessionId":"restart","sessionFormatVersion":3,"throughSeq":12}),
            ),
            capture,
            opaque,
            end(2),
        ],
        10,
    );
    (rows, title_message)
}
pub fn restore_v2(
    events: &[SessionFormatEvent],
    header: SessionFormatHeader,
) -> R<SessionFormatArtifact> {
    let edge = hoh_session_format_v2_to_v3::SESSION_FORMAT_V2_TO_V3;
    let target = edge.migrate_header(header.clone())?;
    let mut stage = edge.create_stage(SessionFormatMigrationStageInput {
        source_header: &header,
        target_header: &target,
        source_inherited_event_count: None,
        source_kind: SessionFormatSourceKind::Decoded,
    })?;
    let mut physical = obj(json!({"type":"session"}));
    physical.extend(header);
    let mut decoder = hoh_session_format_v2_to_v3::RELEASED_V2_SESSION_FORMAT_CODEC
        .create_decoder(&physical.into(), SessionFormatRecovery::Strict)?;
    let mut decoded = SessionFormatEventCollector::new();
    for row in events {
        decoder.decode_row(row.clone().into(), &mut decoded)?;
    }
    decoder.finish(&mut decoded)?;
    let mut v3 = SessionFormatEventCollector::new();
    for event in decoded.values {
        stage.transform_event(event, &mut v3)?;
    }
    let cut = stage.finish(&mut v3)?;
    let target = migrate(v3.values, target, Some(cut), vec![])?;
    let known = RELEASED_V3_EVENT_TYPES
        .iter()
        .copied()
        .chain(["developer/message"])
        .map(str::to_owned)
        .collect();
    restore_released_v4_artifact(target, &known)
}
pub fn canonical_source() -> Vec<SessionFormatEvent> {
    let m = json!({"id":"context","role":"user","source":{"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt"},"content":[{"type":"text","text":"context"}]});
    let dispatch = json!({"rootCallId":"root","parentCallId":"root","subCallId":"child","name":"test","arguments":{"kind":"plugin","plugin":"tools-ptc"}});
    let mut replacement = surface("user/message", m.clone());
    replacement["data"]["id"] = "replacement".into();
    replacement["data"]["source"] = json!({"kind":"plugin","plugin":"tools-code-mode"});
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":3,"end":3}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([3]));
    let mut settled = dispatch.clone();
    settled["isError"] = false.into();
    settled["content"] = json!([]);
    let mut rows = opening();
    rows.extend([ev("request/header",json!({"reason":"initial","header":{"config":{"provider":"mock","model":"mock"},"system":"system","tools":[]}})),surface("user/message",m),ev("tool/code-dispatch-start",dispatch),ev("tool/code-dispatch",settled),replacement,ev("request/header",json!({"reason":"change","header":{"config":{"provider":"mock","model":"mock"},"system":""}}))]);
    rows.extend(closing());
    rows.push(ev("session/end-seed", json!({"inherited":true})));
    numbered(rows, 1)
}
pub fn canonical_header() -> SessionFormatHeader {
    let mut header = h(2, "canonical-v4");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "parent".into());
    header
}
pub const FORK_NOT_STARTED_TEXT: &str = "The history inherited by this branch has no record of this tool call starting. The parent session may have executed it after the fork point. Decide whether to retry from the tool semantics: retry only if the operation is read-only or idempotent; if it may have side effects, first verify external state or ask the user. Do not retry blindly.";
pub fn fork_artifact() -> SessionFormatArtifact {
    let mut rows = opening();
    rows.push(assistant(
        json!([{"type":"tool-call","id":"read-one","name":"read","arguments":"{}"}]),
    ));
    rows = numbered(rows, 1);
    rows.push(obj(
        json!({"type":"session/end-seed","seq":3,"time":3,"data":{"inherited":true}}),
    ));
    let mut message = native_message("read-one", FORK_NOT_STARTED_TEXT, Some(true.into()));
    message["id"] = "forked-tool-result-read-one-4".into();
    let mut result = result(message);
    result.insert("seq".into(), 4.into());
    result.insert("time".into(), 3.into());
    result["data"]["error"] = json!({"name":"ToolNotStartedError","code":"TOOL_NOT_STARTED"});
    rows.push(result);
    rows.push(obj(
        json!({"type":"step/end","seq":5,"time":3,"data":{"turn":1,"step":1}}),
    ));
    rows.push(obj(
        json!({"type":"turn/end","seq":6,"time":3,"data":{"turn":1,"reason":{"kind":"forked"}}}),
    ));
    let mut header = h(4, "fork-child");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "fork-parent".into());
    artifact(rows, header, 3)
}
pub fn restore_v3(
    events: &[SessionFormatEvent],
    header: SessionFormatHeader,
    recovery: SessionFormatRecovery,
) -> R<SessionFormatArtifact> {
    let mut physical = obj(json!({"type":"session"}));
    physical.extend(header.clone());
    let mut decoder =
        RELEASED_V3_SESSION_FORMAT_CODEC.create_decoder(&physical.into(), recovery)?;
    let mut stage = stage(
        decoder.header(),
        decoder.header_inherited_event_count(),
        vec![],
    )?;
    let mut output = SessionFormatEventCollector::new();
    struct Next<'a> {
        stage: &'a mut dyn SessionFormatMigrationStage,
        output: &'a mut SessionFormatEventCollector,
    }
    impl SessionFormatMigrationContext for Next<'_> {
        fn emit_event(&mut self, event: SessionFormatEvent) -> R {
            self.stage.transform_event(event, self.output)
        }
        fn emit_run(&mut self, run: Box<dyn SessionFormatEventRun>) -> R {
            self.stage.transform_run(run, self.output)
        }
    }
    {
        let mut next = Next {
            stage: stage.as_mut(),
            output: &mut output,
        };
        for event in events {
            decoder.decode_row(event.clone().into(), &mut next)?;
        }
        decoder.finish(&mut next)?;
    }
    let cut = stage.finish(&mut output)?;
    let header = SESSION_FORMAT_V3_TO_V4.migrate_header(header)?;
    let known = RELEASED_V3_EVENT_TYPES
        .iter()
        .copied()
        .chain(["developer/message"])
        .map(str::to_owned)
        .collect();
    restore_released_v4_artifact(artifact(output.values, header, cut), &known)
}
impl SessionFormatEventRun for Run {
    fn run_type(&self) -> &str {
        "identity"
    }
    fn first_seq(&self) -> u64 {
        0
    }
    fn event_count(&self) -> u64 {
        self.0.len() as u64
    }
    fn expand(self: Box<Self>) -> Box<dyn Iterator<Item = SessionFormatEvent>> {
        Box::new(self.0.into_iter())
    }
}
