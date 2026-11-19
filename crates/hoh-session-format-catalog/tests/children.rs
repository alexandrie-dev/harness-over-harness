mod common;
use common::*;
fn child(id: &str) -> Value {
    json!({"childId":id,"childCreatedAt":2,"descriptorCount":1,"descriptor":{"version":3,"provider":"spawn","mode":"one-shot"}})
}
#[test]
fn requires_explicit_children_for_historical_bodies_while_retaining_header_and_native_v4_reads() {
    let header = physical(3, "parent");
    let c = session_format_catalog().unwrap();
    assert_eq!(c.read_header(&header).status(), "migration-required");
    err(
        c.create_restore(&header, policy(SessionFormatValidation::Current)),
        "explicit historical child facts",
    );
    assert!(
        create_session_format_catalog_with_children(vec![])
            .unwrap()
            .create_restore(&header, policy(SessionFormatValidation::Current))
            .unwrap()
            .finish()
            .unwrap()
            .events
            .is_empty()
    );
    assert!(
        c.create_restore(
            &physical(4, "parent"),
            policy(SessionFormatValidation::Current)
        )
        .unwrap()
        .finish()
        .unwrap()
        .events
        .is_empty()
    );
}
#[test]
fn isolates_interleaved_restores_and_child_evidence_across_parent_catalogs() {
    let left = create_session_format_catalog_with_children(vec![child("left-child")]).unwrap();
    let mut first = left
        .create_restore(
            &physical(3, "parent"),
            policy(SessionFormatValidation::Current),
        )
        .unwrap();
    let mut second = create_session_format_catalog_with_children(vec![child("right-child")])
        .unwrap()
        .create_restore(
            &physical(3, "other-parent"),
            policy(SessionFormatValidation::Current),
        )
        .unwrap();
    let mut repeated = left
        .create_restore(
            &physical(3, "parent"),
            policy(SessionFormatValidation::Current),
        )
        .unwrap();
    let mut row = ev("feedback/record", json!({"text":"retained"}));
    row.insert("time".into(), 5.into());
    for r in [&mut first, &mut second, &mut repeated] {
        r.decode_row(row.clone().into()).unwrap();
    }
    let right = second.finish().unwrap();
    let result = first.finish().unwrap();
    assert_eq!(
        result.events,
        vec![
            row,
            obj(
                json!({"type":"subagent/catalog","seq":1,"time":5,"data":{"version":0,"childId":"left-child","childCreatedAt":2,"mode":"one-shot"}})
            )
        ]
    );
    assert_eq!(right.events[1]["data"]["childId"], "right-child");
    assert_eq!(repeated.finish().unwrap(), result);
    assert_eq!(reopen(&result).unwrap(), result);
}
