use hoh_chunked_list::*;
use serde_json::json;
use std::sync::Arc;

fn contents(head: &ChunkedList<i64>) -> Vec<i64> {
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
