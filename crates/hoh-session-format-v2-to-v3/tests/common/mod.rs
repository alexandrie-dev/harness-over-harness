#![allow(dead_code)]
use hoh_session_format::*;
use hoh_session_format_v0_to_v1::{
    RELEASED_V0_SESSION_FORMAT_CODEC, RELEASED_V1_SESSION_FORMAT_CODEC, SESSION_FORMAT_V0_TO_V1,
};
use hoh_session_format_v1_to_v2::SESSION_FORMAT_V1_TO_V2;
use hoh_session_format_v2_to_v3::*;
use serde_json::{Map, Value, json};
use std::{collections::HashSet, sync::Arc};
pub type R<T = ()> = Result<T, SessionFormatError>;
pub fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}
pub fn h(version: u64, id: &str) -> SessionFormatHeader {
    obj(json!({"version":version,"id":id,"createdAt":1,"isSeeded":false,"delegationDepth":0}))
}
pub fn e(kind: &str, seq: u64, data: Value) -> SessionFormatEvent {
    obj(json!({"type":kind,"seq":seq,"time":seq+1,"data":data}))
}
pub fn ev(kind: &str, data: Value) -> SessionFormatEvent {
    let mut e = e(kind, 0, data);
    e.insert("time".into(), 42.into());
    e
}
pub fn surface(kind: &str, data: Value) -> SessionFormatEvent {
    let mut e = ev(kind, data);
    e.insert("surfaceOp".into(), "append".into());
    e
}
pub fn dense(mut events: Vec<SessionFormatEvent>) -> Vec<SessionFormatEvent> {
    for (i, e) in events.iter_mut().enumerate() {
        e.insert("seq".into(), i.into());
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
pub fn request(prompt: Option<&str>) -> SessionFormatEvent {
    let mut header = json!({"config":{"provider":"mock","model":"mock"}});
    if let Some(s) = prompt {
        header["system"] = s.into();
    }
    ev(
        "request/header",
        json!({"header":header,"reason":"initial"}),
    )
}
pub fn model() -> Value {
    json!({"kind":"model","provider":"mock","model":"mock"})
}
pub fn assistant(content: Value, stream: Value) -> SessionFormatEvent {
    surface(
        "assistant/message",
        json!({"turn":1,"step":1,"stream":stream,"message":{"id":"assistant","role":"assistant","source":model(),"content":content}}),
    )
}
pub fn artifact(events: Vec<SessionFormatEvent>) -> SessionFormatArtifact {
    SessionFormatArtifact {
        header: h(3, "identity"),
        inherited_event_count: 0,
        events,
    }
}
pub fn restore(a: SessionFormatArtifact) -> R<SessionFormatArtifact> {
    restore_released_v3_artifact(a, &HashSet::new())
}
pub fn stage(
    header: &SessionFormatHeader,
    cut: Option<u64>,
    source_kind: SessionFormatSourceKind,
) -> R<Box<dyn SessionFormatMigrationStage>> {
    let target = SESSION_FORMAT_V2_TO_V3.migrate_header(header.clone())?;
    SESSION_FORMAT_V2_TO_V3.create_stage(SessionFormatMigrationStageInput {
        source_header: header,
        target_header: &target,
        source_inherited_event_count: cut,
        source_kind,
    })
}
pub fn raw(
    events: Vec<SessionFormatEvent>,
    header: SessionFormatHeader,
    cut: Option<u64>,
) -> R<SessionFormatArtifact> {
    let target = SESSION_FORMAT_V2_TO_V3.migrate_header(header.clone())?;
    let mut stage = stage(&header, cut, SessionFormatSourceKind::Decoded)?;
    let mut output = SessionFormatEventCollector::new();
    for event in events {
        stage.transform_event(event, &mut output)?;
    }
    Ok(SessionFormatArtifact {
        header: target,
        inherited_event_count: stage.finish(&mut output)?,
        events: output.values,
    })
}
pub fn migrate(events: Vec<SessionFormatEvent>) -> R<SessionFormatArtifact> {
    let a = raw(dense(events), h(2, "identity"), Some(0))?;
    restore_released_v3_artifact(
        a,
        &HashSet::from([
            "feedback/message-put".into(),
            "feedback/message-delete".into(),
        ]),
    )
}
pub fn seeded_header() -> SessionFormatHeader {
    let mut header = h(2, "identity");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "parent".into());
    header
}
pub fn err<T: std::fmt::Debug>(result: R<T>, fragment: &str) {
    let error = result.unwrap_err();
    assert!(
        error.to_string().contains(fragment),
        "expected {fragment:?}, got {error}"
    );
}
pub fn fails<T>(result: R<T>) {
    assert!(result.is_err());
}
pub fn known() -> HashSet<String> {
    // Exact frozen vocabulary from core/session/src/known-event-types.ts @39163dd3.
    [
        "agent-preset/selected",
        "agent/inbox/spliced",
        "approval/asked",
        "approval/decided",
        "approval/policy",
        "assistant/attempt",
        "assistant/message",
        "command/done",
        "command/run",
        "compaction/end",
        "compaction/prune",
        "compaction/start",
        "compaction/summary",
        "deliverables/presented",
        "developer/message",
        "feedback/message-delete",
        "feedback/message-put",
        "feedback/record",
        "goal/change",
        "hook/invoked",
        "hook/result",
        "image/offload",
        "job/registered",
        "job/settled",
        "llm/retry",
        "llm/retry-started",
        "model/selection",
        "permission/preset",
        "plan/mode",
        "request/context",
        "request/header",
        "sandbox/mode",
        "schedule/change",
        "session-log-deepseek/delivery-accepted",
        "session/end-seed",
        "session/title",
        "session/title-llm-request",
        "step/end",
        "step/start",
        "subagent/catalog",
        "subagent/descriptor",
        "subagent/model-selection-policy",
        "system/message",
        "team/member",
        "team/message/delivered",
        "team/message/queued",
        "team/mode",
        "team/task",
        "todo/write",
        "tool-workflow/agent-end",
        "tool-workflow/agent-start",
        "tool-workflow/run-end",
        "tool-workflow/run-start",
        "tool/call",
        "tool/ptc-dispatch",
        "tool/ptc-dispatch-start",
        "tool/result",
        "turn/end",
        "turn/start",
        "user/message",
        "web/deepseek-search-llm-request",
        "working-directory/change",
        "workspace/changes",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
pub fn catalog() -> SessionFormatCatalog {
    let restore: SessionFormatArtifactRestorer =
        Arc::new(|a| restore_released_v3_artifact(a, &known()));
    create_session_format_catalog(SessionFormatCatalogOptions {
        current_version: 3,
        codecs: vec![
            Arc::new(RELEASED_V0_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V1_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V2_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V3_SESSION_FORMAT_CODEC),
        ],
        migrations: vec![
            Arc::new(SESSION_FORMAT_V0_TO_V1),
            Arc::new(SESSION_FORMAT_V1_TO_V2),
            Arc::new(SESSION_FORMAT_V2_TO_V3),
        ],
        current_encoder: Arc::new(RELEASED_V3_SESSION_FORMAT_CODEC),
        restore_current_header: Arc::new(|h| {
            assert_released_v3_header(&h)?;
            Ok(h)
        }),
        restore_current: restore.clone(),
        restore_transformed_current: restore,
    })
    .unwrap()
}
pub fn physical(header: SessionFormatHeader) -> Value {
    let mut header = header;
    header.insert("type".into(), "session".into());
    Value::Object(header)
}
pub fn read(
    header: Value,
    rows: Vec<SessionFormatEvent>,
    recovery: SessionFormatRecovery,
    validation: SessionFormatValidation,
) -> R<SessionFormatArtifact> {
    let mut reader = catalog().create_restore(
        &header,
        SessionFormatRestoreOptions {
            recovery,
            validation,
        },
    )?;
    for row in rows {
        reader.decode_row(Value::Object(row))?;
    }
    reader.finish()
}
pub fn round_trip(a: SessionFormatArtifact) -> R<SessionFormatArtifact> {
    let header = Value::Object(
        RELEASED_V3_SESSION_FORMAT_CODEC.encode_header(&a.header, a.inherited_event_count)?,
    );
    let rows = a
        .events
        .iter()
        .map(|e| RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(e))
        .collect::<R<Vec<_>>>()?;
    read(
        header,
        rows,
        SessionFormatRecovery::Strict,
        SessionFormatValidation::Current,
    )
}
pub fn decoder(
    seeded: bool,
    recovery: SessionFormatRecovery,
) -> Box<dyn SessionFormatArtifactDecoder> {
    let mut header = h(3, "identity");
    header.insert("isSeeded".into(), seeded.into());
    RELEASED_V3_SESSION_FORMAT_CODEC
        .create_decoder(&physical(header), recovery)
        .unwrap()
}
pub fn requests(events: &[SessionFormatEvent], version: u64) -> Value {
    let mut surface: Vec<&SessionFormatEvent> = Vec::new();
    let mut result = Vec::new();
    for event in events {
        if event.get("surfaceOp") == Some(&json!("append")) {
            surface.push(event);
        } else if let Some(op) = event.get("surfaceOp") {
            let start = &op[if version == 2 { "start" } else { "startSeq" }];
            let end = &op[if version == 2 { "end" } else { "endSeq" }];
            let first = surface.iter().position(|e| e["seq"] == *start).unwrap();
            let last = surface.iter().position(|e| e["seq"] == *end).unwrap();
            assert!(first <= last);
            surface.splice(first..=last, [event]);
        }
        if event["type"] == "request/header" {
            let mut messages = Vec::new();
            for entry in &surface {
                let message = if entry["type"] == "user/message" {
                    &entry["data"]
                } else {
                    &entry["data"]["message"]
                };
                if entry["type"] == "system/message"
                    && message["content"].as_array().unwrap().is_empty()
                {
                    continue;
                }
                messages.push(json!({"role":message["role"],"content":message["content"]}));
            }
            if version == 2
                && let Some(prompt) = event["data"]["header"]["system"]
                    .as_str()
                    .filter(|s| !s.is_empty())
            {
                messages.insert(
                    0,
                    json!({"role":"system","content":[{"type":"text","text":prompt}]}),
                );
            }
            result.push(Value::Array(messages));
        }
    }
    Value::Array(result)
}
pub fn systems(a: &SessionFormatArtifact) -> Vec<&SessionFormatEvent> {
    a.events
        .iter()
        .filter(|e| e["type"] == "system/message")
        .collect()
}
pub fn tool_log(is_error: bool, error: Option<Value>) -> Vec<SessionFormatEvent> {
    let mut result = json!({"turn":1,"step":1,"message":{"id":"tool","role":"user","source":{"kind":"tool","callId":"call"},"content":[{"type":"tool-result","toolCallId":"call","isError":is_error,"content":[{"type":"text","text":"result"}]}]}});
    if let Some(error) = error {
        result["error"] = error;
    }
    let mut events = opening();
    events.push(assistant(
        json!([{"type":"tool-call","id":"call","name":"read","arguments":"{}"}]),
        json!([]),
    ));
    events.push(ev(
        "tool/call",
        json!({"turn":1,"step":1,"callId":"call","name":"read","arguments":"{}"}),
    ));
    let mut e = surface("tool/result", result);
    e.insert("sourceEventSeqs".into(), json!([3]));
    events.push(e);
    events.extend(closing());
    let mut events = dense(events);
    for (i, e) in events.iter_mut().enumerate() {
        e.insert("time".into(), (i + 1).into());
    }
    events
}
pub fn dispatch() -> Value {
    json!({"rootCallId":"tools-code-mode:root","parentCallId":"tools-code-mode:root","subCallId":"tools-code-mode:child","name":"read","arguments":{"path":"tools-code-mode","nested":[1,{"text":"tool/code-dispatch"}]}})
}
pub fn ptc_native() -> Vec<SessionFormatEvent> {
    vec![
        e("turn/start", 0, json!({"turn":1})),
        e("tool/ptc-dispatch-start", 1, dispatch()),
        e("tool/ptc-dispatch", 2, {
            let mut d = dispatch();
            d["isError"] = false.into();
            d["content"] = json!([{"type":"text","text":"tools-code-mode"}]);
            d
        }),
    ]
}
pub fn title(seq: u64, messages: Value) -> SessionFormatEvent {
    ev(
        "session/title-llm-request",
        json!({"titleProvider":"mock","messageSeqs":[seq],"route":{"provider":"mock","model":"mock"},"system":"title","messages":messages,"maxTokens":20}),
    )
}

pub fn composite_source() -> Vec<SessionFormatEvent> {
    let message = json!({"id":"tools-code-mode:message","role":"user","source":{"kind":"plugin","plugin":"tools-code-mode"},"content":[{"type":"text","text":"tool/code-dispatch tools-code-mode"}]});
    let outer = json!({"rootCallId":"tools-code-mode:root","parentCallId":"tools-code-mode:root","subCallId":"tools-code-mode:child","name":"run_code","arguments":{}});
    let mut inner = outer.clone();
    inner["parentCallId"] = outer["subCallId"].clone();
    inner["subCallId"] = "tools-ptc:child".into();
    inner["name"] = "read".into();
    inner["arguments"] = json!({"path":"tools-code-mode"});
    let settle = |mut dispatch: Value| {
        dispatch["isError"] = false.into();
        dispatch["content"] = message["content"].clone();
        dispatch
    };
    let mut first = request(Some("prompt"));
    first["data"]["header"]["tools"] = json!([]);
    first["data"]["header"]["adapterDefaults"] = json!({});
    let mut cleared = request(Some(""));
    cleared["data"]["reason"] = "change".into();
    cleared["data"]["header"]["tools"] = json!([]);
    cleared["data"]["header"]["adapterDefaults"] = json!({});
    let mut replacement = surface("user/message", {
        let mut d = message.clone();
        d["id"] = "tools-ptc:message".into();
        d
    });
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":2,"end":2}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([2]));
    let mut rows = opening();
    rows.extend([
        surface("user/message", message.clone()),
        first,
        ev("tool/code-dispatch-start", outer.clone()),
        ev("tool/code-dispatch-start", inner.clone()),
        ev("tool/code-dispatch", settle(inner)),
        ev("tool/code-dispatch", settle(outer)),
        ev(
            "compaction/prune",
            json!({"shadowedRange":{"start":2,"end":2},"shadowedSeqs":[2],"shadowedTokenCount":17}),
        ),
        replacement,
        cleared,
    ]);
    rows.extend(closing());
    let mut rows = dense(rows);
    for (i, row) in rows.iter_mut().enumerate() {
        row.insert("time".into(), (i + 1).into());
    }
    rows
}
pub fn assert_composite(events: &[SessionFormatEvent]) {
    let systems:Vec<_>=events.iter().filter(|r|r["type"]=="system/message").map(|r|json!({"seq":r["seq"],"surfaceOp":r["surfaceOp"],"sourceEventSeqs":r.get("sourceEventSeqs"),"content":r["data"]["message"]["content"]})).collect();
    assert_eq!(
        systems,
        vec![
            json!({"seq":2,"surfaceOp":"append","sourceEventSeqs":null,"content":[]}),
            json!({"seq":4,"surfaceOp":{"op":"replace","startSeq":2,"endSeq":2},"sourceEventSeqs":[2],"content":[{"type":"text","text":"prompt"}]}),
            json!({"seq":12,"surfaceOp":{"op":"replace","startSeq":4,"endSeq":4},"sourceEventSeqs":[4],"content":[]})
        ]
    );
    let source = composite_source();
    let expected:Vec<_>=source.into_iter().map(|mut row|{let seq=row["seq"].as_u64().unwrap();row.insert("seq".into(),(seq+if seq<2{0}else if seq<3{1}else if seq<10{2}else{3}).into());let kind=row["type"].as_str().unwrap().to_owned();match kind.as_str(){"tool/code-dispatch-start"=>{row.insert("type".into(),"tool/ptc-dispatch-start".into());},"tool/code-dispatch"=>{row.insert("type".into(),"tool/ptc-dispatch".into());},"user/message"=>row["data"]["source"]["plugin"]="tools-ptc".into(),"request/header"=>row["data"]["header"]=json!({"config":{"provider":"mock","model":"mock"}}),"compaction/prune"=>row["data"]=json!({"shadowedRange":{"start":3,"end":3},"shadowedSeqs":[3],"shadowedTokenCount":17}),_=>{}}if seq==9{row.insert("surfaceOp".into(),json!({"op":"replace","startSeq":3,"endSeq":3}));row.insert("sourceEventSeqs".into(),json!([3]));}row}).collect();
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] != "system/message")
            .take(expected.len())
            .cloned()
            .collect::<Vec<_>>(),
        expected
    );
}
