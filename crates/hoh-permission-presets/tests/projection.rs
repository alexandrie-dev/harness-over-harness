use super::*;
use hoh_brand::ids::SessionId;
use hoh_kernel::Kernel;
use serde_json::json;
use std::{
    sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

fn service() -> PermissionPresetService {
    PermissionPresetService::direct_for_tests(
        Config::default(),
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
        Arc::new(Mutex::new(None)),
    )
    .unwrap()
}

fn session(id: &str) -> Session {
    Session::create(SessionId::from(id), None, None, None, None).unwrap()
}

fn permission_definition_with_view_barriers(
    service: &PermissionPresetService,
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
) -> ProjectionDefinition<PermissionProjectionState> {
    let mut definition = permission_projection_definition(service);
    let wire = definition.wire.as_mut().expect("permissions wire");
    let view = wire.view.clone();
    wire.view = Arc::new(move |state| {
        entered.wait();
        release.wait();
        view(state)
    });
    definition
}

#[tokio::test]
async fn current_and_projection_snapshot_do_not_deadlock() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = hoh_session_projection::SessionProjectionRegistry::new(&ctx).unwrap();
    let service = service();
    let current_entered = Arc::new(Barrier::new(2));
    let current_release = Arc::new(Barrier::new(2));
    *lock(&service.inner.before_current_projection_read) = Some({
        let entered = current_entered.clone();
        let release = current_release.clone();
        Arc::new(move || {
            entered.wait();
            release.wait();
        })
    });
    let snapshot_entered = Arc::new(Barrier::new(2));
    let snapshot_release = Arc::new(Barrier::new(2));
    let handle = registry
        .register(
            &ctx,
            permission_definition_with_view_barriers(
                &service,
                snapshot_entered.clone(),
                snapshot_release.clone(),
            ),
        )
        .await
        .unwrap();
    *lock(&service.inner.state_source) = StateSource::Projection(handle);
    let session = session("current-snapshot-lock-order");
    service.permission_state(&session).unwrap();

    let (current_tx, current_rx) = mpsc::channel();
    let current_service = service.clone();
    let current_session = session.clone();
    let current_thread = std::thread::spawn(move || {
        current_tx
            .send(
                current_service
                    .current(&current_session)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
            )
            .ok();
    });
    current_entered.wait();

    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let snapshot_registry = registry.clone();
    let snapshot_session = session.clone();
    let snapshot_thread = std::thread::spawn(move || {
        snapshot_tx
            .send(
                snapshot_registry
                    .snapshot(&snapshot_session, None)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
            )
            .ok();
    });
    snapshot_entered.wait();
    snapshot_release.wait();
    current_release.wait();

    current_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("current() deadlocked with snapshot()")
        .unwrap();
    snapshot_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("snapshot() deadlocked with current()")
        .unwrap();
    current_thread.join().unwrap();
    snapshot_thread.join().unwrap();
    kernel.shutdown().await;
}

#[tokio::test]
async fn set_and_projection_snapshot_do_not_deadlock() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = hoh_session_projection::SessionProjectionRegistry::new(&ctx).unwrap();
    let service = service();
    let snapshot_entered = Arc::new(Barrier::new(2));
    let snapshot_release = Arc::new(Barrier::new(2));
    let handle = registry
        .register(
            &ctx,
            permission_definition_with_view_barriers(
                &service,
                snapshot_entered.clone(),
                snapshot_release.clone(),
            ),
        )
        .await
        .unwrap();
    *lock(&service.inner.state_source) = StateSource::Projection(handle);
    let session = session("set-snapshot-lock-order");
    service.permission_state(&session).unwrap();
    let admission_entered = Arc::new(Barrier::new(2));
    let admission_release = Arc::new(Barrier::new(2));
    let _auto = service
        .register_auto(&ctx, {
            let entered = admission_entered.clone();
            let release = admission_release.clone();
            move || {
                entered.wait();
                release.wait();
                Ok(())
            }
        })
        .unwrap();

    let (set_tx, set_rx) = mpsc::channel();
    let set_service = service.clone();
    let set_session = session.clone();
    let set_thread = std::thread::spawn(move || {
        set_tx
            .send(
                set_service
                    .set(&set_session, AUTO_PRESET)
                    .map_err(|error| error.to_string()),
            )
            .ok();
    });
    admission_entered.wait();

    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let snapshot_registry = registry.clone();
    let snapshot_session = session.clone();
    let snapshot_thread = std::thread::spawn(move || {
        snapshot_tx
            .send(
                snapshot_registry
                    .snapshot(&snapshot_session, None)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
            )
            .ok();
    });
    snapshot_entered.wait();
    snapshot_release.wait();
    admission_release.wait();

    set_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("set() deadlocked with snapshot()")
        .unwrap();
    snapshot_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("snapshot() deadlocked with set()")
        .unwrap();
    set_thread.join().unwrap();
    snapshot_thread.join().unwrap();
    kernel.shutdown().await;
}

#[test]
fn serves_only_the_pinned_new_session_current_value() {
    let service = service();
    let session = session("perm-projected");
    service.pin_initial_permission(&session).unwrap();
    let value = PermissionSelection {
        current_value: service.current(&session).unwrap(),
    };
    assert_eq!(
        serde_json::to_value(value).unwrap(),
        json!({"currentValue":"workspace-write"})
    );
}

#[tokio::test]
async fn folds_the_knob_events_and_notifies_the_change_feed_per_knob_append() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = hoh_session_projection::SessionProjectionRegistry::new(&ctx).unwrap();
    let service = service();
    let _handle = registry
        .register(&ctx, permission_projection_definition(&service))
        .await
        .unwrap();
    let store = hoh_session::SessionStore::new();
    let session = store
        .create(
            &ctx,
            Some(SessionId::from("projection-changes")),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    service.pin_initial_permission(&session).unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    let observed = changes.clone();
    registry
        .on_changed(
            &ctx,
            Arc::new(move |_session, key, value, seq| {
                lock(&observed).push((key.to_owned(), value, seq));
                Ok(())
            }),
        )
        .unwrap();
    service.set(&session, "danger-full-access").unwrap();
    assert_eq!(lock(&changes).len(), 3);
    assert!(
        lock(&changes)
            .iter()
            .all(|(key, _, _)| key == "permissions")
    );
    assert_eq!(
        lock(&changes).last().unwrap().1.as_ref(),
        &json!({"currentValue":"danger-full-access"})
    );
    assert_eq!(service.current(&session).unwrap(), "danger-full-access");
    session
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    assert_eq!(lock(&changes).len(), 3);
    kernel.shutdown().await;
}

#[test]
fn projects_custom_as_a_current_value_when_the_knobs_match_no_preset() {
    let service = service();
    let session = session("projection-custom");
    session
        .append("sandbox/mode", json!({"mode":"read-only"}), None)
        .unwrap();
    assert_eq!(service.current(&session).unwrap(), CUSTOM_PRESET);
}

#[tokio::test]
async fn publishes_auto_only_through_the_process_catalog_and_never_changes_session_seq() {
    let service = service();
    let session = session("catalog-auto");
    let before = session.seq();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let auto = service.register_auto(&ctx, || Ok(())).unwrap();
    assert_eq!(
        service.catalog().unwrap().options.last().unwrap().value,
        AUTO_PRESET
    );
    assert_eq!(session.seq(), before);
    auto.dispose().await;
    assert_eq!(service.catalog().unwrap().options.len(), 2);
    assert_eq!(session.seq(), before);
    kernel.shutdown().await;
}

#[tokio::test]
async fn contains_synchronous_and_asynchronous_catalog_notification_failures() {
    let service = service();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let sync_calls = Arc::new(AtomicUsize::new(0));
    let async_calls = Arc::new(AtomicUsize::new(0));
    let sync_seen = sync_calls.clone();
    ctx.on_contained::<CatalogChanged>(move |_| {
        sync_seen.fetch_add(1, Ordering::SeqCst);
        Err(PermissionError::message("sync observer failed"))
    })
    .unwrap();
    let async_seen = async_calls.clone();
    ctx.on_contained_async::<CatalogChanged, _>(move |_| {
        let async_seen = async_seen.clone();
        async move {
            async_seen.fetch_add(1, Ordering::SeqCst);
            Err(PermissionError::message("async observer failed"))
        }
    })
    .unwrap();
    let auto = service.register_auto(&ctx, || Ok(())).unwrap();
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);
    assert_eq!(async_calls.load(Ordering::SeqCst), 1);
    assert!(service.names().contains(&AUTO_PRESET.to_owned()));
    auto.dispose().await;
    assert_eq!(sync_calls.load(Ordering::SeqCst), 2);
    assert_eq!(async_calls.load(Ordering::SeqCst), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn has_no_permissions_key_without_the_service_and_drops_it_on_unload() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = hoh_session_projection::SessionProjectionRegistry::new(&ctx).unwrap();
    let service = service();
    let session = session("projection-hmr");
    let handle = registry
        .register(&ctx, permission_projection_definition(&service))
        .await
        .unwrap();
    assert!(
        registry
            .snapshot(&session, None)
            .unwrap()
            .values
            .contains_key("permissions")
    );
    handle.dispose().await;
    assert!(
        !registry
            .snapshot(&session, None)
            .unwrap()
            .values
            .contains_key("permissions")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn lists_and_switches_a_contributed_current_session_preset() {
    let service = service();
    let session = session("command-auto");
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let _auto = service.register_auto(&ctx, || Ok(())).unwrap();
    let listed = service
        .execute_permission_command(&session, "", |_| Ok(()))
        .unwrap();
    assert_eq!(listed.kind, "success");
    assert_eq!(
        listed.text,
        "current preset workspace-write (available: workspace-write, danger-full-access, auto)"
    );
    let switched = service
        .execute_permission_command(&session, "auto", |policy| {
            set_approval_policy(&session, policy)
        })
        .unwrap();
    assert_eq!(switched.text, "preset auto");
    assert_eq!(service.current(&session).unwrap(), AUTO_PRESET);
    kernel.shutdown().await;
}

#[test]
fn switches_through_permission_set_and_logs_the_lifecycle_pair() {
    let service = service();
    let session = session("command-switch");
    let command_session = session.clone();
    let approvals = Arc::new(Mutex::new(Vec::new()));
    let seen = approvals.clone();
    let result = service
        .execute_permission_command(&session, " danger-full-access", move |policy| {
            lock(&seen).push(policy);
            set_approval_policy(&command_session, policy)
        })
        .unwrap();
    assert_eq!(result.text, "preset danger-full-access");
    assert_eq!(&*lock(&approvals), &[ApprovalPolicy::Never]);
    assert_eq!(service.current(&session).unwrap(), "danger-full-access");
}

#[test]
fn reports_the_current_preset_and_table_on_bare_invocation() {
    let service = service();
    let session = session("command-list");
    let result = service
        .execute_permission_command(&session, "   ", |_| Ok(()))
        .unwrap();
    assert_eq!(result.kind, "success");
    assert_eq!(
        result.text,
        "current preset workspace-write (available: workspace-write, danger-full-access)"
    );
    assert!(session.snapshot_events(None, None).is_empty());
}

#[test]
fn trims_bom_like_javascript_in_permission_commands() {
    let service = service();
    let session = session("command-bom");

    let listed = service
        .execute_permission_command(&session, "\u{feff}", |_| Ok(()))
        .unwrap();
    assert_eq!(listed.kind, "success");
    assert_eq!(
        listed.text,
        "current preset workspace-write (available: workspace-write, danger-full-access)"
    );

    let switched = service
        .execute_permission_command(&session, "\u{feff}danger-full-access\u{feff}", |policy| {
            set_approval_policy(&session, policy)
        })
        .unwrap();
    assert_eq!(switched.kind, "success");
    assert_eq!(switched.text, "preset danger-full-access");
}

#[test]
fn rejects_an_unknown_preset_without_touching_the_log() {
    let service = service();
    let session = session("command-unknown");
    let before = session.snapshot_events(None, None);
    let result = service
        .execute_permission_command(&session, "plan", |_| Ok(()))
        .unwrap();
    assert_eq!(result.kind, "error");
    assert_eq!(
        result.text,
        "unknown preset \"plan\" (available: workspace-write, danger-full-access)"
    );
    assert_eq!(
        session.snapshot_events(None, None).as_ref(),
        before.as_ref()
    );
}

#[test]
fn keeps_its_own_instance_off_the_generated_settings_pages() {
    let schema = schemars::schema_for!(Config);
    let text = serde_json::to_string(&schema).unwrap();
    assert!(text.contains("defaultPreset"));
    assert!(!text.contains("permissionPresets"));
}
