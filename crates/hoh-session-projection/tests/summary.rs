use crate::{test_support::*, *};
use serde_json::{Value, json};
use std::sync::Arc;

fn kept_unit() -> ProjectionDefinition<Value> {
    let schema: ProjectionViewSchema = Arc::new(|value| match value {
        Some(Value::Null) => Ok(Arc::new(Value::Null)),
        Some(value) if value.get("text").is_some_and(Value::is_string) => {
            Ok(Arc::new(json!({"text":value["text"]})))
        }
        _ => Err(error("invalid summary text")),
    });
    let state_schema = schema.clone();
    ProjectionDefinition {
        key: "test/summary-kept".into(),
        state_version: 1,
        state_schema: Arc::new(move |value| state_schema(value)),
        init: Arc::new(|_, _| Ok(Arc::new(Value::Null))),
        apply: Arc::new(|state, event| {
            if event.event_type() == "turn/start" {
                Ok(Arc::new(
                    json!({"text":format!("turn-{}",hoh_util_values::js_string(&event.data()["turn"]))}),
                ))
            } else {
                Ok(state.clone())
            }
        }),
        wire: Some(ProjectionWire::new(
            schema,
            Arc::new(|state| Ok(Some(state.clone()))),
        )),
    }
}
fn excluded_unit() -> ProjectionDefinition<u64> {
    let mut unit = reader_unit();
    unit.key = "test/summary-excluded".into();
    unit.wire.as_mut().unwrap().summary = false;
    unit
}
fn nullable_unit() -> ProjectionDefinition<Value> {
    let schema: ProjectionViewSchema = Arc::new(|value| match value {
        Some(Value::Null) => Ok(Arc::new(Value::Null)),
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
            Ok(Arc::new(Value::Array(items.clone())))
        }
        _ => Err(error("invalid nullable string array")),
    });
    let state_schema = schema.clone();
    let mut wire = ProjectionWire::new(
        schema,
        Arc::new(|state: &Arc<Value>| Ok(Some(state.clone()))),
    );
    wire.summary = false;
    ProjectionDefinition {
        key: "test/summary-excluded-nullable".into(),
        state_version: 1,
        state_schema: Arc::new(move |value| state_schema(value)),
        init: Arc::new(|_, _| Ok(Arc::new(Value::Null))),
        apply: Arc::new(|state, event| {
            if event.event_type() == "turn/start" {
                Ok(Arc::new(json!([format!(
                    "turn-{}",
                    hoh_util_values::js_string(&event.data()["turn"])
                )])))
            } else {
                Ok(state.clone())
            }
        }),
        wire: Some(wire),
    }
}
#[tokio::test]
async fn derives_the_summary_set_from_the_registered_units_and_their_marks() {
    let h = harness().await;
    h.registry.register(&h.ctx, kept_unit()).await.unwrap();
    h.registry.register(&h.ctx, excluded_unit()).await.unwrap();
    h.registry.register(&h.ctx, nullable_unit()).await.unwrap();
    let keys = h.registry.summary_keys();
    assert_eq!(
        keys.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["test/summary-kept"]
    );
    assert!(!keys.contains("test/summary-excluded"));
    assert!(Arc::ptr_eq(&keys, &h.registry.summary_keys()));
    h.close().await;
}
#[tokio::test]
async fn refuses_a_second_registration_of_one_key_with_a_different_summary_membership() {
    let h = harness().await;
    h.registry.register(&h.ctx, kept_unit()).await.unwrap();
    let mut unit = kept_unit();
    unit.wire.as_mut().unwrap().summary = false;
    let error = h.registry.register(&h.ctx, unit).await.err().unwrap();
    assert!(
        error
            .to_string()
            .contains("test/summary-kept\" is already registered with summary=true")
    );
    h.close().await;
}
#[tokio::test]
async fn rebuilds_the_summary_set_only_when_a_registration_or_its_disposal_changes_the_key_table() {
    let h = harness().await;
    let empty = h.registry.summary_keys();
    assert!(empty.is_empty());
    let (fiber, ctx) = owner(&h.ctx).await;
    h.registry.register(&ctx, kept_unit()).await.unwrap();
    let registered = h.registry.summary_keys();
    assert!(!Arc::ptr_eq(&empty, &registered));
    assert_eq!(registered.len(), 1);
    assert!(Arc::ptr_eq(&registered, &h.registry.summary_keys()));
    fiber.dispose().await;
    let disposed = h.registry.summary_keys();
    assert!(!Arc::ptr_eq(&registered, &disposed));
    assert!(disposed.is_empty());
    h.close().await;
}
#[tokio::test]
async fn serves_a_summary_shaped_block_through_the_key_taking_checkpoint_face() {
    let h = harness().await;
    h.registry.register(&h.ctx, kept_unit()).await.unwrap();
    h.registry.register(&h.ctx, excluded_unit()).await.unwrap();
    h.registry.register(&h.ctx, nullable_unit()).await.unwrap();
    let checkpoint = rows(vec![
        ("test/summary-kept", row(1, 3, json!({"text":"kept"}))),
        ("test/summary-excluded", row(1, 4, json!(7))),
        ("test/summary-excluded-nullable", row(1, 5, json!(["a"]))),
    ]);
    let keys = h.registry.summary_keys();
    assert_eq!(
        json(
            &h.registry
                .view_checkpoint(&checkpoint, selected(&keys))
                .unwrap()
        ),
        json!({"test/summary-kept":{"text":"kept"}})
    );
    assert_eq!(
        json(&h.registry.view_checkpoint(&checkpoint, None).unwrap()),
        json!({"test/summary-kept":{"text":"kept"},"test/summary-excluded":7,"test/summary-excluded-nullable":["a"]})
    );
    h.close().await;
}
#[tokio::test]
async fn serves_cached_summary_cells_through_the_key_taking_cached_face() {
    let h = harness().await;
    h.registry.register(&h.ctx, kept_unit()).await.unwrap();
    h.registry.register(&h.ctx, excluded_unit()).await.unwrap();
    let session = h.another("summary-cached");
    turn(&session, 1);
    let keys = h.registry.summary_keys();
    assert_eq!(
        values(
            h.registry
                .cached_snapshot(&session, selected(&keys))
                .unwrap()
                .unwrap()
        ),
        json!({"test/summary-kept":{"text":"turn-1"}})
    );
    assert_eq!(
        values(h.registry.cached_snapshot(&session, None).unwrap().unwrap()),
        json!({"test/summary-kept":{"text":"turn-1"},"test/summary-excluded":1})
    );
    h.close().await;
}
#[tokio::test]
async fn reports_an_empty_summary_set_while_the_complete_face_still_serves_only_marked_units() {
    let h = harness().await;
    h.registry.register(&h.ctx, excluded_unit()).await.unwrap();
    h.registry.register(&h.ctx, nullable_unit()).await.unwrap();
    let session = h.another("summary-empty");
    turn(&session, 1);
    assert!(h.registry.summary_keys().is_empty());
    let empty = Vec::new();
    assert!(
        h.registry
            .cached_snapshot(&session, Some(ProjectionKeySelection::List(&empty)))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        values(h.registry.cached_snapshot(&session, None).unwrap().unwrap()),
        json!({"test/summary-excluded":1,"test/summary-excluded-nullable":["turn-1"]})
    );
    h.close().await;
}
#[tokio::test]
async fn keeps_the_marked_nullable_entry_nullable_through_the_mark() {
    let h = harness().await;
    h.registry.register(&h.ctx, nullable_unit()).await.unwrap();
    let session = h.another("summary-nullable");
    assert_eq!(
        *h.registry.snapshot(&session, None).unwrap().values["test/summary-excluded-nullable"],
        Value::Null
    );
    h.close().await;
}
#[tokio::test]
async fn drops_an_unmaterialized_cell_and_a_version_mismatched_checkpoint_row() {
    let h = harness().await;
    h.registry.register(&h.ctx, kept_unit()).await.unwrap();
    let session = h.another("summary-unmaterialized");
    let keys = h.registry.summary_keys();
    assert_eq!(
        values(
            h.registry
                .cached_snapshot(&session, selected(&keys))
                .unwrap()
                .unwrap()
        ),
        json!({"test/summary-kept":null})
    );
    let checkpoint = rows(vec![(
        "test/summary-kept",
        row(2, 1, json!({"text":"stale"})),
    )]);
    assert!(
        h.registry
            .view_checkpoint(&checkpoint, selected(&keys))
            .unwrap()
            .is_empty()
    );
    h.close().await;
}
