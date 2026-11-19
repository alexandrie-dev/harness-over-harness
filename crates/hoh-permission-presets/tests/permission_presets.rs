use super::*;
use hoh_brand::ids::SessionId;
use hoh_kernel::Kernel;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

fn service_with(
    config: Config,
    sandbox: Option<SandboxMode>,
    approval: ApprovalPolicy,
) -> (PermissionPresetService, Arc<Mutex<Option<String>>>) {
    let default = Arc::new(Mutex::new(config.default_preset.0.clone()));
    let service =
        PermissionPresetService::direct_for_tests(config, sandbox, approval, default.clone())
            .unwrap();
    (service, default)
}

fn service() -> PermissionPresetService {
    service_with(
        Config::default(),
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
    )
    .0
}

fn session(id: &str) -> Session {
    Session::create(SessionId::from(id), None, None, None, None).unwrap()
}

fn append(session: &Session, event_type: &str, data: Value) {
    session.append(event_type, data, None).unwrap();
}

fn events(session: &Session) -> Vec<(String, Value)> {
    session
        .snapshot_events(None, None)
        .iter()
        .map(|event| (event.event_type().into(), event.data().clone()))
        .collect()
}

fn custom_config(entries: &[(&str, SandboxMode, ApprovalPolicy)]) -> Config {
    Config {
        presets: entries
            .iter()
            .map(|(name, sandbox, approval)| {
                (
                    (*name).into(),
                    PresetSpec {
                        sandbox: *sandbox,
                        approval: *approval,
                        name: None,
                        description: None,
                    },
                )
            })
            .collect(),
        default_preset: Volatile(None),
    }
}

#[test]
fn folds_the_latest_preset_selection_and_steps_over_unrelated_events() {
    let source = session("sess-fold");
    append(
        &source,
        "permission/preset",
        json!({"preset":"danger-full-access"}),
    );
    append(
        &source,
        "permission/preset",
        json!({"preset":"workspace-write"}),
    );
    append(&source, "sandbox/mode", json!({"mode":"read-only"}));
    let state = fold_session(&source).unwrap();
    assert_eq!(state.preset.as_deref(), Some("workspace-write"));
    assert_eq!(state.sandbox, Some(SandboxMode::ReadOnly));

    let seed = source.snapshot_events(None, None);
    let seeded = Session::create(
        SessionId::from("sess-fold-seeded"),
        Some(&seed),
        None,
        None,
        None,
    )
    .unwrap();
    assert!(fold_session(&seeded).unwrap().seeded);
}

#[test]
fn does_not_activate_without_the_required_projection_registry() {
    let ready = service();
    *lock(&ready.inner.state_source) = StateSource::Pending;
    let error = ready.current(&session("missing-projection")).unwrap_err();
    assert!(error.to_string().contains("not ready"));
}

#[test]
fn advertises_the_preset_table_in_declaration_order_and_resolves_bundles() {
    let service = service();
    assert_eq!(service.names(), ["workspace-write", "danger-full-access"]);
    assert_eq!(
        service.resolve("danger-full-access").unwrap(),
        PresetSpec {
            sandbox: SandboxMode::DangerFullAccess,
            approval: ApprovalPolicy::Never,
            name: Some("danger-full-access".into()),
            description: Some("Full file access without approval prompts.".into()),
        }
    );
    assert!(
        service
            .resolve("plan")
            .unwrap_err()
            .to_string()
            .contains("unknown preset \"plan\"")
    );
}

#[test]
fn orders_integer_like_preset_keys_like_a_javascript_object() {
    let config = custom_config(&[
        ("2", SandboxMode::WorkspaceWrite, ApprovalPolicy::Ask),
        ("1", SandboxMode::WorkspaceWrite, ApprovalPolicy::Ask),
    ]);
    let (service, _) = service_with(
        config,
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
    );

    assert_eq!(service.names(), ["1", "2"]);
    let catalog = service.catalog().unwrap();
    assert_eq!(
        catalog
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert_eq!(
        catalog
            .default_options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert_eq!(catalog.default_preset, "1");
}

#[tokio::test]
async fn publishes_an_effect_scoped_current_session_preset_and_removes_it_on_unload() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let service = service();
    let disposer = service.register_auto(&ctx, || Ok(())).unwrap();
    assert_eq!(
        service.names().last().map(String::as_str),
        Some(AUTO_PRESET)
    );
    assert_eq!(service.resolve(AUTO_PRESET).unwrap(), auto_preset_spec());
    assert_eq!(
        service.option_of(AUTO_PRESET).unwrap(),
        PresetOption {
            value: "auto".into(),
            name: "auto".into(),
            description: None,
        }
    );
    disposer.dispose().await;
    assert_eq!(service.names(), ["workspace-write", "danger-full-access"]);
    assert!(service.resolve(AUTO_PRESET).is_err());
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_a_duplicate_auto_integration() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let service = service();
    let _first = service.register_auto(&ctx, || Ok(())).unwrap();
    let error = match service.register_auto(&ctx, || Ok(())) {
        Ok(_) => panic!("duplicate Auto registration succeeded"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("already registered"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn runs_auto_admission_before_any_write_including_a_no_op_selection() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let service = service();
    let admissions = Arc::new(AtomicUsize::new(0));
    let count = admissions.clone();
    let reentrant_service = service.clone();
    let reentrant_ctx = ctx.clone();
    let _auto = service
        .register_auto(&ctx, move || {
            count.fetch_add(1, Ordering::SeqCst);
            let duplicate = reentrant_service.register_auto(&reentrant_ctx, || Ok(()));
            let error = match duplicate {
                Ok(_) => {
                    return Err(PermissionError::message(
                        "duplicate Auto registration succeeded",
                    ));
                }
                Err(error) => error,
            };
            assert!(error.to_string().contains("already registered"));
            Ok(())
        })
        .unwrap();
    let session = session("sess-auto-admit");
    service.set(&session, AUTO_PRESET).unwrap();
    assert_eq!(admissions.load(Ordering::SeqCst), 1);
    assert_eq!(
        events(&session),
        [
            ("permission/preset".into(), json!({"preset":"auto"})),
            ("sandbox/mode".into(), json!({"mode":"danger-full-access"})),
        ]
    );
    service.set(&session, AUTO_PRESET).unwrap();
    assert_eq!(admissions.load(Ordering::SeqCst), 2);
    assert_eq!(events(&session).len(), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_an_admitted_auto_selection_when_admission_removes_its_registration() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let service = service();
    let registration = Arc::new(Mutex::new(None::<Disposer>));
    let remove = registration.clone();
    let disposer = service
        .register_auto(&ctx, move || {
            let disposer = lock(&remove)
                .take()
                .expect("Auto registration is still live during admission");
            drop(disposer.dispose());
            Ok(())
        })
        .unwrap();
    *lock(&registration) = Some(disposer);

    let session = session("auto-removes-itself");
    service.set(&session, AUTO_PRESET).unwrap();

    assert_eq!(
        events(&session),
        [
            ("permission/preset".into(), json!({"preset":"auto"})),
            ("sandbox/mode".into(), json!({"mode":"danger-full-access"})),
        ]
    );
    assert!(!service.names().contains(&AUTO_PRESET.to_owned()));
    kernel.shutdown().await;
}

#[tokio::test]
async fn leaves_the_session_untouched_when_dynamic_admission_rejects_a_selection() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let service = service();
    let _auto = service
        .register_auto(&ctx, || {
            Err(PermissionError::message("auto review is closing"))
        })
        .unwrap();
    let session = session("sess-auto-closed");
    assert!(
        service
            .set(&session, AUTO_PRESET)
            .unwrap_err()
            .to_string()
            .contains("closing")
    );
    assert!(events(&session).is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn switches_between_auto_and_full_access_through_identity_and_approval_policy() {
    let config = custom_config(&[
        ("read-only", SandboxMode::ReadOnly, ApprovalPolicy::Ask),
        (
            "workspace-write",
            SandboxMode::WorkspaceWrite,
            ApprovalPolicy::Ask,
        ),
        (
            "danger-full-access",
            SandboxMode::DangerFullAccess,
            ApprovalPolicy::Never,
        ),
    ]);
    let (service, _) = service_with(
        config,
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
    );
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let _auto = service.register_auto(&ctx, || Ok(())).unwrap();
    let session = session("shared-bundle-switch");
    service.set(&session, AUTO_PRESET).unwrap();
    let baseline = events(&session).len();
    service.set(&session, "danger-full-access").unwrap();
    assert_eq!(
        &events(&session)[baseline..],
        &[
            (
                "permission/preset".into(),
                json!({"preset":"danger-full-access"})
            ),
            ("approval/policy".into(), json!({"policy":"never"})),
        ]
    );
    service.set(&session, AUTO_PRESET).unwrap();
    append(&session, "approval/policy", json!({"policy":"never"}));
    assert_eq!(service.current(&session).unwrap(), AUTO_PRESET);
    kernel.shutdown().await;
}

#[test]
fn current_derives_from_the_effective_knobs() {
    let service = service();
    let session = session("sess-current");
    assert_eq!(service.current(&session).unwrap(), "workspace-write");
    service.set(&session, "danger-full-access").unwrap();
    assert_eq!(service.current(&session).unwrap(), "danger-full-access");
}

#[test]
fn a_knob_state_matching_no_table_entry_derives_custom() {
    let service = service();
    let session = session("sess-custom");
    append(&session, "sandbox/mode", json!({"mode":"read-only"}));
    assert_eq!(service.current(&session).unwrap(), CUSTOM_PRESET);
    service.set(&session, "danger-full-access").unwrap();
    assert_eq!(service.current(&session).unwrap(), "danger-full-access");
    assert!(service.resolve(CUSTOM_PRESET).is_err());
}

#[test]
fn composition_defaults_outside_the_table_still_derive_custom_with_an_explicit_default() {
    let config = Config {
        default_preset: Volatile(Some("workspace-write".into())),
        ..Config::default()
    };
    let (service, _) = service_with(
        config,
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Never,
    );
    assert_eq!(
        service.current(&session("sess-defaults-custom")).unwrap(),
        CUSTOM_PRESET
    );
}

#[test]
fn the_fold_breaks_bundle_ties_and_a_stale_fold_falls_back_to_table_order() {
    let config = custom_config(&[
        (
            "workspace-write",
            SandboxMode::WorkspaceWrite,
            ApprovalPolicy::Ask,
        ),
        ("agentish", SandboxMode::WorkspaceWrite, ApprovalPolicy::Ask),
        (
            "danger-full-access",
            SandboxMode::DangerFullAccess,
            ApprovalPolicy::Never,
        ),
    ]);
    let (service, _) = service_with(
        config,
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
    );
    let session = session("sess-tie");
    service.set(&session, "agentish").unwrap();
    assert_eq!(service.current(&session).unwrap(), "agentish");
    append(&session, "approval/policy", json!({"policy":"never"}));
    append(
        &session,
        "sandbox/mode",
        json!({"mode":"danger-full-access"}),
    );
    assert_eq!(service.current(&session).unwrap(), "danger-full-access");
}

#[test]
fn set_writes_one_preset_event_plus_both_knob_events() {
    let service = service();
    let session = session("sess-set");
    service.set(&session, "danger-full-access").unwrap();
    assert_eq!(events(&session).len(), 3);
    assert_eq!(events(&session)[0].0, "permission/preset");
    assert_eq!(events(&session)[1].0, "sandbox/mode");
    assert_eq!(events(&session)[2].0, "approval/policy");
}

#[test]
fn set_to_the_current_preset_is_a_no_op_when_the_knobs_already_match() {
    let service = service();
    let session = session("sess-noop");
    service.set(&session, "workspace-write").unwrap();
    assert!(events(&session).is_empty());
}

#[test]
fn reasserting_a_preset_from_a_drifted_state_records_choice_and_repairs_only_the_knob() {
    let service = service();
    let session = session("sess-drift");
    service.set(&session, "danger-full-access").unwrap();
    append(&session, "sandbox/mode", json!({"mode":"read-only"}));
    service.set(&session, "danger-full-access").unwrap();
    assert_eq!(
        &events(&session)[4..],
        &[
            (
                "permission/preset".into(),
                json!({"preset":"danger-full-access"})
            ),
            ("sandbox/mode".into(), json!({"mode":"danger-full-access"})),
        ]
    );
}

#[test]
fn rejects_composition_over_a_non_confining_executor_at_load() {
    let error = match PermissionPresetService::direct_for_tests(
        Config::default(),
        None,
        ApprovalPolicy::Ask,
        Arc::new(Mutex::new(None)),
    ) {
        Ok(_) => panic!("non-confining executor was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("does not confine"));
}

#[test]
fn option_of_presents_shipped_fallback_and_custom_values() {
    let service = service();
    assert_eq!(
        service
            .option_of("danger-full-access")
            .unwrap()
            .description
            .as_deref(),
        Some("Full file access without approval prompts.")
    );
    assert_eq!(service.option_of(CUSTOM_PRESET).unwrap().name, "Custom");
    let config = custom_config(&[("plain", SandboxMode::WorkspaceWrite, ApprovalPolicy::Ask)]);
    let (bare, _) = service_with(
        config,
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
    );
    assert_eq!(bare.option_of("plain").unwrap().name, "plain");
    assert!(service.option_of("plan").is_err());
}

#[test]
fn rejects_a_table_entry_named_custom() {
    let config = custom_config(&[("custom", SandboxMode::ReadOnly, ApprovalPolicy::Ask)]);
    let error = match PermissionPresetService::direct_for_tests(
        config,
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
        Arc::new(Mutex::new(None)),
    ) {
        Ok(_) => panic!("reserved custom entry was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("reserved for the derived"));
}

#[test]
fn reserves_auto_for_an_integration_contribution() {
    let config = custom_config(&[("auto", SandboxMode::DangerFullAccess, ApprovalPolicy::Never)]);
    let error = match PermissionPresetService::direct_for_tests(
        config,
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
        Arc::new(Mutex::new(None)),
    ) {
        Ok(_) => panic!("reserved auto entry was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("\"auto\" is reserved"));
}

#[test]
fn requires_an_explicit_default_when_composition_defaults_match_no_preset() {
    let error = match PermissionPresetService::direct_for_tests(
        Config::default(),
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Never,
        Arc::new(Mutex::new(None)),
    ) {
        Ok(_) => panic!("missing explicit default was accepted"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("configure defaultPreset explicitly")
    );
}

#[test]
fn reads_a_schema_less_approval_stand_in_as_the_ask_default() {
    let service = service();
    let session = session("sess-standin");
    service.set(&session, "workspace-write").unwrap();
    assert!(events(&session).is_empty());
    assert_eq!(service.current(&session).unwrap(), "workspace-write");
}

#[test]
fn rejects_persisted_auto_before_publication_when_its_integration_is_absent() {
    let service = service();
    let source = session("auto-source");
    append(&source, "permission/preset", json!({"preset":"auto"}));
    append(
        &source,
        "sandbox/mode",
        json!({"mode":"danger-full-access"}),
    );
    append(&source, "approval/policy", json!({"policy":"never"}));
    let seed = source.snapshot_events(None, None);
    let resumed = Session::create(
        SessionId::from("auto-without-integration"),
        Some(&seed),
        None,
        None,
        None,
    )
    .unwrap();
    let before = events(&resumed);
    let error = service.pin_initial_permission(&resumed).unwrap_err();
    assert!(error.to_string().contains("cannot restore preset \"auto\""));
    assert_eq!(events(&resumed), before);
}

#[tokio::test]
async fn admits_persisted_auto_through_the_live_integration_without_rewriting_it() {
    let service = service();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let admissions = Arc::new(AtomicUsize::new(0));
    let count = admissions.clone();
    let _auto = service
        .register_auto(&ctx, move || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    let source = session("auto-source-present");
    append(&source, "permission/preset", json!({"preset":"auto"}));
    append(
        &source,
        "sandbox/mode",
        json!({"mode":"danger-full-access"}),
    );
    append(&source, "approval/policy", json!({"policy":"never"}));
    let seed = source.snapshot_events(None, None);
    let resumed = Session::create(
        SessionId::from("auto-with-integration"),
        Some(&seed),
        None,
        None,
        None,
    )
    .unwrap();
    service.pin_initial_permission(&resumed).unwrap();
    assert_eq!(admissions.load(Ordering::SeqCst), 1);
    assert_eq!(service.current(&resumed).unwrap(), AUTO_PRESET);
    assert_eq!(
        events(&resumed)
            .iter()
            .filter(|(kind, _)| kind == "permission/preset")
            .count(),
        1
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn keeps_an_admitted_auto_restore_when_admission_removes_its_registration() {
    let service = service();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registration = Arc::new(Mutex::new(None::<Disposer>));
    let remove = registration.clone();
    let disposer = service
        .register_auto(&ctx, move || {
            let disposer = lock(&remove)
                .take()
                .expect("Auto registration is still live during restore admission");
            drop(disposer.dispose());
            Ok(())
        })
        .unwrap();
    *lock(&registration) = Some(disposer);
    let restored = session("auto-restore-removes-itself");
    append(&restored, "permission/preset", json!({"preset":"auto"}));
    append(
        &restored,
        "sandbox/mode",
        json!({"mode":"danger-full-access"}),
    );
    append(&restored, "approval/policy", json!({"policy":"never"}));
    let before = events(&restored);

    service.pin_initial_permission(&restored).unwrap();

    assert_eq!(events(&restored), before);
    assert!(!service.names().contains(&AUTO_PRESET.to_owned()));
    kernel.shutdown().await;
}

#[test]
fn pins_the_current_setting_into_each_new_session_without_changing_earlier_sessions() {
    let (service, default) = service_with(
        Config::default(),
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
    );
    let first = session("first");
    service.pin_initial_permission(&first).unwrap();
    *lock(&default) = Some("danger-full-access".into());
    let second = session("second");
    service.pin_initial_permission(&second).unwrap();
    assert_eq!(service.current(&first).unwrap(), "workspace-write");
    assert_eq!(service.current(&second).unwrap(), "danger-full-access");
    assert_eq!(events(&first).len(), 3);
    assert_eq!(events(&second).len(), 3);
}

#[test]
fn preserves_a_seeded_legacy_session_instead_of_applying_latest_default() {
    let (service, default) = service_with(
        Config::default(),
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
    );
    *lock(&default) = Some("danger-full-access".into());
    let legacy = session("legacy-source");
    append(&legacy, "turn/start", json!({"turn":1}));
    let seed = legacy.snapshot_events(None, None);
    let resumed = Session::create(
        SessionId::from("legacy-resumed"),
        Some(&seed),
        None,
        None,
        None,
    )
    .unwrap();
    service.pin_initial_permission(&resumed).unwrap();
    assert_eq!(service.current(&resumed).unwrap(), "workspace-write");
    let kinds = events(&resumed)
        .into_iter()
        .map(|(kind, _)| kind)
        .collect::<Vec<_>>();
    assert_eq!(
        &kinds[kinds.len() - 3..],
        &["permission/preset", "sandbox/mode", "approval/policy"]
    );
}

#[test]
fn preserves_composition_defaults_when_an_empty_stored_session_resumes() {
    let (service, default) = service_with(
        Config::default(),
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
    );
    *lock(&default) = Some("danger-full-access".into());
    let resumed = Session::create(
        SessionId::from("empty-resumed"),
        Some(&[]),
        None,
        None,
        None,
    )
    .unwrap();
    service.pin_initial_permission(&resumed).unwrap();
    assert_eq!(service.current(&resumed).unwrap(), "workspace-write");
    assert_eq!(events(&resumed)[0].0, "session/end-seed");
}

#[test]
fn pins_sessions_that_already_exist_when_the_service_remounts() {
    let service = service();
    let existing = session("existing-before-permission");
    assert!(events(&existing).is_empty());
    service.pin_initial_permission(&existing).unwrap();
    assert_eq!(events(&existing).len(), 3);
    assert_eq!(service.current(&existing).unwrap(), "workspace-write");
}

#[test]
fn preserves_existing_knob_overrides_when_the_service_remounts() {
    let service = service();
    let existing = session("existing-knobs");
    append(&existing, "sandbox/mode", json!({"mode":"read-only"}));
    append(&existing, "approval/policy", json!({"policy":"never"}));
    service.pin_initial_permission(&existing).unwrap();
    assert_eq!(events(&existing).len(), 2);
    assert_eq!(service.current(&existing).unwrap(), CUSTOM_PRESET);
}

#[test]
fn fills_only_missing_legacy_facts_and_preserves_an_unmatched_seeded_combination() {
    let service = service();
    let partial = session("partial-source");
    append(&partial, "sandbox/mode", json!({"mode":"workspace-write"}));
    append(&partial, "approval/policy", json!({"policy":"ask"}));
    let partial_seed = partial.snapshot_events(None, None);
    let resumed = Session::create(
        SessionId::from("partial-resumed"),
        Some(&partial_seed),
        None,
        None,
        None,
    )
    .unwrap();
    service.pin_initial_permission(&resumed).unwrap();
    assert_eq!(
        events(&resumed).last().unwrap().1,
        json!({"preset":"workspace-write"})
    );

    let custom = session("custom-source");
    append(&custom, "sandbox/mode", json!({"mode":"read-only"}));
    append(&custom, "approval/policy", json!({"policy":"never"}));
    let custom_seed = custom.snapshot_events(None, None);
    let unmatched = Session::create(
        SessionId::from("custom-resumed"),
        Some(&custom_seed),
        None,
        None,
        None,
    )
    .unwrap();
    service.pin_initial_permission(&unmatched).unwrap();
    assert_eq!(service.current(&unmatched).unwrap(), CUSTOM_PRESET);
    assert_eq!(events(&unmatched).last().unwrap().0, "session/end-seed");
}

#[test]
fn materializes_ask_when_a_legacy_seed_and_approval_stand_in_omit_the_policy() {
    let service = service();
    let partial = session("approval-fallback-source");
    append(&partial, "sandbox/mode", json!({"mode":"workspace-write"}));
    let seed = partial.snapshot_events(None, None);
    let resumed = Session::create(
        SessionId::from("approval-fallback-resumed"),
        Some(&seed),
        None,
        None,
        None,
    )
    .unwrap();
    service.pin_initial_permission(&resumed).unwrap();
    assert_eq!(events(&resumed).last().unwrap().1, json!({"policy":"ask"}));
}

#[test]
fn fails_to_read_a_stored_default_outside_the_configured_table_until_repaired() {
    let (service, default) = service_with(
        Config::default(),
        Some(SandboxMode::WorkspaceWrite),
        ApprovalPolicy::Ask,
    );
    *lock(&default) = Some(AUTO_PRESET.into());
    assert!(
        service
            .default_preset()
            .unwrap_err()
            .to_string()
            .contains("unknown default preset")
    );
    *lock(&default) = Some("workspace-write".into());
    assert_eq!(service.default_preset().unwrap(), "workspace-write");
}
