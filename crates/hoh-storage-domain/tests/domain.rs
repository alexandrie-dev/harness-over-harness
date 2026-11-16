mod helpers;
use helpers::*;
use hoh_error::SharedError;
use hoh_kernel::{Kernel, service};
use hoh_storage::{Storage, StorageBackend, StorageError, StorageErrorCode, StoragePlugin};
use hoh_storage_domain::*;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, atomic::Ordering};
service!(MemoryReady:dyn StorageBackend="storage.backend.memory");
service!(OtherReady:dyn StorageBackend="storage.backend.other");

#[test]
fn rejects_invalid_names_and_versions_loudly() {
    let mut input = spec_input();
    input.name = "Bad-Name".into();
    assert!(
        define_domain(input)
            .err()
            .unwrap()
            .to_string()
            .contains("must match")
    );
    let mut input = spec_input();
    input.name = "ok".into();
    input.version = json!(1.5);
    assert!(
        define_domain(input)
            .err()
            .unwrap()
            .to_string()
            .contains("non-negative integer")
    );
    let input = DomainSpecInput::new(
        "ok",
        1,
        IndexMap::from([(
            "Bad Table".into(),
            domain_table::<String, Value>(item_schema()).into(),
        )]),
    );
    assert!(
        define_domain(input)
            .err()
            .unwrap()
            .to_string()
            .contains("table name")
    );
}
#[test]
fn rejects_a_global_schema_that_accepts_null_the_never_written_sentinel() {
    let mut input = spec_input();
    input.global = Some(
        DomainGlobalSpec {
            schema: ValueSchema::new(|v: &Value| {
                if v.is_null() {
                    Ok(Value::Null)
                } else {
                    settings_schema().parse(v)
                }
            }),
            initial: Value::Null,
        }
        .into(),
    );
    assert!(
        define_domain(input)
            .err()
            .unwrap()
            .to_string()
            .contains("must not accept null")
    );
}
#[test]
fn validates_compatible_versions_entries_and_projects_them_onto_the_descriptor() {
    for value in [json!(1.5), json!(2), json!(-1)] {
        let mut input = spec_input();
        input.version = json!(2);
        input.compatible_versions = Some(vec![value]);
        assert!(
            define_domain(input).err().unwrap().to_string().contains(
                "compatibleVersions entries must be non-negative integers below version 2"
            )
        );
    }
    let mut input = spec_input();
    input.version = json!(2);
    input.compatible_versions = Some(vec![json!(0), json!(1)]);
    assert_eq!(
        descriptor_of(&define_domain(input).unwrap()).compatible_versions,
        Some(vec![0, 1])
    );
    assert!(
        serde_json::to_value(descriptor_of(&spec()))
            .unwrap()
            .get("compatibleVersions")
            .is_none()
    );
}
#[test]
fn rejects_an_unknown_invalid_records_policy() {
    let mut input = spec_input();
    input.invalid_records = Some("zap".into());
    assert!(
        define_domain(input)
            .err()
            .unwrap()
            .to_string()
            .contains("invalidRecords")
    );
}
#[test]
fn rejects_an_invalid_layout_and_projects_the_declared_one_onto_the_descriptor() {
    let mut input = spec_input();
    input.layout = Some("every-record".into());
    assert!(
        define_domain(input)
            .err()
            .unwrap()
            .to_string()
            .contains("layout")
    );
    let mut input = spec_input();
    input.name = "per".into();
    input.layout = Some("per-record".into());
    let value = serde_json::to_value(descriptor_of(&define_domain(input).unwrap())).unwrap();
    assert_eq!(value["name"], "per");
    assert_eq!(value["layout"], "per-record");
    assert!(
        serde_json::to_value(descriptor_of(&spec()))
            .unwrap()
            .get("layout")
            .is_none()
    );
}
#[tokio::test]
async fn opens_reads_back_stored_records_and_rejects_a_second_open_of_the_same_name() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    table(&domain)
        .put("a".into(), item("first", 1))
        .await
        .unwrap();
    let error = h.facility.open(spec()).await.err().unwrap();
    assert_eq!(code(&error), "already-open");
    assert_eq!(
        error.downcast_ref::<DomainError>().unwrap().name(),
        "DomainError"
    );
    assert_eq!(
        *table(&domain).get(&"a".into()).unwrap().unwrap(),
        item("first", 1)
    );
    h.stop().await;
}
#[tokio::test]
async fn routes_per_domain_name_and_fails_loud_on_an_unregistered_route_target() {
    let h = Harness::new(
        Arc::default(),
        Some(Config {
            backend: "memory".into(),
            routes: IndexMap::from([("demo".into(), "nonexistent".into())]),
        }),
    )
    .await;
    let error = h.facility.open(spec()).await.err().unwrap();
    assert_eq!(code(&error), "backend-not-found");
    assert_eq!(
        error.downcast_ref::<StorageError>().unwrap().name(),
        "StorageError"
    );
    let healthy = harness().await;
    healthy.facility.open(spec()).await.unwrap();
    h.stop().await;
    healthy.stop().await;
}
#[tokio::test]
async fn rejects_a_backend_without_the_kv_facet() {
    let h = Harness::new(
        Arc::default(),
        Some(Config {
            backend: "nokv".into(),
            routes: IndexMap::new(),
        }),
    )
    .await;
    let _dispose = h
        .ctx
        .get::<Storage>()
        .unwrap()
        .backend
        .register("nokv", Arc::new(helpers::memory_backend::NoKv))
        .unwrap();
    assert_eq!(
        code(&h.facility.open(spec()).await.err().unwrap()),
        "facet-unsupported"
    );
    h.stop().await;
}
#[tokio::test]
async fn falls_back_to_the_default_backend_when_no_route_table_is_configured() {
    let h = harness().await;
    let config: Config = serde_json::from_value(json!({"backend":"memory"})).unwrap();
    let routeless = DomainFacility::new(h.ctx.clone(), config);
    routeless.open(bare_spec()).await.unwrap();
    routeless.close_all().await.unwrap();
    h.stop().await;
}
#[tokio::test]
async fn treats_a_table_key_the_backend_omitted_from_load_all_as_empty() {
    let backend = MemoryStorageBackend::new(Arc::default());
    backend.control.sparse.store(true, Ordering::Release);
    let h = Harness::backend(backend, None).await;
    let domain = h.facility.open(bare_spec()).await.unwrap();
    assert_eq!(
        domain
            .table::<String, Value>("rows")
            .unwrap()
            .size()
            .unwrap(),
        0
    );
    h.stop().await;
}
#[tokio::test]
async fn rejects_stored_records_that_fail_their_schema_naming_table_and_key() {
    let pool = Arc::new(MemoryMediaPool::default());
    let first = Harness::new(pool.clone(), None).await;
    let domain = first.facility.open(spec()).await.unwrap();
    table(&domain)
        .put("bad".into(), item("x", 2))
        .await
        .unwrap();
    first.stop().await;
    pool.state
        .lock()
        .unwrap()
        .media
        .get_mut("demo")
        .unwrap()
        .tables
        .get_mut("items")
        .unwrap()
        .insert("bad".into(), json!({"label":"x","count":"NaN"}));
    let h = Harness::new(pool, None).await;
    let error = h.facility.open(spec()).await.err().unwrap();
    assert_eq!(code(&error), "invalid-record");
    assert_eq!(
        error.downcast_ref::<DomainError>().unwrap().detail,
        Some(InvalidRecordDetail {
            table: "items".into(),
            key: "bad".into()
        })
    );
    h.stop().await;
}
#[tokio::test]
async fn keeps_the_rejecting_default_under_backup_and_skip_when_the_backend_cannot_move_documents()
{
    let mut input = spec_input();
    input.name = "salvage".into();
    input.global = None;
    input.invalid_records = Some("backup-and-skip".into());
    let spec = Arc::new(define_domain(input).unwrap());
    let pool = Arc::new(MemoryMediaPool::default());
    let first = Harness::new(pool.clone(), None).await;
    let domain = first.facility.open(spec.clone()).await.unwrap();
    table(&domain)
        .put("bad".into(), item("x", 2))
        .await
        .unwrap();
    first.stop().await;
    pool.state
        .lock()
        .unwrap()
        .media
        .get_mut("salvage")
        .unwrap()
        .tables
        .get_mut("items")
        .unwrap()
        .insert("bad".into(), json!({"label":"x","count":"NaN"}));
    let h = Harness::new(pool, None).await;
    let error = h.facility.open(spec).await.err().unwrap();
    assert_eq!(code(&error), "invalid-record");
    assert_eq!(
        error.downcast_ref::<DomainError>().unwrap().detail,
        Some(InvalidRecordDetail {
            table: "items".into(),
            key: "bad".into()
        })
    );
    h.stop().await;
}
#[tokio::test]
async fn rejects_a_stored_global_that_fails_its_schema_with_the_global_marker() {
    let pool = Arc::new(MemoryMediaPool::default());
    {
        let mut state = pool.state.lock().unwrap();
        state.versions.insert("demo".into(), 1);
        state.media.insert(
            "demo".into(),
            hoh_storage::KvSnapshot {
                tables: IndexMap::new(),
                global: json!({"theme":42}),
            },
        );
    }
    let h = Harness::new(pool, None).await;
    let error = h.facility.open(spec()).await.err().unwrap();
    assert_eq!(code(&error), "invalid-record");
    assert_eq!(
        error.downcast_ref::<DomainError>().unwrap().detail,
        Some(InvalidRecordDetail {
            table: String::new(),
            key: String::new()
        })
    );
    h.stop().await;
}
#[tokio::test]
async fn passes_through_a_backend_version_mismatch() {
    let pool = Arc::new(MemoryMediaPool::default());
    pool.state.lock().unwrap().versions.insert("demo".into(), 7);
    let h = Harness::new(pool, None).await;
    let error = h.facility.open(spec()).await.err().unwrap();
    assert_eq!(
        error.downcast_ref::<StorageError>().unwrap().name(),
        "StorageError"
    );
    assert_eq!(code(&error), "version-mismatch");
    h.stop().await;
}

#[tokio::test]
async fn uses_only_the_default_backend_when_routes_are_omitted() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<StoragePlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let backend = MemoryStorageBackend::new(Arc::default());
    let _unregister = ctx
        .get::<Storage>()
        .unwrap()
        .backend
        .register("memory", backend.clone())
        .unwrap();
    let ready = ctx.provide::<MemoryReady>(backend).await.unwrap();
    let fiber = ctx
        .plugin::<StorageDomainPlugin>(json!({"backend":"memory"}))
        .unwrap();
    fiber.settled().await.unwrap();
    settle_service(&kernel, &ctx, true).await;
    ready.dispose().await;
    settle_service(&kernel, &ctx, false).await;
    fiber.dispose().await;
    kernel.shutdown().await;
}
#[tokio::test]
async fn waits_for_routed_backends_then_mounts_one_lifecycle_bound_service_and_form() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<StoragePlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = ctx
        .plugin::<StorageDomainPlugin>(json!({"backend":"memory"}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(ctx.get::<StorageDomain>().is_none());
    assert!(ctx.get::<Storage>().unwrap().form("domain").is_err());
    let backend = MemoryStorageBackend::new(Arc::default());
    let _unregister = ctx
        .get::<Storage>()
        .unwrap()
        .backend
        .register("memory", backend.clone())
        .unwrap();
    let ready = ctx.provide::<MemoryReady>(backend).await.unwrap();
    settle_service(&kernel, &ctx, true).await;
    let form = ctx
        .get::<Storage>()
        .unwrap()
        .domain()
        .unwrap()
        .downcast::<DomainFacility>()
        .ok()
        .unwrap();
    assert!(std::ptr::eq(
        form.as_ref(),
        &*ctx.get::<StorageDomain>().unwrap()
    ));
    ready.dispose().await;
    settle_service(&kernel, &ctx, false).await;
    assert!(ctx.get::<Storage>().unwrap().form("domain").is_err());
    fiber.dispose().await;
    kernel.shutdown().await;
}
#[tokio::test]
async fn serves_entries_keys_and_size_as_stable_snapshots_unknown_table_names_throw() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    table.put("a".into(), item("x", 1)).await.unwrap();
    table.put("b".into(), item("y", 2)).await.unwrap();
    assert_eq!(table.size().unwrap(), 2);
    let mut keys = table.keys().unwrap().collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, ["a", "b"]);
    assert_eq!(
        *table.entries().unwrap().collect::<IndexMap<_, _>>()["a"],
        item("x", 1)
    );
    assert!(
        domain
            .table::<String, Value>("nope")
            .err()
            .unwrap()
            .to_string()
            .contains("declares no table")
    );
    h.stop().await;
}
#[tokio::test]
async fn serializes_concurrent_updates_on_one_key_without_losing_increments() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    table.put("counter".into(), item("c", 0)).await.unwrap();
    let jobs = (0..50)
        .map(|_| {
            table.update("counter".into(), |current| {
                item("c", current["count"].as_i64().unwrap() + 1)
            })
        })
        .collect::<Vec<_>>();
    futures_util::future::try_join_all(jobs).await.unwrap();
    assert_eq!(
        *table.get(&"counter".into()).unwrap().unwrap(),
        item("c", 50)
    );
    h.stop().await;
}
#[tokio::test]
async fn update_rejects_a_missing_key_delete_reports_prior_existence() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    assert_eq!(
        code(
            &table
                .update("ghost".into(), Clone::clone)
                .await
                .err()
                .unwrap()
        ),
        "missing-key"
    );
    table.put("a".into(), item("x", 1)).await.unwrap();
    assert!(table.delete("a".into()).await.unwrap());
    assert!(!table.delete("a".into()).await.unwrap());
    h.stop().await;
}
#[tokio::test]
async fn delete_many_removes_the_existing_keys_in_one_write_slot_and_reports_the_count() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    for (key, value) in [
        ("a", item("x", 1)),
        ("b", item("y", 2)),
        ("c", item("z", 3)),
    ] {
        table.put(key.into(), value).await.unwrap();
    }
    let seen = h.changes.lock().unwrap().len();
    assert_eq!(
        table
            .delete_many(vec!["a".into(), "c".into(), "missing".into(), "a".into()])
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        table
            .entries()
            .unwrap()
            .map(|(k, v)| (k, (*v).clone()))
            .collect::<Vec<_>>(),
        vec![("b".into(), item("y", 2))]
    );
    let events = h.changes.lock().unwrap()[seen..]
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        vec![
            json!({"domain":"demo","table":"items","key":"a","operation":"deleted"}),
            json!({"domain":"demo","table":"items","key":"c","operation":"deleted"})
        ]
    );
    assert_eq!(table.delete_many(vec!["ghost".into()]).await.unwrap(), 0);
    assert_eq!(h.changes.lock().unwrap().len(), seen + 2);
    h.stop().await;
}
#[tokio::test]
async fn applies_the_whole_batch_to_memory_before_announcing_it() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    table.put("a".into(), item("x", 1)).await.unwrap();
    table.put("b".into(), item("y", 2)).await.unwrap();
    let observed = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let seen = observed.clone();
    let read = table.clone();
    h.ctx
        .on::<DomainChanged>(move |_| {
            let mut keys = read.keys().unwrap().collect::<Vec<_>>();
            keys.sort();
            seen.lock().unwrap().push(keys);
        })
        .unwrap();
    table
        .delete_many(vec!["a".into(), "b".into()])
        .await
        .unwrap();
    assert_eq!(
        *observed.lock().unwrap(),
        vec![Vec::<String>::new(), vec![]]
    );
    h.stop().await;
}
#[tokio::test]
async fn calls_the_backend_batch_primitive_exactly_once_per_delete_many() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    table.put("a".into(), item("x", 1)).await.unwrap();
    table.put("b".into(), item("y", 2)).await.unwrap();
    assert_eq!(
        table
            .delete_many(vec!["a".into(), "b".into(), "missing".into()])
            .await
            .unwrap(),
        2
    );
    assert_eq!(h.backend.control.batches.load(Ordering::Relaxed), 1);
    assert_eq!(h.backend.control.singles.load(Ordering::Relaxed), 0);
    h.stop().await;
}
#[tokio::test]
async fn emits_domain_changed_per_durable_write_in_order_with_tombstones_and_global_marker() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    table.put("a".into(), item("x", 1)).await.unwrap();
    table.update("a".into(), |_| item("x", 2)).await.unwrap();
    table.delete("a".into()).await.unwrap();
    table.delete("a".into()).await.unwrap();
    global(&domain).set(json!({"theme":"dark"})).await.unwrap();
    let events = h
        .changes
        .lock()
        .unwrap()
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        vec![
            json!({"domain":"demo","table":"items","key":"a","operation":"put","value":item("x",1)}),
            json!({"domain":"demo","table":"items","key":"a","operation":"put","value":item("x",2)}),
            json!({"domain":"demo","table":"items","key":"a","operation":"deleted"}),
            json!({"domain":"demo","table":"","key":"","operation":"put","value":{"theme":"dark"}})
        ]
    );
    h.stop().await;
}
#[tokio::test]
async fn leaves_memory_untouched_and_emits_nothing_when_the_backend_rejects_a_write() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    table.put("a".into(), item("x", 1)).await.unwrap();
    let seen = h.changes.lock().unwrap().len();
    h.backend.pool.state.lock().unwrap().fail_next_writes = 3;
    for result in [
        table.put("a".into(), item("x", 99)).await,
        table
            .update("a".into(), |c| item("x", c["count"].as_i64().unwrap() + 1))
            .await
            .map(|_| ()),
        table.delete("a".into()).await.map(|_| ()),
    ] {
        assert!(result.err().unwrap().to_string().contains("injected"));
    }
    assert_eq!(*table.get(&"a".into()).unwrap().unwrap(), item("x", 1));
    assert_eq!(
        h.backend.pool.state.lock().unwrap().media["demo"].tables["items"]["a"],
        item("x", 1)
    );
    assert_eq!(h.changes.lock().unwrap().len(), seen);
    table
        .update("a".into(), |c| item("x", c["count"].as_i64().unwrap() + 1))
        .await
        .unwrap();
    assert_eq!(*table.get(&"a".into()).unwrap().unwrap(), item("x", 2));
    h.stop().await;
}
#[tokio::test]
async fn leaves_every_record_in_place_when_a_batch_delete_fails_durability() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    table.put("a".into(), item("x", 1)).await.unwrap();
    table.put("b".into(), item("y", 2)).await.unwrap();
    let seen = h.changes.lock().unwrap().len();
    h.backend.pool.state.lock().unwrap().fail_next_writes = 1;
    assert!(
        table
            .delete_many(vec!["a".into(), "b".into()])
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("injected")
    );
    assert_eq!(
        table
            .entries()
            .unwrap()
            .map(|(k, v)| (k, (*v).clone()))
            .collect::<Vec<_>>(),
        vec![("a".into(), item("x", 1)), ("b".into(), item("y", 2))]
    );
    assert_eq!(
        h.backend.pool.state.lock().unwrap().media["demo"].tables["items"]["b"],
        item("y", 2)
    );
    assert_eq!(h.changes.lock().unwrap().len(), seen);
    table.put("c".into(), item("z", 3)).await.unwrap();
    assert_eq!(*table.get(&"c".into()).unwrap().unwrap(), item("z", 3));
    h.stop().await;
}
#[tokio::test]
async fn quarantines_the_domain_after_a_batch_reports_unknown_state() {
    let h = harness().await;
    let poison = SharedError::new(StorageError::new(
        StorageErrorCode::UnknownState,
        "unknown-state injected by the test",
    ));
    *h.backend.control.poison.lock().unwrap() = Some(poison.clone());
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    table.put("a".into(), item("x", 1)).await.unwrap();
    let failing = table.delete_many(vec!["a".into()]);
    let queued = table.put("b".into(), item("y", 2));
    for error in [failing.await.err().unwrap(), queued.await.err().unwrap()] {
        assert_eq!(code(&error), "unknown-state");
        assert!(error.ptr_eq(&poison));
    }
    assert!(table.get(&"a".into()).err().unwrap().ptr_eq(&poison));
    assert!(table.keys().err().unwrap().ptr_eq(&poison));
    assert!(table.size().err().unwrap().ptr_eq(&poison));
    assert!(
        table
            .delete_many(vec!["missing".into()])
            .await
            .err()
            .unwrap()
            .ptr_eq(&poison)
    );
    assert!(
        table
            .put("c".into(), item("z", 3))
            .await
            .err()
            .unwrap()
            .ptr_eq(&poison)
    );
    assert!(
        global(&domain)
            .set(json!({"theme":"dark"}))
            .await
            .err()
            .unwrap()
            .ptr_eq(&poison)
    );
    assert!(global(&domain).get().err().unwrap().ptr_eq(&poison));
    domain.close().await.unwrap();
    assert_eq!(code(&table.get(&"a".into()).err().unwrap()), "closed");
    h.stop().await;
}
#[tokio::test]
async fn keeps_serving_initial_when_the_first_global_set_fails_durability() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    h.backend.pool.state.lock().unwrap().fail_next_writes = 1;
    assert!(
        global(&domain)
            .set(json!({"theme":"dark"}))
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("injected")
    );
    assert_eq!(*global(&domain).get().unwrap(), json!({"theme":"plain"}));
    assert!(
        h.backend.pool.state.lock().unwrap().media["demo"]
            .global
            .is_null()
    );
    h.stop().await;
}
#[tokio::test]
async fn serves_initial_before_first_set_without_materializing_then_persists_the_first_set() {
    let pool = Arc::new(MemoryMediaPool::default());
    let h = Harness::new(pool.clone(), None).await;
    let domain = h.facility.open(spec()).await.unwrap();
    assert_eq!(*global(&domain).get().unwrap(), json!({"theme":"plain"}));
    assert!(pool.state.lock().unwrap().media["demo"].global.is_null());
    global(&domain).set(json!({"theme":"dark"})).await.unwrap();
    assert_eq!(
        pool.state.lock().unwrap().media["demo"].global,
        json!({"theme":"dark"})
    );
    h.stop().await;
    let restarted = Harness::new(pool, None).await;
    assert_eq!(
        *global(&restarted.facility.open(spec()).await.unwrap())
            .get()
            .unwrap(),
        json!({"theme":"dark"})
    );
    restarted.stop().await;
}
#[tokio::test]
async fn throws_on_access_when_the_spec_declares_no_global() {
    let h = harness().await;
    let domain = h.facility.open(bare_spec()).await.unwrap();
    assert!(
        domain
            .global::<Value>()
            .err()
            .unwrap()
            .to_string()
            .contains("declares no global")
    );
    h.stop().await;
}
#[tokio::test]
async fn close_drains_queued_writes_then_rejects_reads_and_writes_and_frees_the_name() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    let a = table.put("a".into(), item("x", 1));
    let b = table.put("b".into(), item("y", 2));
    futures_util::future::try_join_all([domain.close(), domain.close()])
        .await
        .unwrap();
    a.await.unwrap();
    b.await.unwrap();
    let mut saved = h.backend.pool.state.lock().unwrap().media["demo"].tables["items"]
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    saved.sort();
    assert_eq!(saved, ["a", "b"]);
    assert_eq!(
        code(&table.put("c".into(), item("z", 3)).await.err().unwrap()),
        "closed"
    );
    assert!(
        table
            .get(&"a".into())
            .err()
            .unwrap()
            .to_string()
            .contains("closed")
    );
    let reopened = h.facility.open(spec()).await.unwrap();
    let mut keys = helpers::table(&reopened)
        .keys()
        .unwrap()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, ["a", "b"]);
    h.stop().await;
}
#[tokio::test]
async fn facility_unmount_closes_domains_the_consumer_never_closed() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<StoragePlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let backend = MemoryStorageBackend::new(Arc::default());
    let _unregister = ctx
        .get::<Storage>()
        .unwrap()
        .backend
        .register("memory", backend.clone())
        .unwrap();
    let _ready = ctx.provide::<MemoryReady>(backend).await.unwrap();
    let fiber = ctx
        .plugin::<StorageDomainPlugin>(json!({"backend":"memory"}))
        .unwrap();
    fiber.settled().await.unwrap();
    settle_service(&kernel, &ctx, true).await;
    let domain = ctx
        .get::<StorageDomain>()
        .unwrap()
        .open(bare_spec())
        .await
        .unwrap();
    let table = domain.table::<String, Value>("rows").unwrap();
    table.put("a".into(), item("x", 1)).await.unwrap();
    fiber.dispose().await;
    assert_eq!(
        code(&table.put("b".into(), item("y", 2)).await.err().unwrap()),
        "closed"
    );
    assert!(ctx.get::<Storage>().unwrap().form("domain").is_err());
    kernel.shutdown().await;
}
#[tokio::test]
async fn contains_a_throwing_domain_changed_listener_without_rejecting_the_committed_write() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    h.ctx
        .on::<DomainChanged>(|_| panic!("hostile observer"))
        .unwrap();
    table.put("a".into(), item("x", 1)).await.unwrap();
    assert_eq!(*table.get(&"a".into()).unwrap().unwrap(), item("x", 1));
    assert_eq!(
        h.backend.pool.state.lock().unwrap().media["demo"].tables["items"]["a"],
        item("x", 1)
    );
    assert_eq!(h.changes.lock().unwrap().len(), 1);
    assert!(table.delete("a".into()).await.unwrap());
    assert!(
        h.ctx.logger().buffer().iter().any(|r| r.message
            == "domain 'demo': domain/changed listener failed: Error: hostile observer")
    );
    h.stop().await;
}

#[tokio::test]
async fn backup_and_skip_logs_the_exact_zod_cause_and_keeps_good_records() {
    let backend = MemoryStorageBackend::new(Arc::default());
    backend.control.backup.store(true, Ordering::Release);
    {
        let mut state = backend.pool.state.lock().unwrap();
        state.media.insert(
            "demo".into(),
            hoh_storage::KvSnapshot {
                tables: IndexMap::from([(
                    "items".into(),
                    IndexMap::from([
                        ("bad".into(), json!({"label":"x","count":"NaN"})),
                        ("good".into(), item("ok", 2)),
                    ]),
                )]),
                global: Value::Null,
            },
        );
    }
    let h = Harness::backend(backend, None).await;
    let mut input = spec_input();
    input.invalid_records = Some("backup-and-skip".into());
    let domain = h
        .facility
        .open(Arc::new(define_domain(input).unwrap()))
        .await
        .unwrap();
    assert!(table(&domain).get(&"bad".into()).unwrap().is_none());
    assert_eq!(
        *table(&domain).get(&"good".into()).unwrap().unwrap(),
        item("ok", 2)
    );
    assert!(
        !h.backend.pool.state.lock().unwrap().media["demo"].tables["items"].contains_key("bad")
    );
    let expected = "domain 'demo': stored record 'bad' in table 'items' failed schema validation; moved to 'items/bad.json.bak.1' and treated as absent. Cause: ZodError: [\n  {\n    \"expected\": \"number\",\n    \"code\": \"invalid_type\",\n    \"path\": [\n      \"count\"\n    ],\n    \"message\": \"Invalid input: expected number, received string\"\n  }\n]";
    assert!(
        h.ctx
            .logger()
            .buffer()
            .iter()
            .any(|r| r.message == expected)
    );
    h.stop().await;
}
#[tokio::test]
async fn failed_global_validation_closes_the_unit_and_releases_the_same_name() {
    let backend = MemoryStorageBackend::new(Arc::default());
    backend.control.backup.store(true, Ordering::Release);
    backend.pool.state.lock().unwrap().media.insert(
        "demo".into(),
        hoh_storage::KvSnapshot {
            tables: IndexMap::new(),
            global: json!({"theme":42}),
        },
    );
    let h = Harness::backend(backend, None).await;
    let mut input = spec_input();
    input.invalid_records = Some("backup-and-skip".into());
    let declared = Arc::new(define_domain(input).unwrap());
    let error = h.facility.open(declared.clone()).await.err().unwrap();
    let domain_error = error.downcast_ref::<DomainError>().unwrap();
    assert_eq!(domain_error.code(), "invalid-record");
    assert!(
        domain_error
            .cause()
            .unwrap()
            .downcast_ref::<ZodError>()
            .is_some()
    );
    assert!(hoh_error::HarnessError::of(&error).is_none());
    assert_eq!(h.backend.control.closes.load(Ordering::Relaxed), 1);
    assert!(h.facility.get("demo").is_none());
    h.backend
        .pool
        .state
        .lock()
        .unwrap()
        .media
        .get_mut("demo")
        .unwrap()
        .global = json!({"theme":"fixed"});
    h.facility.open(declared).await.unwrap();
    h.stop().await;
}
#[tokio::test]
async fn stored_schemas_strip_extra_properties_and_writes_do_not_revalidate() {
    let backend = MemoryStorageBackend::new(Arc::default());
    backend.pool.state.lock().unwrap().media.insert(
        "demo".into(),
        hoh_storage::KvSnapshot {
            tables: IndexMap::from([(
                "items".into(),
                IndexMap::from([("a".into(), json!({"label":"x","count":1,"extra":true}))]),
            )]),
            global: json!({"theme":"plain","extra":true}),
        },
    );
    let h = Harness::backend(backend, None).await;
    let domain = h.facility.open(spec()).await.unwrap();
    assert_eq!(
        *table(&domain).get(&"a".into()).unwrap().unwrap(),
        item("x", 1)
    );
    assert_eq!(*global(&domain).get().unwrap(), json!({"theme":"plain"}));
    table(&domain)
        .put("bad".into(), json!({"label":"x","count":"2"}))
        .await
        .unwrap();
    assert_eq!(
        h.backend.pool.state.lock().unwrap().media["demo"].tables["items"]["bad"]["count"],
        "2"
    );
    domain.close().await.unwrap();
    assert_eq!(
        code(&h.facility.open(spec()).await.err().unwrap()),
        "invalid-record"
    );
    h.stop().await;
}
#[test]
fn the_test_decoders_match_zod_object_and_safe_integer_acceptance() {
    for value in [
        json!({"label":"x","count":0}),
        json!({"label":"x","count":-1}),
        json!({"label":"x","count":9_007_199_254_740_991_i64}),
        json!({"label":"x","count":1.0}),
    ] {
        assert!(item_schema().parse(&value).is_ok());
    }
    for value in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"label":42,"count":2}),
        json!({"label":"x","count":"2"}),
        json!({"label":"x","count":1.5}),
        json!({"label":"x","count":9_007_199_254_740_992_i64}),
    ] {
        assert!(item_schema().parse(&value).is_err());
    }
    assert_eq!(
        item_schema()
            .parse(&json!({"label":"x","count":1,"ignored":true}))
            .unwrap(),
        item("x", 1)
    );
}
#[tokio::test]
async fn opening_uses_object_key_order_but_later_writes_keep_map_insertion_order() {
    let backend = MemoryStorageBackend::new(Arc::default());
    backend.pool.state.lock().unwrap().media.insert(
        "demo".into(),
        hoh_storage::KvSnapshot {
            tables: IndexMap::from([(
                "items".into(),
                ["10", "b", "2", "01"]
                    .map(|key| (key.into(), item(key, 1)))
                    .into_iter()
                    .collect(),
            )]),
            global: Value::Null,
        },
    );
    let h = Harness::backend(backend, None).await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    assert_eq!(
        table.keys().unwrap().collect::<Vec<_>>(),
        ["2", "10", "b", "01"]
    );
    table.put("1".into(), item("one", 1)).await.unwrap();
    assert_eq!(
        table.keys().unwrap().collect::<Vec<_>>(),
        ["2", "10", "b", "01", "1"]
    );
    h.stop().await;
}
#[tokio::test]
async fn handles_and_values_keep_identity_and_iterators_remain_stable() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    assert!(Arc::ptr_eq(&table, &helpers::table(&domain)));
    assert!(Arc::ptr_eq(&global(&domain), &global(&domain)));
    table.put("a".into(), item("old", 1)).await.unwrap();
    let first = table.get(&"a".into()).unwrap().unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &table.get(&"a".into()).unwrap().unwrap()
    ));
    let snapshot = table.entries().unwrap();
    table.put("b".into(), item("new", 2)).await.unwrap();
    table
        .update("a".into(), |_| item("replaced", 3))
        .await
        .unwrap();
    let entries = snapshot.collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    assert!(Arc::ptr_eq(&entries[0].1, &first));
    assert_eq!(*first, item("old", 1));

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TypedItem {
        label: String,
        count: i64,
    }
    let schema =
        ValueSchema::<TypedItem>::new(|raw| Ok(serde_json::from_value(item_schema().parse(raw)?)?));
    let mut input = DomainSpecInput::new(
        "typed",
        1,
        IndexMap::from([(
            "items".into(),
            domain_table::<String, TypedItem>(schema).into(),
        )]),
    );
    let global_schema = ValueSchema::<String>::new(|raw| {
        raw.as_str()
            .map(str::to_uppercase)
            .ok_or_else(|| "expected string".into())
    });
    input.global = Some(
        DomainGlobalSpec {
            schema: global_schema,
            initial: "plain".to_owned(),
        }
        .into(),
    );
    let declared = Arc::new(define_domain(input).unwrap());
    let typed = h.facility.open(declared.clone()).await.unwrap();
    let typed_table = typed.table::<String, TypedItem>("items").unwrap();
    typed_table
        .put(
            "one".into(),
            TypedItem {
                label: "typed".into(),
                count: 1,
            },
        )
        .await
        .unwrap();
    typed
        .global::<String>()
        .unwrap()
        .set("dark".into())
        .await
        .unwrap();
    assert_eq!(
        typed.global::<String>().unwrap().get().unwrap().as_str(),
        "dark"
    );
    typed.close().await.unwrap();
    let reopened = h.facility.open(declared).await.unwrap();
    assert_eq!(
        reopened
            .table::<String, TypedItem>("items")
            .unwrap()
            .get(&"one".into())
            .unwrap()
            .unwrap()
            .as_ref(),
        &TypedItem {
            label: "typed".into(),
            count: 1
        }
    );
    assert_eq!(
        reopened.global::<String>().unwrap().get().unwrap().as_str(),
        "DARK"
    );
    let _: TableKeyOf<DomainTableSpec<String, TypedItem>> = "one".into();
    let _: GlobalValueOf<DomainGlobalSpec<String>> = "plain".into();
    h.stop().await;
}
#[tokio::test]
async fn close_reserves_the_name_until_a_blocked_write_is_durable_and_still_emits() {
    let h = harness().await;
    let gate = helpers::memory_backend::Gate::new();
    *h.backend.control.gate.lock().unwrap() = Some(gate.clone());
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    let pending = table.put("a".into(), item("x", 1));
    gate.entered.acquire().await.unwrap().forget();
    assert_eq!(table.size().unwrap(), 0);
    assert!(h.changes.lock().unwrap().is_empty());
    let closing = domain.close();
    assert_eq!(
        code(&h.facility.open(spec()).await.err().unwrap()),
        "already-open"
    );
    assert_eq!(
        code(&table.put("b".into(), item("y", 2)).await.err().unwrap()),
        "closed"
    );
    assert!(table.get(&"a".into()).unwrap().is_none());
    gate.release.add_permits(1);
    pending.await.unwrap();
    closing.await.unwrap();
    assert_eq!(h.changes.lock().unwrap().len(), 1);
    assert_eq!(h.backend.control.closes.load(Ordering::Relaxed), 1);
    h.facility.open(spec()).await.unwrap();
    h.stop().await;
}
#[tokio::test]
async fn dropped_write_and_close_waiters_do_not_cancel_the_underlying_work() {
    let h = harness().await;
    let domain = h.facility.open(spec()).await.unwrap();
    let table = table(&domain);
    drop(table.put("a".into(), item("x", 1)));
    drop(domain.close());
    domain.close().await.unwrap();
    assert_eq!(
        h.backend.pool.state.lock().unwrap().media["demo"].tables["items"]["a"],
        item("x", 1)
    );
    assert_eq!(h.backend.control.closes.load(Ordering::Relaxed), 1);
    h.stop().await;
}
#[tokio::test]
async fn all_distinct_routed_lifecycle_services_are_required() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<StoragePlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let backend = MemoryStorageBackend::new(Arc::default());
    let storage = ctx.get::<Storage>().unwrap();
    let _memory = storage.backend.register("memory", backend.clone()).unwrap();
    let _other = storage.backend.register("other", backend.clone()).unwrap();
    let _ready = ctx.provide::<MemoryReady>(backend.clone()).await.unwrap();
    let fiber = ctx
        .plugin::<StorageDomainPlugin>(
            json!({"backend":"memory","routes":{"demo":"other","bare":"other"}}),
        )
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(ctx.get::<StorageDomain>().is_none());
    let other = ctx.provide::<OtherReady>(backend).await.unwrap();
    settle_service(&kernel, &ctx, true).await;
    ctx.get::<StorageDomain>()
        .unwrap()
        .open(spec())
        .await
        .unwrap();
    other.dispose().await;
    settle_service(&kernel, &ctx, false).await;
    assert!(ctx.get::<Storage>().unwrap().form("domain").is_err());
    fiber.dispose().await;
    kernel.shutdown().await;
}
#[test]
fn change_payloads_keep_field_order_and_reject_unknown_operation_tags() {
    use hoh_storage_domain::events::{DomainChangedDeleted, DomainChangedPut};
    let put = DomainChanged::Put(DomainChangedPut {
        domain: "d".into(),
        table: "t".into(),
        key: "k".into(),
        value: json!({"v":1}),
    });
    let value = serde_json::to_value(&put).unwrap();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["domain", "table", "key", "operation", "value"]
    );
    assert_eq!(serde_json::from_value::<DomainChanged>(value).unwrap(), put);
    let deleted = DomainChanged::Deleted(DomainChangedDeleted {
        domain: "d".into(),
        table: "t".into(),
        key: "k".into(),
    });
    let value = serde_json::to_value(&deleted).unwrap();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["domain", "table", "key", "operation"]
    );
    assert_eq!(
        serde_json::from_value::<DomainChanged>(value).unwrap(),
        deleted
    );
    assert!(serde_json::from_value::<DomainChanged>(json!({"operation":"unknown"})).is_err());
}
