use hoh_llm::*;
use serde_json::{Value, json};

#[test]
fn raw_chunks_are_snapshotted_without_field_validation() {
    let chunks = [
        json!({"type":"block-start"}),
        json!({"type":"block-start","index":-1,"blockType":false}),
        json!({"type":"block-end"}),
        json!({"type":"block-end","index":"bad","block":null}),
        json!({"type":"usage"}),
        json!({"type":"usage","usage":{"inputTokens":"bad","cost":0.5}}),
        json!({"type":"finish"}),
        json!({"type":"finish","reason":false,"replayState":[]}),
    ];
    let mut accumulator = AssistantStreamAccumulator::new();
    let mut errors = vec![];
    for (time, chunk) in chunks.iter().enumerate() {
        let input = json!({"time":time,"chunk":chunk});
        match accumulator.push_value(&input) {
            Ok(returned) => assert_eq!(serde_json::to_value(returned).unwrap(), input),
            Err(error) => errors.push(format!("{chunk}: {error}")),
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert_eq!(
        serde_json::to_value(accumulator.snapshot()).unwrap(),
        Value::Array(
            chunks
                .into_iter()
                .enumerate()
                .map(|(time, chunk)| json!({"type":"chunk","time":time,"chunk":chunk}))
                .collect()
        )
    );
}

#[test]
fn json_snapshot_precedes_all_chunk_field_checks() {
    let mut failures = vec![];
    for kind in [
        "block-start",
        "block-end",
        "usage",
        "finish",
        "text-delta",
        "reasoning-delta",
        "tool-call-delta",
        "future",
    ] {
        let mut accumulator = AssistantStreamAccumulator::new();
        let input = json!({"time":1,"chunk":{"type":kind,"index":-1,"id":false,"name":false,"argumentsDelta":false,"text":false,"extension":-0.0}});
        let got = accumulator.push_value(&input).unwrap_err().to_string();
        if got != "Assistant stream chunk must be losslessly JSON-serializable" {
            failures.push(format!("{kind}: {got}"));
        }
        assert!(accumulator.snapshot().is_empty());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn tool_delta_validation_is_index_then_id_then_name_then_arguments() {
    let mut failures = vec![];
    for (chunk, expected) in [
        (
            json!({"type":"tool-call-delta","index":-1,"id":false,"name":false,"argumentsDelta":false}),
            "tool-call-delta index must be a non-negative safe integer",
        ),
        (
            json!({"type":"tool-call-delta","index":0,"id":false,"name":false,"argumentsDelta":false}),
            "tool-call-delta id must be a string",
        ),
        (
            json!({"type":"tool-call-delta","index":0,"id":"x","name":false,"argumentsDelta":false}),
            "tool-call-delta name must be a string",
        ),
        (
            json!({"type":"tool-call-delta","index":0,"id":"x","name":null}),
            "tool-call-delta name must be a string",
        ),
        (
            json!({"type":"tool-call-delta","index":0,"id":"x"}),
            "tool-call-delta argumentsDelta must be a string",
        ),
        (
            json!({"type":"tool-call-delta","index":0,"id":"","name":false,"argumentsDelta":false}),
            "tool-call-delta name must be a string",
        ),
    ] {
        let mut accumulator = AssistantStreamAccumulator::new();
        let got = accumulator
            .push_value(&json!({"time":1,"chunk":chunk}))
            .unwrap_err()
            .to_string();
        if got != expected {
            failures.push(format!("{chunk}: expected {expected}; got {got}"));
        }
        assert!(accumulator.snapshot().is_empty());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn source_number_indexes_and_rejected_chunks_preserve_packing() {
    let mut accumulator = AssistantStreamAccumulator::new();
    accumulator
        .push_value(&json!({"time":1,"chunk":{"type":"text-delta","index":1.0,"text":"a"}}))
        .unwrap();
    assert!(accumulator.push_value(&json!({"time":2,"chunk":{"type":"tool-call-delta","index":0,"id":"x","name":false}})).is_err());
    accumulator
        .push_value(&json!({"time":3,"chunk":{"type":"text-delta","index":1,"text":"b"}}))
        .unwrap();
    assert_eq!(
        serde_json::to_value(accumulator.snapshot()).unwrap(),
        json!([{"type":"text-chunks","time0":1,"index":1,"dt":[2],"texts":["a","b"]}])
    );
}

#[test]
fn time_precedes_snapshot_and_null_reaches_property_access() {
    let mut accumulator = AssistantStreamAccumulator::new();
    assert_eq!(
        accumulator
            .push_value(&json!({"time":0.5,"chunk":{"extension":-0.0}}))
            .unwrap_err()
            .to_string(),
        "Assistant stream time must be a safe integer, got 0.5"
    );
    assert_eq!(
        accumulator
            .push_value(&json!({"time":1,"chunk":null}))
            .unwrap_err()
            .to_string(),
        "Cannot read properties of null (reading 'type')"
    );
}

#[test]
fn missing_time_uses_undefined_in_the_source_diagnostic() {
    assert_eq!(
        AssistantStreamAccumulator::new()
            .push_value(&json!({"chunk":{"type":"finish"}}))
            .unwrap_err()
            .to_string(),
        "Assistant stream time must be a safe integer, got undefined"
    );
}
