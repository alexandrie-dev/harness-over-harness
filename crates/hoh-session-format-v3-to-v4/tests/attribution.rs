mod common;
use common::*;
#[test]
fn preserves_source_metadata_through_decoding_adoption_and_derivation() {
    for role in ["user", "developer"] {
        let message = json!({"id":"attributed-message","role":role,"source":{"kind":"external-attribution","location":{"file":"notes.txt","lines":[2,5]},"enabled":false},"content":[{"type":"text","text":"Retain this context."}]});
        let mut rows = opening();
        rows.push(surface(
            &format!("{role}/message"),
            if role == "user" {
                message.clone()
            } else {
                json!({"turn":1,"step":1,"message":message})
            },
        ));
        rows.extend(closing());
        let rows = numbered(rows, 1);
        let physical: Vec<_> = rows
            .iter()
            .map(|e| {
                hoh_util_values::js_json_stringify(
                    &RELEASED_V4_SESSION_FORMAT_CODEC
                        .encode_event(e)
                        .unwrap()
                        .into(),
                )
            })
            .collect();
        let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
            .create_decoder(
                &RELEASED_V4_SESSION_FORMAT_CODEC
                    .encode_header(&h(4, "unknown-attribution"), 0)
                    .unwrap()
                    .into(),
                SessionFormatRecovery::Strict,
            )
            .unwrap();
        let mut out = SessionFormatEventCollector::new();
        for text in &physical {
            d.decode_row(hoh_util_values::js_json_parse(text).unwrap(), &mut out)
                .unwrap();
        }
        let cut = d.finish(&mut out).unwrap();
        let restored = restore_released_v4_artifact(
            artifact(out.values, h(4, "unknown-attribution"), cut),
            &known(&rows),
        )
        .unwrap();
        assert_eq!(restored.events, rows);
        assert_eq!(
            restored
                .events
                .iter()
                .map(|e| hoh_util_values::js_json_stringify(
                    &RELEASED_V4_SESSION_FORMAT_CODEC
                        .encode_event(e)
                        .unwrap()
                        .into()
                ))
                .collect::<Vec<_>>(),
            physical
        );
        let actual = if role == "user" {
            &restored.events[2]["data"]
        } else {
            &restored.events[2]["data"]["message"]
        };
        assert_eq!(actual, &message);
    }
}
