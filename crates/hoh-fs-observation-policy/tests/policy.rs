use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use hoh_error::{BoxError, HarnessError};
use hoh_fs::{
    FileSystem, FsActor, FsDeleteIntent, FsEditIntent, FsErrorCode, FsObservation, FsObserved,
    FsResolveOptions, FsTarget, FsVersionGuard, FsWriteIntent, FsWriteIntentEvent,
};
use hoh_fs_local::{Config as LocalConfig, LocalFileSystem};
use hoh_fs_observation_policy::{FsObservationActor, FsObservationAgent, FsObservationPolicy};
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_scope::ScopeKey;
use hoh_session::Session;
use hoh_timeout::AbortController;
use hoh_tools::{Agent, ToolExecution, ToolExecutionInput, ToolExecutionPipeline};
use serde_json::Value;
use tempfile::TempDir;

struct Setup {
    kernel: Kernel,
    ctx: Context,
    fiber: Fiber,
}

async fn setup() -> Setup {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<FsObservationPolicy>(().into())
        .expect("plugin");
    fiber.settled().await.expect("plugin active");
    Setup { kernel, ctx, fiber }
}

fn target(path: &str) -> FsTarget {
    FsTarget {
        target_key: path.into(),
        display_path: path.into(),
    }
}

fn session(id: &str) -> Session {
    Session::create(id.into(), None, None, None, None).expect("session")
}

fn owner_actor(owner: &Session) -> Option<FsActor> {
    Some(Arc::new(FsObservationActor {
        agent: Some(FsObservationAgent {
            session: Some(owner.clone()),
        }),
    }))
}

fn agent_without_session() -> Option<FsActor> {
    Some(Arc::new(FsObservationActor {
        agent: Some(FsObservationAgent { session: None }),
    }))
}

fn unrelated_actor() -> Option<FsActor> {
    Some(Arc::new(()))
}

struct TestAgent {
    key: ScopeKey,
    session: Session,
}

impl Agent for TestAgent {
    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }

    fn session(&self) -> Session {
        self.session.clone()
    }
}

async fn tool_execution(ctx: &Context, agent: Arc<dyn Agent>, call_id: &str) -> Arc<ToolExecution> {
    let mut input = ToolExecutionInput::new(
        call_id.into(),
        "observation-policy-test",
        Value::Null,
        AbortController::new().signal(),
    );
    input.agent = Some(agent);
    ToolExecutionPipeline::new(ctx.clone())
        .expect("pipeline")
        .prepare(input)
        .await
        .exec()
        .clone()
}

fn tool_actor(execution: &Arc<ToolExecution>) -> Option<FsActor> {
    Some(execution.clone())
}

fn present(version: &str) -> FsObservation {
    FsObservation::Present {
        version: version.into(),
    }
}

fn emit_observed(
    ctx: &Context,
    target: FsTarget,
    observation: FsObservation,
    actor: Option<FsActor>,
) {
    ctx.emit(&FsObserved {
        target,
        observation,
        actor,
    });
}

async fn write_intent(
    ctx: &Context,
    target: FsTarget,
    actor: Option<FsActor>,
) -> Result<Option<FsWriteIntent>, BoxError> {
    ctx.waterfall(FsWriteIntentEvent { target, actor }, |_| async { Ok(None) })
        .await
}

async fn edit_intent(
    ctx: &Context,
    target: FsTarget,
    actor: Option<FsActor>,
) -> Result<Option<FsVersionGuard>, BoxError> {
    ctx.waterfall(FsEditIntent { target, actor }, |_| async { Ok(None) })
        .await
}

fn assert_fs_error(error: &BoxError, code: FsErrorCode, message: Option<&str>) {
    let error = HarnessError::of(error.as_ref()).expect("structured FsError");
    assert_eq!(error.name(), "FsError");
    assert_eq!(error.code(), code.as_str());
    if let Some(message) = message {
        assert_eq!(error.message(), message);
    }
}

#[tokio::test]
async fn registers_no_service_api_it_is_a_plugin_not_ctx_fs_policy() {
    let setup = setup().await;
    assert!(setup.kernel.snapshot().await.provisions.is_empty());
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn mounts_with_no_inject_reads_no_services() {
    let setup = setup().await;
    assert_eq!(
        write_intent(&setup.ctx, target("a.txt"), None)
            .await
            .unwrap(),
        Some(FsWriteIntent::CreateIfAbsent)
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn an_unobserved_target_decides_create_if_absent() {
    let setup = setup().await;
    let owner = session("owner");
    assert_eq!(
        write_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
            .await
            .unwrap(),
        Some(FsWriteIntent::CreateIfAbsent)
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn a_no_owner_actor_decides_create_if_absent() {
    let setup = setup().await;
    for actor in [None, unrelated_actor()] {
        assert_eq!(
            write_intent(&setup.ctx, target("a.txt"), actor)
                .await
                .unwrap(),
            Some(FsWriteIntent::CreateIfAbsent)
        );
    }
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn an_actor_with_an_agent_but_no_session_has_no_owner_create_if_absent() {
    let setup = setup().await;
    assert_eq!(
        write_intent(&setup.ctx, target("a.txt"), agent_without_session())
            .await
            .unwrap(),
        Some(FsWriteIntent::CreateIfAbsent)
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn an_observed_target_decides_replace_if_version_at_the_observed_version() {
    let setup = setup().await;
    let owner = session("owner");
    emit_observed(
        &setup.ctx,
        target("a.txt"),
        present("v7"),
        owner_actor(&owner),
    );
    assert_eq!(
        write_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
            .await
            .unwrap(),
        Some(FsWriteIntent::ReplaceIfVersion {
            version: "v7".into()
        })
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn a_target_observed_absent_decides_create_if_absent() {
    let setup = setup().await;
    let owner = session("owner");
    emit_observed(
        &setup.ctx,
        target("a.txt"),
        FsObservation::Absent,
        owner_actor(&owner),
    );
    assert_eq!(
        write_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
            .await
            .unwrap(),
        Some(FsWriteIntent::CreateIfAbsent)
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_an_unread_edit_with_fs_not_observed() {
    let setup = setup().await;
    let owner = session("owner");
    let error = edit_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
        .await
        .expect_err("unread edit must fail");
    assert_fs_error(
        &error,
        FsErrorCode::NotObserved,
        Some("edit requires reading \"a.txt\" first"),
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_an_edit_with_no_owner_cannot_prove_prior_observation() {
    let setup = setup().await;
    let error = edit_intent(&setup.ctx, target("a.txt"), None)
        .await
        .expect_err("ownerless edit must fail");
    assert_fs_error(&error, FsErrorCode::NotObserved, None);
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_an_edit_whose_actor_has_an_agent_but_no_session() {
    let setup = setup().await;
    let error = edit_intent(&setup.ctx, target("a.txt"), agent_without_session())
        .await
        .expect_err("sessionless edit must fail");
    assert_fs_error(&error, FsErrorCode::NotObserved, None);
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn returns_the_observed_version_as_the_cas_basis_after_an_observation() {
    let setup = setup().await;
    let owner = session("owner");
    emit_observed(
        &setup.ctx,
        target("a.txt"),
        present("v3"),
        owner_actor(&owner),
    );
    assert_eq!(
        edit_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
            .await
            .unwrap(),
        Some(FsVersionGuard {
            version: "v3".into()
        })
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_editing_a_target_observed_absent_with_fs_not_found() {
    let setup = setup().await;
    let owner = session("owner");
    emit_observed(
        &setup.ctx,
        target("a.txt"),
        FsObservation::Absent,
        owner_actor(&owner),
    );
    let error = edit_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
        .await
        .expect_err("absent edit must fail");
    assert_fs_error(&error, FsErrorCode::NotFound, None);
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn a_read_observation_authorizes_an_in_place_write_at_that_version() {
    let setup = setup().await;
    let owner = session("owner");
    emit_observed(
        &setup.ctx,
        target("a.txt"),
        present("v0"),
        owner_actor(&owner),
    );
    assert_eq!(
        write_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
            .await
            .unwrap(),
        Some(FsWriteIntent::ReplaceIfVersion {
            version: "v0".into()
        })
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn a_write_edit_observation_refreshes_the_basis_so_the_next_edit_needs_no_re_read() {
    let setup = setup().await;
    let owner = session("owner");
    for version in ["v1", "v2"] {
        emit_observed(
            &setup.ctx,
            target("a.txt"),
            present(version),
            owner_actor(&owner),
        );
        assert_eq!(
            edit_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
                .await
                .unwrap(),
            Some(FsVersionGuard {
                version: version.into()
            })
        );
    }
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn a_no_owner_observation_records_nothing() {
    let setup = setup().await;
    let owner = session("owner");
    emit_observed(&setup.ctx, target("a.txt"), present("v0"), None);
    let error = edit_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
        .await
        .expect_err("ownerless observation must not authorize");
    assert_fs_error(&error, FsErrorCode::NotObserved, None);
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn supports_present_absent_present_transitions_for_one_owner() {
    let setup = setup().await;
    let owner = session("owner");
    let actor = || owner_actor(&owner);
    emit_observed(&setup.ctx, target("a.txt"), present("v1"), actor());
    assert!(matches!(
        write_intent(&setup.ctx, target("a.txt"), actor())
            .await
            .unwrap(),
        Some(FsWriteIntent::ReplaceIfVersion { .. })
    ));
    emit_observed(&setup.ctx, target("a.txt"), FsObservation::Absent, actor());
    assert_eq!(
        write_intent(&setup.ctx, target("a.txt"), actor())
            .await
            .unwrap(),
        Some(FsWriteIntent::CreateIfAbsent)
    );
    let error = edit_intent(&setup.ctx, target("a.txt"), actor())
        .await
        .expect_err("absent target");
    assert_fs_error(&error, FsErrorCode::NotFound, None);
    emit_observed(&setup.ctx, target("a.txt"), present("v2"), actor());
    assert_eq!(
        edit_intent(&setup.ctx, target("a.txt"), actor())
            .await
            .unwrap(),
        Some(FsVersionGuard {
            version: "v2".into()
        })
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn owner_a_observing_does_not_grant_owner_b_edit_authority() {
    let setup = setup().await;
    let a = session("same-id");
    let b = session("same-id");
    emit_observed(&setup.ctx, target("a.txt"), present("v0"), owner_actor(&a));
    let error = edit_intent(&setup.ctx, target("a.txt"), owner_actor(&b))
        .await
        .expect_err("owner B did not observe");
    assert_fs_error(&error, FsErrorCode::NotObserved, None);
    assert!(
        edit_intent(&setup.ctx, target("a.txt"), owner_actor(&a))
            .await
            .is_ok()
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn each_owner_records_its_own_observed_version_independently() {
    let setup = setup().await;
    let a = session("a");
    let b = session("b");
    emit_observed(&setup.ctx, target("a.txt"), present("v0"), owner_actor(&a));
    assert_eq!(
        write_intent(&setup.ctx, target("a.txt"), owner_actor(&b))
            .await
            .unwrap(),
        Some(FsWriteIntent::CreateIfAbsent)
    );
    assert!(matches!(
        write_intent(&setup.ctx, target("a.txt"), owner_actor(&a))
            .await
            .unwrap(),
        Some(FsWriteIntent::ReplaceIfVersion { .. })
    ));
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn fully_decides_the_slot_without_calling_next_the_bare_default_is_unreached() {
    let setup = setup().await;
    let default_ran = Arc::new(AtomicBool::new(false));
    let ran = default_ran.clone();
    let intent: Option<FsWriteIntent> = setup
        .ctx
        .waterfall(
            FsWriteIntentEvent {
                target: target("a.txt"),
                actor: None,
            },
            move |_| async move {
                ran.store(true, Ordering::SeqCst);
                Ok(None)
            },
        )
        .await
        .unwrap();
    assert_eq!(intent, Some(FsWriteIntent::CreateIfAbsent));
    assert!(!default_ran.load(Ordering::SeqCst));
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn a_second_edit_decider_registered_after_policy_is_not_reached() {
    let setup = setup().await;
    let second_ran = Arc::new(AtomicBool::new(false));
    let ran = second_ran.clone();
    setup
        .ctx
        .on_waterfall::<FsEditIntent, Option<FsVersionGuard>, _>(move |_event, _next| {
            ran.store(true, Ordering::SeqCst);
            async { Ok(None) }
        })
        .unwrap();
    let owner = session("owner");
    emit_observed(
        &setup.ctx,
        target("a.txt"),
        present("v0"),
        owner_actor(&owner),
    );
    edit_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
        .await
        .unwrap();
    assert!(!second_ran.load(Ordering::SeqCst));
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn a_second_write_intent_decider_registered_after_policy_is_not_reached() {
    let setup = setup().await;
    let second_ran = Arc::new(AtomicBool::new(false));
    let ran = second_ran.clone();
    setup
        .ctx
        .on_waterfall::<FsWriteIntentEvent, Option<FsWriteIntent>, _>(move |_event, _next| {
            ran.store(true, Ordering::SeqCst);
            async { Ok(None) }
        })
        .unwrap();
    write_intent(&setup.ctx, target("a.txt"), None)
        .await
        .unwrap();
    assert!(!second_ran.load(Ordering::SeqCst));
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn a_fresh_plugin_after_disposal_starts_with_no_inherited_state() {
    let setup = setup().await;
    let owner = session("owner");
    emit_observed(
        &setup.ctx,
        target("a.txt"),
        present("v0"),
        owner_actor(&owner),
    );
    assert!(
        edit_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
            .await
            .is_ok()
    );
    setup.fiber.dispose().await;
    let fiber = setup.ctx.plugin::<FsObservationPolicy>(().into()).unwrap();
    fiber.settled().await.unwrap();
    let error = edit_intent(&setup.ctx, target("a.txt"), owner_actor(&owner))
        .await
        .expect_err("fresh plugin has no state");
    assert_fs_error(&error, FsErrorCode::NotObserved, None);
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn no_listeners_remain_after_disposal_the_gate_no_longer_decides() {
    let setup = setup().await;
    setup.fiber.dispose().await;
    assert_eq!(
        write_intent(&setup.ctx, target("a.txt"), None)
            .await
            .unwrap(),
        None
    );
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn delete_intent_rejects_unseen_and_absent_returns_present_and_releases_listener() {
    let setup = setup().await;
    let owner = session("owner");
    let actor = || owner_actor(&owner);
    let dispatch = |actor| {
        setup.ctx.waterfall(
            FsDeleteIntent {
                target: target("a"),
                actor,
            },
            |_| async { Ok(None) },
        )
    };
    let error = dispatch(actor())
        .await
        .expect_err("unseen delete must fail");
    assert_fs_error(
        &error,
        FsErrorCode::NotObserved,
        Some("delete requires reading \"a\" first"),
    );
    emit_observed(&setup.ctx, target("a"), FsObservation::Absent, actor());
    let error = dispatch(actor())
        .await
        .expect_err("absent delete must fail");
    assert_fs_error(
        &error,
        FsErrorCode::NotFound,
        Some("cannot delete \"a\": not found"),
    );
    emit_observed(&setup.ctx, target("a"), present("v7"), actor());
    assert_eq!(
        dispatch(actor()).await.unwrap(),
        Some(FsVersionGuard {
            version: "v7".into()
        })
    );
    setup.fiber.dispose().await;
    assert_eq!(dispatch(actor()).await.unwrap(), None);
    setup.kernel.shutdown().await;
}

#[tokio::test]
async fn unread_existing_file_overwrite_is_denied_and_the_file_is_untouched() {
    let setup = setup().await;
    let agent: Arc<dyn Agent> = Arc::new(TestAgent {
        key: ScopeKey::new(),
        session: session("owner"),
    });
    let weak_agent = Arc::downgrade(&agent);
    let first_execution = tool_execution(&setup.ctx, agent.clone(), "first").await;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("existing.txt");
    std::fs::write(&path, "original").unwrap();
    let local = LocalFileSystem::new(LocalConfig {
        cwd: Some(temp.path().to_string_lossy().into_owned()),
        ..LocalConfig::default()
    })
    .unwrap();
    let resolved = local
        .resolve(&path.to_string_lossy(), FsResolveOptions::default())
        .await
        .unwrap();
    let intent = write_intent(&setup.ctx, resolved.clone(), tool_actor(&first_execution))
        .await
        .unwrap();
    assert_eq!(intent, Some(FsWriteIntent::CreateIfAbsent));
    let error = local
        .write_text(&resolved, "changed", intent, None, None)
        .await
        .expect_err("unread overwrite must fail atomically");
    assert_fs_error(
        &error,
        FsErrorCode::NotObserved,
        Some(&format!(
            "cannot overwrite existing \"{}\" without reading it first",
            resolved.display_path
        )),
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");

    emit_observed(
        &setup.ctx,
        resolved.clone(),
        present("observed-version"),
        tool_actor(&first_execution),
    );
    let second_execution = tool_execution(&setup.ctx, agent.clone(), "second").await;
    assert_eq!(
        write_intent(&setup.ctx, resolved, tool_actor(&second_execution),)
            .await
            .unwrap(),
        Some(FsWriteIntent::ReplaceIfVersion {
            version: "observed-version".into(),
        })
    );

    drop(first_execution);
    drop(second_execution);
    drop(agent);
    assert!(
        weak_agent.upgrade().is_none(),
        "policy state must not retain the production agent"
    );
    setup.kernel.shutdown().await;
}
