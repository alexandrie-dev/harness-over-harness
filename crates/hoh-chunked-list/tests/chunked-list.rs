use hoh_chunked_list::*;
use serde_json::json;
use std::sync::Arc;

fn contents(head: &Arc<ChunkedList<i64>>) -> Vec<i64> {
    iterate_chunked_list(Some(head)).copied().collect()
}
#[test]
fn iterates_empty_list_and_retains_undefined_values() {
    assert_eq!(iterate_chunked_list::<()>(None).count(), 0);
    let head = append_chunked_list(None, None::<()>);
    assert_eq!(
        iterate_chunked_list(Some(&head)).collect::<Vec<_>>(),
        [&None]
    );
}
#[test]
fn keeps_every_prefix_unchanged_across_rollovers_and_divergent_appends() {
    let mut head = None;
    let mut prefixes = Vec::new();
    for n in 0..200 {
        head = Some(append_chunked_list(head.as_ref(), n));
        prefixes.push(head.clone().unwrap());
    }
    for (i, prefix) in prefixes.iter().enumerate() {
        assert_eq!(contents(prefix), (0..=i as i64).collect::<Vec<_>>());
    }
    assert!(Arc::ptr_eq(prefixes[64].previous().unwrap(), &prefixes[63]));
    assert!(Arc::ptr_eq(prefixes[65].previous().unwrap(), &prefixes[63]));
    let branch = append_chunked_list(Some(&prefixes[64]), -1);
    assert_eq!(contents(&branch), (0..65).chain([-1]).collect::<Vec<_>>());
    assert_eq!(contents(&prefixes[65]), (0..66).collect::<Vec<_>>());
}
#[test]
fn retains_stored_objects_by_reference() {
    let value = Arc::new(json!({"id":"first"}));
    let head = append_chunked_list(None, value.clone());
    assert!(Arc::ptr_eq(
        iterate_chunked_list(Some(&head)).next().unwrap(),
        &value
    ));
    let second = append_chunked_list(Some(&head), Arc::new(json!({})));
    assert!(Arc::ptr_eq(&head.values()[0], &second.values()[0]));
}
#[test]
fn restores_checkpoints_and_appends_at_and_after_rollover() {
    let mut head = None;
    for n in 0..130 {
        let next = append_chunked_list(head.as_ref(), n);
        head =
            Some(chunked_list_schema::<i64>(serde_json::to_value(next.as_ref()).unwrap()).unwrap());
    }
    assert_eq!(contents(&head.unwrap()), (0..130).collect::<Vec<_>>());
}
#[test]
fn rejects_every_invalid_checkpoint() {
    for invalid in [
        json!({"values":[]}),
        json!({"values":vec![0;65]}),
        json!({"values":["invalid"]}),
        json!({"values":[0],"extra":true}),
    ] {
        assert!(chunked_list_schema::<i64>(invalid.clone()).is_err());
        assert!(chunked_list_schema::<i64>(json!({"values":[0],"previous":invalid})).is_err());
    }
    assert!(chunked_list_schema::<i64>(json!({"values":[0],"previous":null})).is_err());
}

#[test]
fn rejects_non_objects_at_the_root_and_every_previous_level() {
    for invalid in [
        json!([[0]]),
        json!([ [0], {"values":[1]} ]),
        json!(null),
        json!(true),
        json!(7),
        json!("chunk"),
    ] {
        for depth in 0..4 {
            let mut checkpoint = invalid.clone();
            for _ in 0..depth {
                checkpoint = json!({"values":[0],"previous":checkpoint});
            }
            assert!(chunked_list_schema::<i64>(checkpoint.clone()).is_err());
            assert!(serde_json::from_value::<ChunkedList<i64>>(checkpoint).is_err());
        }
    }
}

#[test]
fn validates_restores_iterates_and_drops_two_hundred_thousand_values() {
    let mut head = None;
    for value in 0..200_000u64 {
        head = Some(append_chunked_list(head.as_ref(), value));
    }
    let mut chunks = Vec::new();
    let mut chunk = head.as_ref();
    while let Some(node) = chunk {
        chunks.push(node);
        chunk = node.previous();
    }
    let mut checkpoint = None;
    for chunk in chunks.into_iter().rev() {
        let mut object = serde_json::Map::new();
        object.insert(
            "values".into(),
            serde_json::Value::Array(chunk.values().iter().map(|value| json!(**value)).collect()),
        );
        if let Some(previous) = checkpoint {
            object.insert("previous".into(), previous);
        }
        checkpoint = Some(serde_json::Value::Object(object));
    }
    let restored = chunked_list_schema::<u64>(checkpoint.unwrap()).unwrap();
    assert!(iterate_chunked_list(head.as_ref()).copied().eq(0..200_000));
    assert!(
        iterate_chunked_list(Some(&restored))
            .copied()
            .eq(0..200_000)
    );
    drop(head);
    drop(restored);
}

#[test]
fn rejects_a_deep_invalid_suffix_and_drops_it_iteratively() {
    let mut checkpoint = json!([[0]]);
    for _ in 0..3125 {
        let mut object = serde_json::Map::new();
        object.insert("values".into(), json!([0]));
        object.insert("previous".into(), checkpoint);
        checkpoint = serde_json::Value::Object(object);
    }
    assert!(chunked_list_schema::<u64>(checkpoint).is_err());
}
