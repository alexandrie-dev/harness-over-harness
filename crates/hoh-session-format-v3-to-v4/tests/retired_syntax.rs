mod common;
use common::*;
#[test]
fn rejects_retired_headers_and_ptc_tags_in_strict_reads_encoding_and_recoverable_suffixes() {
    let invalid = vec![
        e("request/header", 1, Value::Null),
        e("request/header", 1, json!({"header":null})),
        e(
            "request/header",
            1,
            json!({"header":{"config":{"provider":"mock","model":"mock"},"system":""}}),
        ),
        e("tool/code-dispatch-start", 1, json!({})),
        e("tool/code-dispatch", 1, json!({})),
    ];
    for row in invalid {
        assert!(RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row).is_err());
        for recovery in [
            SessionFormatRecovery::Strict,
            SessionFormatRecovery::Recoverable,
        ] {
            let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
                .create_decoder(
                    &RELEASED_V4_SESSION_FORMAT_CODEC
                        .encode_header(&h(4, "native-syntax"), 0)
                        .unwrap()
                        .into(),
                    recovery,
                )
                .unwrap();
            let mut out = SessionFormatEventCollector::new();
            if recovery == SessionFormatRecovery::Strict {
                d.decode_row(opening()[0].clone().into(), &mut out).unwrap();
            } else {
                d.decode_row(Value::Null, &mut out).unwrap();
            }
            assert!(d.decode_row(row.clone().into(), &mut out).is_err());
        }
        assert!(native(vec![opening()[0].clone(), row]).is_err());
    }
}
#[test]
fn keeps_known_request_fields_open_and_obsolete_ignorable_tags_opaque() {
    let mut req = request();
    req["data"]["header"]["extra"] = true.into();
    let mut old = ev("tool/code-dispatch-start", json!({"unknown":true}));
    old.insert("ignorable".into(), true.into());
    let mut end = ev("tool/code-dispatch", Value::Null);
    end.insert("ignorable".into(), true.into());
    let rows = dense(vec![opening()[0].clone(), req, old, end]);
    let (decoded, _) = round_trip(&rows, &h(4, "native-syntax"), 0).unwrap();
    assert_eq!(decoded, rows);
    let k = ["turn/start", "request/header"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        restore_released_v4_artifact(artifact(decoded, h(4, "native-syntax"), 0), &k)
            .unwrap()
            .events,
        rows
    );
}
