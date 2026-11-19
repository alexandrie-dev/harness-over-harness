mod common;
use common::*;
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::{JsonlCompression, catalog_migration::*};
use serde_json::json;
#[tokio::test]
async fn invalidates_lightweight_historical_revisions_on_child_membership_and_contents() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(compression).await;
        let mut h = serde_json::to_value(meta("parent")).unwrap();
        h["version"] = 3.into();
        f.store_raw(&h, &[]).await;
        let first = f
            .backend
            .stat(&"parent".into(), None)
            .await
            .unwrap()
            .unwrap()
            .revision;
        assert_eq!(
            first,
            f.backend
                .stat(&"parent".into(), None)
                .await
                .unwrap()
                .unwrap()
                .revision
        );
        let mut child = h.clone();
        child["id"] = "child".into();
        child["origin"] = "subagent".into();
        child["parentSession"] = "parent".into();
        f.store_raw(&child, &[]).await;
        let added = f
            .backend
            .stat(&"parent".into(), None)
            .await
            .unwrap()
            .unwrap()
            .revision;
        assert_ne!(first, added);
        assert_eq!(
            added,
            f.backend
                .list(None)
                .await
                .unwrap()
                .into_iter()
                .find(|r| r.header.id.as_str() == "parent")
                .unwrap()
                .revision
        );
        f.store_raw(&child, &[event("feedback/record", 0, json!({}))])
            .await;
        assert_ne!(
            added,
            f.backend
                .stat(&"parent".into(), None)
                .await
                .unwrap()
                .unwrap()
                .revision
        );
        f.finish().await;
    }
}
#[tokio::test]
async fn keeps_current_revisions_independent_of_unrelated_historical_logs_and_write_ownership() {
    let f = Fixture::new(JsonlCompression::None).await;
    f.store(&meta("parent"), &[]).await;
    let first = f
        .backend
        .stat(&"parent".into(), None)
        .await
        .unwrap()
        .unwrap()
        .revision;
    let mut h = serde_json::to_value(meta("child")).unwrap();
    h["version"] = 3.into();
    f.store_raw(&h, &[]).await;
    assert_eq!(
        first,
        f.backend
            .stat(&"parent".into(), None)
            .await
            .unwrap()
            .unwrap()
            .revision
    );
    let w = f
        .backend
        .open(&"parent".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    w.close().await.unwrap();
    assert_eq!(
        first,
        f.backend
            .stat(&"parent".into(), None)
            .await
            .unwrap()
            .unwrap()
            .revision
    );
    f.finish().await;
}
#[tokio::test]
async fn invalidates_historical_revisions_when_children_disappear_or_gain_an_opaque_successor() {
    let f = Fixture::new(JsonlCompression::None).await;
    for id in ["parent", "child"] {
        let mut h = serde_json::to_value(meta(id)).unwrap();
        h["version"] = 3.into();
        f.store_raw(&h, &[]).await;
    }
    let first = f
        .backend
        .stat(&"parent".into(), None)
        .await
        .unwrap()
        .unwrap()
        .revision;
    let path = hoh_session_persistence_jsonl::format::generation_log_path(
        &f.root.path().to_string_lossy(),
        None,
        &"child".into(),
        3,
        JsonlCompression::None,
    )
    .unwrap();
    tokio::fs::remove_file(path).await.unwrap();
    let removed = f
        .backend
        .stat(&"parent".into(), None)
        .await
        .unwrap()
        .unwrap()
        .revision;
    assert_ne!(first, removed);
    f.store_raw(&json!({"id":"child","version":5}), &[]).await;
    assert_ne!(
        removed,
        f.backend
            .stat(&"parent".into(), None)
            .await
            .unwrap()
            .unwrap()
            .revision
    );
    f.finish().await;
}
#[tokio::test]
async fn refuses_publication_when_a_related_child_disappears() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut h = meta("child");
    h.delegation_depth = Some(0);
    h.origin = Some("subagent".into());
    h.parent_session = Some("parent".into());
    let (path, _) = f.store(&h, &[]).await;
    let c = hoh_timeout::AbortController::new();
    let prepared = prepare_catalog_facts(
        &"parent".into(),
        &[CatalogSource {
            header: h.clone(),
            path: path.clone(),
        }],
        JsonlCompression::None,
        &c.signal(),
    )
    .await
    .unwrap();
    tokio::fs::remove_file(&path).await.unwrap();
    assert!(prepared.validate().await.is_err());
    assert!(
        prepare_catalog_facts(
            &"parent".into(),
            &[CatalogSource { header: h, path }],
            JsonlCompression::None,
            &c.signal()
        )
        .await
        .is_err()
    );
    f.finish().await;
}
#[tokio::test]
async fn rejects_changed_header_identities_invalid_source_selection_and_cancellation() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut h = meta("child");
    h.delegation_depth = Some(0);
    h.origin = Some("subagent".into());
    h.parent_session = Some("parent".into());
    let (path, _) = f.store(&h, &[]).await;
    h.id = "wrong-child".into();
    let c = hoh_timeout::AbortController::new();
    fail(
        prepare_catalog_facts(
            &"parent".into(),
            &[CatalogSource {
                header: h.clone(),
                path: path.clone(),
            }],
            JsonlCompression::None,
            &c.signal(),
        )
        .await,
        "changed",
    );
    fail(
        prepare_catalog_facts(
            &"parent".into(),
            &[CatalogSource {
                header: h.clone(),
                path: format!("{path}.foreign"),
            }],
            JsonlCompression::None,
            &c.signal(),
        )
        .await,
        "unrecognized historical child generation",
    );
    c.abort_with(std::io::Error::other("cancelled child collection"));
    fail(
        prepare_catalog_facts(
            &"parent".into(),
            &[CatalogSource { header: h, path }],
            JsonlCompression::None,
            &c.signal(),
        )
        .await,
        "cancelled child collection",
    );
    f.finish().await;
}
