mod common;
use common::*;
#[test]
fn migrates_a_seeded_header_and_every_inherited_local_selection() {
    for version in [0, 1, 2] {
        let mut header = json!({"type":"session","version":version,"id":"code","createdAt":1,"parentSession":"parent","delegationDepth":0,"agentPreset":"code"});
        if version == 2 {
            header["isSeeded"] = true.into();
        } else {
            header["seedLength"] = 2.into();
        }
        let selections = ["code", "standard", "code", "minimal", "code"];
        let mut rows = vec![];
        for (index, preset) in selections.iter().enumerate() {
            if version == 2 && index == 2 {
                let mut marker = e("session/end-seed", 2, json!({"inherited":true}));
                marker.insert("time".into(), 2.into());
                rows.push(marker);
            }
            let seq = index + usize::from(version == 2 && index >= 2);
            let mut row = e(
                "agent-preset/selected",
                seq as u64,
                json!({"agentPreset":preset}),
            );
            row.insert("time".into(), (index + 1).into());
            rows.push(row);
        }
        let before = (header.clone(), rows.clone());
        let a = restore(&rows, &header, SessionFormatValidation::Current).unwrap();
        assert_eq!(a.header["version"], hoh_session::SESSION_FORMAT_VERSION);
        assert_eq!(a.header["id"], "code");
        assert_eq!(a.header["agentPreset"], "ptc");
        assert_eq!(a.header["isSeeded"], true);
        assert_eq!(a.inherited_event_count, 2);
        assert_eq!(
            a.events
                .iter()
                .map(|e| e["seq"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5]
        );
        assert_eq!(
            a.events
                .iter()
                .filter(|e| e["type"] == "agent-preset/selected")
                .map(|e| e["data"].clone())
                .collect::<Vec<_>>(),
            ["ptc", "standard", "ptc", "minimal", "ptc"].map(|p| json!({"agentPreset":p}))
        );
        assert_eq!(a.events[2]["type"], "session/end-seed");
        assert_eq!(a.events[2]["data"], json!({"inherited":true}));
        assert_eq!((header, rows), before);
    }
}
#[test]
fn does_not_reinterpret_a_v3_custom_preset_named_code() {
    let mut header = physical(3, "native");
    header["agentPreset"] = "code".into();
    let row = ev("agent-preset/selected", json!({"agentPreset":"code"}));
    let a = restore(
        std::slice::from_ref(&row),
        &header,
        SessionFormatValidation::Current,
    )
    .unwrap();
    assert_eq!(a.header["agentPreset"], "code");
    assert_eq!(a.events, vec![row]);
}
