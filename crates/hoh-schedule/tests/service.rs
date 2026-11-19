use hoh_agent::AgentRegistryPlugin;
use hoh_kernel::{Context, Kernel};
use hoh_schedule::*;
use hoh_session::{SessionHeader, SessionId, SessionPlugin};
use hoh_session_persistence::SessionPersistenceService;
use hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin;
use hoh_session_projection::SessionProjectionRegistry;
use hoh_storage::StoragePlugin;
use hoh_storage_domain::StorageDomainPlugin;
use hoh_storage_json::StorageJsonPlugin;
use hoh_timeout::AbortController;
use hoh_workspace::WorkspaceRegistryPlugin;
use serde_json::json;
use tempfile::TempDir;

struct Harness {
    kernel: Kernel,
    ctx: Context,
    _root: TempDir,
    _schedule: hoh_kernel::Fiber,
}

impl Harness {
    async fn new() -> Self {
        let root = TempDir::new().unwrap();
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<StoragePlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<StorageJsonPlugin>(
            json!({"root":root.path().join("storage").to_string_lossy()}),
        )
        .unwrap()
        .settled()
        .await
        .unwrap();
        ctx.plugin::<StorageDomainPlugin>(json!({"backend":"json"}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<SessionPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<SessionProjectionRegistry>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<AgentRegistryPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<JsonlSessionPersistencePlugin>(
            json!({"root":root.path().join("sessions").to_string_lossy(),"compression":"none"}),
        )
        .unwrap()
        .settled()
        .await
        .unwrap();
        let persistence = ctx.get::<SessionPersistenceService>().unwrap();
        let header = SessionHeader::new(SessionId::from("session-existing"), 1);
        let handle = persistence.create(&header, None).await.unwrap();
        handle.flush(None).await.unwrap();
        handle.close().await.unwrap();
        ctx.plugin::<WorkspaceRegistryPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let schedule = ctx.plugin::<SchedulePlugin>(json!({})).unwrap();
        schedule.settled().await.unwrap();
        if schedule.state() != hoh_kernel::FiberState::Active {
            let snapshot = ctx.reflect().store().await;
            let info = snapshot
                .fibers
                .into_iter()
                .find(|fiber| fiber.uid == schedule.uid())
                .unwrap();
            panic!("schedule pending on {:?}", info.missing);
        }
        Self {
            kernel,
            ctx,
            _root: root,
            _schedule: schedule,
        }
    }

    fn service(&self) -> hoh_kernel::Service<Schedule> {
        self.ctx.get::<Schedule>().unwrap()
    }
}

fn future_request(title: &str) -> ScheduleCreateRequest {
    ScheduleCreateRequest {
        title: title.into(),
        prompt: "future reminder".into(),
        timing: ScheduleTiming::At {
            at: AtInput::Instant("2099-01-01T00:00:00.000Z".into()),
        },
        target: Some(ScheduleTargetRequest::Session {
            session_id: SessionId::from("session-existing"),
        }),
    }
}

#[tokio::test]
async fn management_create_list_view_and_delete_are_durable_operations() {
    let harness = Harness::new().await;
    let service = harness.service();
    let created = service
        .remote_create(future_request(" Future "))
        .await
        .unwrap();
    assert_eq!(created.record.title(), "Future");
    assert_eq!(created.status, ScheduleStatus::Active);
    assert!(matches!(created.target, ScheduleTarget::Session { .. }));

    let listed = service.list(ScheduleListRequest::default()).await.unwrap();
    assert_eq!(listed.total, 1);
    assert_eq!(listed.items[0].record.id(), created.record.id());
    assert!(matches!(
        service
            .view(ScheduleViewRequest {
                id: created.record.id().clone()
            })
            .await
            .unwrap(),
        ScheduleViewResult::Entry(_)
    ));

    let deleted = service
        .remote_edit(ScheduleEditRequest::Delete {
            id: created.record.id().clone(),
            expected_updated_at: Some(created.updated_at.clone()),
        })
        .await
        .unwrap();
    assert!(matches!(
        deleted,
        ScheduleEditResult::Deleted { deleted: true, .. }
    ));
    assert_eq!(
        service
            .list(ScheduleListRequest::default())
            .await
            .unwrap()
            .total,
        0
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn optimistic_edit_conflict_is_non_mutating() {
    let harness = Harness::new().await;
    let service = harness.service();
    let created = service
        .remote_create(future_request("Original"))
        .await
        .unwrap();
    let result = service
        .remote_edit(ScheduleEditRequest::Update {
            id: created.record.id().clone(),
            patch: SchedulePatch {
                title: Some("Changed".into()),
                ..SchedulePatch::default()
            },
            expected_updated_at: Some(Some("2000-01-01T00:00:00.000Z".into())),
        })
        .await
        .unwrap();
    assert!(matches!(result, ScheduleEditResult::Conflict(_)));
    let ScheduleViewResult::Entry(viewed) = service
        .view(ScheduleViewRequest {
            id: created.record.id().clone(),
        })
        .await
        .unwrap()
    else {
        panic!("task disappeared")
    };
    assert_eq!(viewed.record.title(), "Original");
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn list_validates_page_bounds_before_reading() {
    let harness = Harness::new().await;
    let error = harness
        .service()
        .list(ScheduleListRequest {
            limit: Some(501),
            offset: None,
            active: None,
        })
        .await
        .unwrap_err();
    let error = error.downcast::<ScheduleInputError>().unwrap();
    assert_eq!(error.code(), ScheduleInputErrorCode::InvalidRule);
    assert_eq!(
        error.to_string(),
        "limit must be 1-500 and offset must be a non-negative integer."
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn implicit_new_session_target_requires_a_session_creation_provider() {
    let harness = Harness::new().await;
    let mut request = future_request("Needs provider");
    request.target = None;
    let error = harness.service().remote_create(request).await.unwrap_err();
    let error = error.downcast::<hoh_error::HarnessError>().unwrap();
    assert_eq!(error.name(), "RemoteError");
    assert_eq!(error.code(), "schedule/input-invalid");
    assert_eq!(
        error.message(),
        "This Host cannot create Sessions; choose an existing Session target."
    );
    assert_eq!(
        error.detail::<serde_json::Value>(),
        Some(&json!({"code":"invalid_rule"}))
    );
    assert_eq!(
        harness
            .service()
            .list(ScheduleListRequest::default())
            .await
            .unwrap()
            .total,
        0
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn an_aborted_create_keeps_the_abort_reason_and_does_not_write() {
    let harness = Harness::new().await;
    let controller = AbortController::new();
    controller.abort();
    let expected = controller.signal().reason().unwrap();
    let error = harness
        .service()
        .create(future_request("Cancelled"), None, Some(controller.signal()))
        .await
        .unwrap_err();
    assert!(hoh_timeout::is_abort_error(error.as_ref()));
    assert!(expected.ptr_eq(hoh_error::SharedError::of(error.as_ref()).unwrap()));
    assert_eq!(
        harness
            .service()
            .list(ScheduleListRequest::default())
            .await
            .unwrap()
            .total,
        0
    );
    harness.kernel.shutdown().await;
}

#[tokio::test]
async fn plugin_teardown_closes_the_service_before_future_mutations() {
    let harness = Harness::new().await;
    let service = harness.service();
    harness._schedule.dispose().await;
    assert_eq!(
        service
            .remote_create(future_request("Too late"))
            .await
            .unwrap_err()
            .to_string(),
        "Schedule service is stopping"
    );
    harness.kernel.shutdown().await;
}
