use hoh_subprocess::*;
use hoh_util_values::js_json_stringify;
use serde::Serialize;
use serde_json::{Value, json};

fn text(value: &impl Serialize) -> String {
    js_json_stringify(&serde_json::to_value(value).unwrap())
}

#[test]
fn preserves_stdio_unions_field_names_and_key_order() {
    let stdio = SubprocessStdio {
        stdin: SubprocessStdinMode::Data {
            data: "π\n".into()
        },
        stdout: SubprocessOutputMode::Collect(SubprocessCollect {
            max_bytes: 123,
            spill: Some(SubprocessSpill { max_bytes: 456 }),
        }),
        stderr: SubprocessOutputMode::Inherit,
        control: Some(SubprocessControlMode::Pipe),
    };
    assert_eq!(
        text(&stdio),
        "{\"stdin\":{\"data\":\"π\\n\"},\"stdout\":{\"maxBytes\":123,\"spill\":{\"maxBytes\":456}},\"stderr\":\"inherit\",\"control\":\"pipe\"}"
    );
    let round_trip: SubprocessStdio =
        serde_json::from_value(serde_json::to_value(&stdio).unwrap()).unwrap();
    assert_eq!(round_trip, stdio);
    for (mode, expected) in [
        (SubprocessStdinMode::Ignore, "\"ignore\""),
        (SubprocessStdinMode::Pipe, "\"pipe\""),
    ] {
        assert_eq!(text(&mode), expected);
    }
    assert_eq!(text(&SubprocessOutputMode::Pipe), "\"pipe\"");
}

#[test]
fn keeps_null_exit_facts_and_omits_absent_recovery_fields() {
    assert_eq!(
        text(&SubprocessOutcome {
            exit_code: Some(0),
            signal: None
        }),
        "{\"exitCode\":0,\"signal\":null}"
    );
    assert_eq!(
        text(&SubprocessOutcome {
            exit_code: None,
            signal: Some("SIGTERM".into())
        }),
        "{\"exitCode\":null,\"signal\":\"SIGTERM\"}"
    );
    assert_eq!(
        text(&CollectedOutput {
            text: "tail".into(),
            truncated: true,
            spill_path: Some("/spill".into())
        }),
        "{\"text\":\"tail\",\"truncated\":true,\"spillPath\":\"/spill\"}"
    );
    assert_eq!(
        text(&SubprocessOutputRead {
            text: "tail".into(),
            next_offset: 50,
            lossy: false,
            spill_path: None
        }),
        "{\"text\":\"tail\",\"nextOffset\":50,\"lossy\":false}"
    );
    assert_eq!(
        text(&SubprocessTerminalEnvironment {
            platform: SubprocessPlatform::Windows,
            default_shell: None
        }),
        "{\"platform\":\"windows\"}"
    );
    for (signal, expected) in [
        (SubprocessTerminalSignal::Sigint, "SIGINT"),
        (SubprocessTerminalSignal::Sigterm, "SIGTERM"),
        (SubprocessTerminalSignal::Sigkill, "SIGKILL"),
        (SubprocessTerminalSignal::Sigtstp, "SIGTSTP"),
        (SubprocessTerminalSignal::Sighup, "SIGHUP"),
    ] {
        assert_eq!(
            serde_json::to_value(signal).unwrap(),
            Value::String(expected.into())
        );
    }
}

#[test]
fn serializes_undefined_environment_entries_as_absent_without_changing_runtime_tombstones() {
    let spec = SubprocessSpawnSpec {
        argv: vec!["program".into()],
        cwd: "/work".into(),
        stdio: SubprocessStdio {
            stdin: SubprocessStdinMode::Ignore,
            stdout: SubprocessOutputMode::Pipe,
            stderr: SubprocessOutputMode::Inherit,
            control: None,
        },
        grace_ms: 1.0,
        signal: None,
        env: Some(
            [
                ("KEEP".into(), Some("value".into())),
                ("REMOVE".into(), None),
            ]
            .into_iter()
            .collect(),
        ),
    };
    assert_eq!(
        serde_json::to_value(&spec).unwrap()["env"],
        json!({"KEEP": "value"})
    );
    assert_eq!(spec.env.as_ref().unwrap().get("REMOVE"), Some(&None));
    assert_eq!(
        text(&spec),
        "{\"argv\":[\"program\"],\"cwd\":\"/work\",\"stdio\":{\"stdin\":\"ignore\",\"stdout\":\"pipe\",\"stderr\":\"inherit\"},\"graceMs\":1,\"env\":{\"KEEP\":\"value\"}}"
    );
}
