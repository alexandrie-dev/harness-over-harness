use crate::{
    JobEventFilter, JobEventListener, JobEvents, JobFuture, JobId, JobOutputRead, JobOutputView,
    JobRead, JobRegistryApi, JobSpec, JobStatus, Jobs, KillOutcome, provide_jobs,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, Kernel, KernelError, NoConfig, Plugin, Undo};
use hoh_session::SessionId;
use hoh_timeout::AbortSignal;
use hoh_workspace::{
    SessionActivity, SessionActivityItem, WorkspaceSessionActivity, WorkspaceSessionStop,
};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone)]
struct Row {
    id: JobId,
    label: String,
    owner: Option<SessionId>,
    status: JobStatus,
}

type KillCall = (JobId, Option<SessionId>, Option<String>);

#[derive(Default)]
struct ScriptedJobRegistry {
    rows: Mutex<Vec<Row>>,
    kills: Mutex<Vec<KillCall>>,
    refusing: Mutex<Option<JobId>>,
    events: NoopEvents,
}

#[derive(Default)]
struct NoopEvents;

impl JobEvents for NoopEvents {
    fn subscribe(
        &self,
        ctx: &Context,
        _filter: JobEventFilter,
        _listener: JobEventListener,
    ) -> Result<Disposer, KernelError> {
        ctx.effect("scripted jobs subscription", Undo::none)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl ScriptedJobRegistry {
    fn push(&self, rows: impl IntoIterator<Item = Row>) {
        lock(&self.rows).extend(rows);
    }

    fn kills(&self) -> Vec<KillCall> {
        lock(&self.kills).clone()
    }

    fn refuse(&self, id: JobId) {
        *lock(&self.refusing) = Some(id);
    }

    fn view(row: &Row) -> crate::JobView {
        crate::JobView {
            id: row.id.clone(),
            kind: "bash".to_owned(),
            label: row.label.clone(),
            owner: row.owner.clone(),
            output_limit_bytes: None,
            status: row.status,
            progress: None,
            detail: None,
            started_at: 0,
            finished_at: None,
            output: JobOutputView::default(),
        }
    }
}

impl JobRegistryApi for ScriptedJobRegistry {
    fn events(&self) -> &dyn JobEvents {
        &self.events
    }

    fn start(&self, _spec: JobSpec) -> Result<JobId, BoxError> {
        Err(std::io::Error::other("not scripted").into())
    }

    fn list(&self, caller: Option<&SessionId>) -> Vec<crate::JobView> {
        lock(&self.rows)
            .iter()
            .filter(|row| row.owner.is_none() || row.owner.as_ref() == caller)
            .map(Self::view)
            .collect()
    }

    fn get(&self, id: &JobId, _caller: Option<&SessionId>) -> Result<crate::JobView, BoxError> {
        lock(&self.rows)
            .iter()
            .find(|row| &row.id == id)
            .map(Self::view)
            .ok_or_else(|| std::io::Error::other(format!("unknown job {id}")).into())
    }

    fn read(&self, id: &JobId, caller: Option<&SessionId>) -> Result<JobRead, BoxError> {
        Ok(JobRead {
            chunks: Vec::new(),
            lossy: false,
            result: None,
            job: self.get(id, caller)?,
        })
    }

    fn read_at(
        &self,
        _id: &JobId,
        from: f64,
        _caller: Option<&SessionId>,
    ) -> Result<JobOutputRead, BoxError> {
        Ok(JobOutputRead {
            chunks: Vec::new(),
            next: from as u64,
            lossy: false,
        })
    }

    fn kill(
        &self,
        id: &JobId,
        caller: Option<&SessionId>,
        reason: Option<&str>,
    ) -> Result<KillOutcome, BoxError> {
        if lock(&self.refusing).as_ref() == Some(id) {
            return Err(std::io::Error::other("producer refuses cancel").into());
        }
        lock(&self.kills).push((id.clone(), caller.cloned(), reason.map(str::to_owned)));
        if let Some(row) = lock(&self.rows).iter_mut().find(|row| &row.id == id) {
            row.status = JobStatus::Stopping;
        }
        Ok(KillOutcome::Requested)
    }

    fn wait(
        &self,
        id: &JobId,
        _timeout_ms: f64,
        caller: Option<&SessionId>,
        _signal: Option<AbortSignal>,
    ) -> JobFuture<'static, Result<crate::JobView, BoxError>> {
        let result = self.get(id, caller);
        Box::pin(std::future::ready(result))
    }

    fn remove(&self, _id: &JobId, _caller: Option<&SessionId>) -> Result<(), BoxError> {
        Ok(())
    }

    fn attach_controller(&self, ctx: &Context, _name: &str) -> Result<Disposer, KernelError> {
        ctx.effect("scripted jobs controller", Undo::none)
    }
}

hoh_kernel::service!(ScriptedJobs: ScriptedJobRegistry = "scriptedJobs");

struct ScriptedRegistryPlugin;

impl Plugin for ScriptedRegistryPlugin {
    const NAME: &'static str = "ScriptedJobRegistry";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(
        ctx: Context,
        _deps: Self::Deps,
        _config: hoh_kernel::Cfg<Self::Config>,
    ) -> Result<(), hoh_kernel::Error> {
        let registry = Arc::new(ScriptedJobRegistry::default());
        ctx.provide::<ScriptedJobs>(registry.clone()).await?;
        provide_jobs(&ctx, registry).await?;
        Ok(())
    }
}

async fn harness() -> (Kernel, Context, hoh_kernel::Fiber) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<ScriptedRegistryPlugin>(serde_json::json!({}))
        .expect("scripted registry plugin should be accepted");
    fiber
        .settled()
        .await
        .expect("scripted registry plugin should load");
    (kernel, ctx, fiber)
}

async fn ask(ctx: &Context, session_id: SessionId) -> Vec<SessionActivity> {
    ctx.waterfall::<WorkspaceSessionActivity, Vec<SessionActivity>, _>(
        WorkspaceSessionActivity { session_id },
        |_| async { Ok(Vec::new()) },
    )
    .await
    .expect("activity waterfall should succeed")
}

async fn stop(ctx: &Context, session_id: SessionId) {
    ctx.parallel::<WorkspaceSessionStop>(WorkspaceSessionStop { session_id })
        .await
        .expect("session stop should succeed");
}

fn row(id: &str, label: &str, owner: Option<&SessionId>, status: JobStatus) -> Row {
    Row {
        id: id.into(),
        label: label.to_owned(),
        owner: owner.cloned(),
        status,
    }
}

#[tokio::test]
async fn reports_running_or_stopping_owned_jobs_never_unowned_foreign_or_settled() {
    let (kernel, ctx, _fiber) = harness().await;
    let jobs = ctx
        .get::<ScriptedJobs>()
        .expect("scripted registry service");
    let owner: SessionId = "owner".into();
    let other: SessionId = "other".into();
    jobs.push([
        row("bash-1", "pnpm run build", Some(&owner), JobStatus::Running),
        row("bash-2", "pnpm test", Some(&owner), JobStatus::Stopping),
        row("bash-3", "done", Some(&owner), JobStatus::Completed),
        row("bash-4", "theirs", Some(&other), JobStatus::Running),
        row("bash-5", "unowned", None, JobStatus::Running),
    ]);
    let turn = ctx
        .on_waterfall::<WorkspaceSessionActivity, Vec<SessionActivity>, _>(
            |_request, next| async move {
                let mut rest = next.run().await?;
                rest.push(SessionActivity {
                    kind: "turn".to_owned(),
                    items: None,
                });
                Ok(rest)
            },
        )
        .expect("turn activity listener");
    let activity = ask(&ctx, owner).await;
    assert_eq!(
        activity,
        vec![
            SessionActivity {
                kind: "job".to_owned(),
                items: Some(vec![
                    SessionActivityItem {
                        id: "bash-1".to_owned(),
                        label: Some("pnpm run build".to_owned()),
                    },
                    SessionActivityItem {
                        id: "bash-2".to_owned(),
                        label: Some("pnpm test".to_owned()),
                    },
                ]),
            },
            SessionActivity {
                kind: "turn".to_owned(),
                items: None,
            },
        ]
    );
    assert_eq!(
        ask(&ctx, "nobody".into()).await,
        vec![SessionActivity {
            kind: "turn".to_owned(),
            items: None,
        }]
    );
    turn.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test]
async fn kills_each_owned_live_job_and_keeps_going_when_one_producer_refuses() {
    let (kernel, ctx, _fiber) = harness().await;
    let jobs = ctx
        .get::<ScriptedJobs>()
        .expect("scripted registry service");
    let owner: SessionId = "owner".into();
    let other: SessionId = "other".into();
    jobs.push([
        row("bash-1", "stubborn", Some(&owner), JobStatus::Running),
        row("bash-2", "quiet", Some(&owner), JobStatus::Running),
        row("bash-3", "theirs", Some(&other), JobStatus::Running),
    ]);
    jobs.refuse("bash-1".into());

    stop(&ctx, owner.clone()).await;
    assert_eq!(
        jobs.kills(),
        vec![(
            JobId::from("bash-2"),
            Some(owner.clone()),
            Some("session archived".to_owned()),
        )]
    );
    assert_eq!(
        jobs.get(&"bash-3".into(), Some(&other))
            .expect("foreign row remains")
            .status,
        JobStatus::Running
    );
    let warnings = ctx
        .logger()
        .buffer()
        .iter()
        .filter(|record| record.message.contains("killing \"bash-1\""))
        .map(|record| record.message.clone())
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("producer refuses cancel"));

    stop(&ctx, owner).await;
    assert_eq!(jobs.kills().len(), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn stops_answering_once_the_registry_fiber_is_disposed() {
    let (kernel, ctx, fiber) = harness().await;
    let jobs = ctx
        .get::<ScriptedJobs>()
        .expect("scripted registry service");
    let owner: SessionId = "owner".into();
    jobs.push([row(
        "bash-1",
        "pnpm run build",
        Some(&owner),
        JobStatus::Running,
    )]);
    assert_eq!(
        ask(&ctx, owner.clone()).await,
        vec![SessionActivity {
            kind: "job".to_owned(),
            items: Some(vec![SessionActivityItem {
                id: "bash-1".to_owned(),
                label: Some("pnpm run build".to_owned()),
            }]),
        }]
    );
    fiber.dispose().await;
    assert!(ask(&ctx, owner.clone()).await.is_empty());
    stop(&ctx, owner).await;
    assert!(jobs.kills().is_empty());
    assert!(ctx.get::<Jobs>().is_none());
    kernel.shutdown().await;
}
