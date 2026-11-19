mod common;
use common::*;
use hoh_session_format::*;
use serde_json::{Value, json};
fn text() -> Value {
    json!({"type":"text","text":"input"})
}
fn future() -> Value {
    json!({"type":"future-content","seq":987,"content":[{"type":"text","text":"opaque"}]})
}
fn nested(content: Value) -> Value {
    json!({"type":"tool-result","toolCallId":"nested","content":content})
}
fn open() -> Vec<SessionFormatEvent> {
    let mut rows = opening();
    let mut u = user("user");
    u["data"]["content"] = json!([text()]);
    rows.push(u);
    rows
}
fn wrap(rows: Vec<SessionFormatEvent>) -> Vec<SessionFormatEvent> {
    let mut output = open();
    output.extend(rows);
    output.extend(closing());
    dense(output)
}
fn carrier(index: usize, content: Value) -> Vec<SessionFormatEvent> {
    match index {
        0 => {
            let mut u = user("second");
            u["data"]["content"] = content;
            vec![u]
        }
        1 => vec![assistant(content, json!([]))],
        2 => {
            let mut result = surface(
                "tool/result",
                json!({"turn":1,"step":1,"message":{"id":"result","role":"user","source":{"kind":"tool","callId":"call"},"content":[{"type":"tool-result","toolCallId":"call","content":content}]}}),
            );
            result.insert("time".into(), 1.into());
            vec![
                assistant(
                    json!([{"type":"tool-call","id":"call","name":"read","arguments":"{}"}]),
                    json!([]),
                ),
                ev(
                    "tool/call",
                    json!({"turn":1,"step":1,"callId":"call","name":"read","arguments":"{}"}),
                ),
                result,
            ]
        }
        3 => {
            let mut message = user_data("inbox");
            message["content"] = content;
            vec![ev(
                "agent/inbox/spliced",
                json!({"target":"next-turn","start":0,"inserted":[message]}),
            )]
        }
        4 => {
            let mut message = user_data("title");
            message["source"] = json!({"kind":"plugin","plugin":"dsh-session-title-llm"});
            message["content"] = content;
            let mut row = title(2, json!([message]));
            row["data"]["maxTokens"] = 987.into();
            vec![row]
        }
        5 => vec![ev(
            "team/message/queued",
            json!({"version":1,"teamId":"team","message":{"id":"queued","senderId":"lead","senderName":"lead","targetId":"worker","delivery":"quiet","content":content}}),
        )],
        6 | 7 => {
            let mut data = json!({"compactionId":"compact","provider":"mock","model":"mock","summary":[text()],"shadowedRange":{"start":2,"end":2},"shadowedSeqs":[2],"shadowedTokenCount":987});
            data[if index == 6 { "summary" } else { "rawOutput" }] = content;
            vec![
                ev(
                    "compaction/start",
                    json!({"compactionId":"compact","turn":1}),
                ),
                ev("compaction/summary", data),
                ev("compaction/end", json!({"compactionId":"compact","turn":1})),
            ]
        }
        8 => {
            let dispatch = json!({"rootCallId":"root","parentCallId":"root","subCallId":"sub","name":"read","arguments":{}});
            let mut settled = dispatch.clone();
            settled["content"] = content;
            settled["isError"] = false.into();
            vec![
                ev("tool/code-dispatch-start", dispatch),
                ev("tool/code-dispatch", settled),
            ]
        }
        _ => unreachable!(),
    }
}
fn info(index: usize) -> (&'static str, &'static str, usize) {
    match index {
        0 => ("user/message", "data.content", 3),
        1 => ("assistant/message", "data.message.content", 3),
        2 => ("tool/result", "data.message.content[0].content", 5),
        3 => ("agent/inbox/spliced", "data.inserted[0].content", 3),
        4 => ("session/title-llm-request", "data.messages[0].content", 3),
        5 => ("team/message/queued", "data.message.content", 3),
        6 => ("compaction/summary", "data.summary", 4),
        7 => ("compaction/summary", "data.rawOutput", 4),
        8 => ("tool/code-dispatch", "data.content", 4),
        _ => unreachable!(),
    }
}
fn admitted() -> Vec<Value> {
    vec![
        text(),
        json!({"type":"reasoning","text":""}),
        json!({"type":"image","attachment":{"attachmentId":"image","mediaType":"image/png","bytes":987,"width":1,"height":2,"name":"","originalDimensions":{"width":3,"height":4}}}),
        json!({"type":"file","attachment":{"attachmentId":"file","name":"","bytes":987}}),
        json!({"type":"tool-call","id":"opaque-call","name":"read","arguments":"{\"type\":\"future-content\",\"seq\":987}"}),
        json!({"type":"tool-result","toolCallId":"nested","content":[text()],"isError":false}),
    ]
}
fn malformed() -> Vec<(Value, &'static str)> {
    vec![
        (json!({"type":"text","text":12}), "text"),
        (
            json!({"type":"reasoning","text":"ok","extra":true}),
            "reasoning",
        ),
        (json!({"type":"image","attachment":{}}), "image"),
        (
            json!({"type":"file","attachment":{"attachmentId":"file","name":"file","bytes":-1}}),
            "file",
        ),
        (
            json!({"type":"tool-call","id":"","name":"read","arguments":"{}"}),
            "tool-call",
        ),
        (
            json!({"type":"tool-result","toolCallId":"","content":[]}),
            "tool-result",
        ),
        (
            json!({"type":"tool-result","toolCallId":"call","content":null}),
            "tool-result",
        ),
    ]
}
fn historical(
    rows: Vec<SessionFormatEvent>,
    version: u64,
    validation: SessionFormatValidation,
) -> R<SessionFormatArtifact> {
    let header = if version == 3 {
        physical(h(3, "identity"))
    } else {
        json!({"type":"session","version":version,"id":"identity","createdAt":1,"delegationDepth":0})
    };
    read(
        header,
        dense(rows),
        SessionFormatRecovery::Strict,
        validation,
    )
}
#[test]
fn preserves_opaque_json_serialized_arguments_compact_runs_and_non_content_counters_exactly() {
    let mut opaque = future();
    opaque["blockType"] = "future-content".into();
    opaque["block"] = future();
    opaque["blocks"] = json!([future()]);
    opaque["summary"] = json!([future()]);
    opaque["message"] = json!({"content":[future()]});
    let replay = json!({"response":opaque,"blocks":[opaque,opaque]});
    let serialized = hoh_util_values::js_json_stringify(&opaque);
    let stream = json!([{"type":"text-chunks","time0":1,"index":0,"dt":[0],"texts":[serialized,""]},{"type":"tool-call-chunks","time0":1,"index":1,"id":"opaque-call","name":"read","dt":[],"args":[serialized]},{"type":"chunk","time":1,"chunk":{"type":"finish","reason":{"kind":"tool-calls"},"replayState":replay}}]);
    let dispatch = json!({"rootCallId":"root","parentCallId":"root","subCallId":"sub","name":"read","arguments":opaque});
    let mut settled = dispatch.clone();
    settled["content"] = json!([text()]);
    settled["isError"] = false.into();
    let mut message = assistant(
        json!([{"type":"text","text":serialized},{"type":"tool-call","id":"opaque-call","name":"read","arguments":serialized}]),
        stream,
    );
    message["data"]["message"]["source"]["replayState"] = replay;
    let rows = wrap(vec![
        ev("tool/code-dispatch-start", dispatch),
        ev("tool/code-dispatch", settled),
        message,
        ev(
            "tool/call",
            json!({"turn":1,"step":1,"callId":"opaque-call","name":"read","arguments":serialized}),
        ),
        surface(
            "tool/result",
            json!({"turn":1,"step":1,"message":{"id":"result","role":"user","source":{"kind":"tool","callId":"opaque-call"},"content":[{"type":"tool-result","toolCallId":"opaque-call","content":[text()]}]}}),
        ),
    ]);
    let before = json!(rows);
    let a = migrate(rows.clone()).unwrap();
    restore(a.clone()).unwrap();
    assert_eq!(
        a.events
            .iter()
            .filter(|r| [
                "tool/ptc-dispatch-start",
                "tool/ptc-dispatch",
                "assistant/message"
            ]
            .contains(&r["type"].as_str().unwrap()))
            .map(|r| r["data"].clone())
            .collect::<Vec<_>>(),
        rows[3..6]
            .iter()
            .map(|r| r["data"].clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(json!(rows), before);
}
#[test]
fn routes_historical_queued_content_through_v2_admission() {
    for version in [0, 1] {
        assert_eq!(
            historical(
                wrap(carrier(5, json!([text()]))),
                version,
                SessionFormatValidation::Current
            )
            .unwrap()
            .header["version"],
            3
        );
        err(
            historical(
                wrap(carrier(5, json!([nested(json!([future()]))]))),
                version,
                SessionFormatValidation::Current,
            ),
            "format v2 team/message/queued at seq 3 data.message.content[0].content[0]",
        );
    }
}
#[test]
fn routes_historical_chunk_starts_and_ends_through_v2_admission() {
    for version in [0, 1] {
        for chunk in [
            json!({"type":"block-start","index":987,"blockType":"text"}),
            json!({"type":"block-end","index":987,"block":text()}),
        ] {
            let mut rows = open();
            rows.push(ev(
                "assistant/chunk",
                json!({"turn":1,"step":1,"chunk":chunk}),
            ));
            assert_eq!(
                historical(rows, version, SessionFormatValidation::Current)
                    .unwrap()
                    .header["version"],
                3
            );
        }
        for chunk in [
            json!({"type":"block-start","index":987,"blockType":"future-content"}),
            json!({"type":"block-end","index":987,"block":nested(json!([future()]))}),
        ] {
            let mut rows = open();
            rows.push(ev(
                "assistant/chunk",
                json!({"turn":1,"step":1,"chunk":chunk}),
            ));
            err(
                historical(rows, version, SessionFormatValidation::Current),
                "format v2 assistant/attempt at seq 3 data.stream[0].chunk",
            );
        }
    }
}
fn stream_event(kind: &str, stream: Value, content: Value) -> SessionFormatEvent {
    if kind == "assistant/message" {
        assistant(content, stream)
    } else {
        ev(kind, json!({"turn":1,"step":1,"stream":stream}))
    }
}
#[test]
fn keeps_native_v3_extension_acceptance_in_content_and_raw_stream_records() {
    for validation in [
        SessionFormatValidation::Current,
        SessionFormatValidation::Transformed,
    ] {
        let base = migrate(wrap(vec![])).unwrap();
        let mut body = base.events[..base.events.len() - 2].to_vec();
        let mut extension = user("extension");
        extension["data"]["content"] = json!([future(), nested(json!([future()]))]);
        let mut extensions = vec![extension];
        extensions.extend(carrier(5, json!([future()])));
        extensions.push(assistant(json!([text()]),json!([{"type":"chunk","time":987,"chunk":{"type":"block-end","index":987,"block":future()}}])));
        extensions.push(stream_event("assistant/attempt",json!([{"type":"chunk","time":987,"chunk":{"type":"block-start","index":987,"blockType":"future-content"}}]),json!([])));
        let size = body.len();
        body.extend(extensions.clone());
        body.extend(closing());
        let a = historical(body, 3, validation).unwrap();
        assert_eq!(
            a.events[size..a.events.len() - 2]
                .iter()
                .map(|r| r["data"].clone())
                .collect::<Vec<_>>(),
            extensions
                .iter()
                .map(|r| r["data"].clone())
                .collect::<Vec<_>>()
        );
    }
}
#[test]
fn accepts_empty_arrays_in_all_admitted_carriers() {
    for index in (0..9).filter(|i| *i != 4) {
        restore(migrate(wrap(carrier(index, json!([])))).unwrap()).unwrap();
    }
}
macro_rules! carrier_tests{($module:ident,$index:expr)=>{mod $module{use super::*;
#[test]fn accepts_valid_historical_content(){restore(migrate(wrap(carrier($index,json!([text()])))).unwrap()).unwrap();}
#[test]fn preserves_valid_content_without_mutation(){let content=if $index==4{json!([text()])}else{let mut content=vec![nested(json!(admitted()))];content.extend(admitted().into_iter().filter(|b|b["type"]!="tool-call"));json!(content)};let input=wrap(carrier($index,content));let before=json!(input);let a=migrate(input.clone()).unwrap();restore(a.clone()).unwrap();assert_eq!(json!(input),before);let(kind,_,seq)=info($index);let row=a.events.iter().find(|r|r["type"]==if kind=="tool/code-dispatch"{"tool/ptc-dispatch"}else{kind}&&r["seq"]==json!(seq+1)).unwrap();let mut expected=input[seq]["data"].clone();if kind=="compaction/summary"{expected["shadowedRange"]=json!({"start":3,"end":3});expected["shadowedSeqs"]=json!([3]);}else if kind=="session/title-llm-request"{expected["messageSeqs"]=json!([3]);}assert_eq!(row["data"],expected);}
#[test]fn rejects_malformed_owned_content_kinds(){let(kind,path,seq)=info($index);for(block,name)in malformed(){let input=wrap(carrier($index,json!([nested(json!([block]))])));let before=json!(input);let error=migrate(input.clone()).unwrap_err().to_string();assert!(error.contains(&format!("format v2 {kind} at seq {seq} {path}[0].content[0]")),"{error}");assert!(error.contains(&format!("kind \"{name}\"")),"{error}");assert_eq!(json!(input),before);}}
#[test]fn rejects_malformed_content_arrays(){let(kind,path,seq)=info($index);for content in [Value::Null,json!(false),json!({}),json!([null]),json!([nested(Value::Null)])]{err(migrate(wrap(carrier($index,content))),&format!("format v2 {kind} at seq {seq} {path}"));}}
#[test]fn rejects_unknown_nested_and_direct_content_kinds(){let(kind,path,seq)=info($index);for deep in [false,true]{let content=if deep{json!([text(),nested(json!([text(),nested(json!([future()]))]))])}else{json!([future()])};let suffix=if deep{"[1].content[1].content[0]"}else{"[0]"};err(migrate(wrap(carrier($index,content))),&format!("format v2 {kind} at seq {seq} {path}{suffix}: cannot safely transform unclassified message content kind \"future-content\""));}}
}};}
carrier_tests!(user_message, 0);
carrier_tests!(assistant_message, 1);
carrier_tests!(tool_result, 2);
carrier_tests!(inbox, 3);
carrier_tests!(title_request, 4);
carrier_tests!(team_queued, 5);
carrier_tests!(compaction_summary, 6);
carrier_tests!(compaction_raw_output, 7);
carrier_tests!(code_dispatch, 8);
macro_rules! stream_tests{($module:ident,$kind:expr)=>{mod $module{use super::*;
fn input(stream:Value)->Vec<SessionFormatEvent>{let mut rows=open();rows.push(stream_event($kind,stream,json!([text()])));rows}
#[test]fn rejects_malformed_raw_block_end_content_kinds(){for(block,kind)in malformed(){let error=migrate(input(json!([{"type":"chunk","time":4,"chunk":{"type":"block-end","index":987,"block":nested(json!([block]))}}]))).unwrap_err().to_string();assert!(error.contains(&format!("format v2 {} at seq 3 data.stream[0].chunk.block.content[0]",$kind)),"{error}");assert!(error.contains(&format!("kind \"{kind}\"")),"{error}");}}
#[test]fn rejects_missing_or_malformed_block_start_kinds(){for fields in [json!({}),json!({"blockType":null}),json!({"blockType":1}),json!({"blockType":""})]{let mut chunk=json!({"type":"block-start","index":987});chunk.as_object_mut().unwrap().extend(obj(fields));err(migrate(input(json!([{"type":"chunk","time":4,"chunk":chunk}]))),&format!("format v2 {} at seq 3 data.stream[0].chunk.blockType: cannot safely transform unclassified message content kind",$kind));}}
#[test]fn narrows_owned_durable_stream_containers(){for stream in [Value::Null,json!({}),json!([null]),json!([{"type":"chunk"}]),json!([{"type":"chunk","chunk":[]}]),json!([{"type":"chunk","chunk":{"type":"block-end"}}])]{err(migrate(input(stream)),&format!("format v2 {} at seq 3 data.stream",$kind));}}
#[test]fn preserves_admitted_raw_block_kinds(){for block in admitted(){let stream=json!([{"type":"chunk","time":4,"chunk":{"type":"block-start","index":987,"blockType":block["type"]}},{"type":"chunk","time":5,"chunk":{"type":"block-end","index":987,"block":block}}]);let mut rows=open();rows.push(stream_event($kind,stream,json!([block])));if $kind=="assistant/message"&&block["type"]=="tool-call"{rows.push(ev("tool/call",json!({"turn":1,"step":1,"callId":"opaque-call","name":"read","arguments":"{\"type\":\"future-content\",\"seq\":987}"})));rows.push(surface("tool/result",json!({"turn":1,"step":1,"message":{"id":"result","role":"user","source":{"kind":"tool","callId":"opaque-call"},"content":[{"type":"tool-result","toolCallId":"opaque-call","content":[text()]}]}})));}rows.extend(closing());let before=json!(rows);let a=migrate(rows.clone()).unwrap();restore(a.clone()).unwrap();assert_eq!(a.events.iter().find(|r|r["type"]==$kind).unwrap()["data"],rows[3]["data"]);assert_eq!(json!(rows),before);}}
#[test]fn accepts_historical_raw_block_chunks(){let mut rows=input(json!([{"type":"chunk","time":4,"chunk":{"type":"block-start","index":987,"blockType":"text"}},{"type":"chunk","time":5,"chunk":{"type":"block-end","index":987,"block":text()}}]));rows.extend(closing());restore(migrate(rows).unwrap()).unwrap();}
#[test]fn rejects_raw_block_end_unknown_kinds_nested_or_direct(){for deep in [false,true]{let block=if deep{nested(json!([future()]))}else{future()};let mut rows=input(json!([{"type":"chunk","time":4,"chunk":{"type":"block-end","index":987,"block":block}}]));rows.extend(closing());err(migrate(rows),&format!("format v2 {} at seq 3 data.stream[0].chunk.block{}: cannot safely transform unclassified message content kind \"future-content\"",$kind,if deep{".content[0]"}else{""}));}}
#[test]fn rejects_raw_block_start_unknown_kind_without_an_end(){err(migrate(input(json!([{"type":"chunk","time":4,"chunk":{"type":"block-start","index":987,"blockType":"future-content"}}]))),&format!("format v2 {} at seq 3 data.stream[0].chunk.blockType: cannot safely transform unclassified message content kind \"future-content\"",$kind));}
}};}
stream_tests!(assistant_stream, "assistant/message");
stream_tests!(attempt_stream, "assistant/attempt");
