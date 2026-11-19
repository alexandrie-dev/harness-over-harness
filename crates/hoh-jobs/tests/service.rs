use hoh_error::BoxError;
use hoh_jobs::{
    JobAppendOptions, JobChannel, JobEvent, JobEventFilter, JobEventListener, JobEventOwners,
    JobEvents, JobFuture, JobHandle, JobHooks, JobId, JobOutputRead, JobOutputSource,
    JobOutputView, JobRead, JobRegistry, JobRegistryApi, JobSourceRead, JobSpec, JobStatus,
    JobView, Jobs, KillOutcome, provide_jobs,
};
use hoh_kernel::{Context, Disposer, Kernel, KernelError, NoConfig, Plugin, Undo};
use hoh_session::SessionId;
use hoh_timeout::AbortSignal;
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct StubEvents {
    subscriptions: Mutex<Vec<JobEventFilter>>,
}

impl JobEvents for StubEvents {
    fn subscribe(
        &self,
        ctx: &Context,
        filter: JobEventFilter,
        _listener: JobEventListener,
    ) -> Result<Disposer, KernelError> {
        lock(&self.subscriptions).push(filter);
        ctx.effect("stub jobs subscription", Undo::none)
    }
}

#[derive(Default)]
struct StubJobRegistry {
    events: StubEvents,
}

impl StubJobRegistry {
    fn view(id: JobId, owner: Option<&SessionId>) -> JobView {
        JobView {
            id,
            kind: "bash".to_owned(),
            label: "sleep 60".to_owned(),
            owner: owner.cloned(),
            output_limit_bytes: None,
            status: JobStatus::Running,
            progress: None,
            detail: None,
            started_at: 0,
            finished_at: None,
            output: JobOutputView::default(),
        }
    }
}

impl JobRegistryApi for StubJobRegistry {
    fn events(&self) -> &dyn JobEvents {
        &self.events
    }

    fn start(&self, spec: JobSpec) -> Result<JobId, BoxError> {
        let id = JobId::from(format!("{}-1", spec.kind));
        let handle = JobHandle::new(id.clone(), |_, _| {}, |_| {});
        let _hooks = (spec.run)(handle)?;
        Ok(id)
    }

    fn list(&self, caller: Option<&SessionId>) -> Vec<JobView> {
        vec![Self::view("bash-1".into(), caller)]
    }

    fn get(&self, id: &JobId, caller: Option<&SessionId>) -> Result<JobView, BoxError> {
        Ok(Self::view(id.clone(), caller))
    }

    fn read(&self, id: &JobId, caller: Option<&SessionId>) -> Result<JobRead, BoxError> {
        Ok(JobRead {
            chunks: Vec::new(),
            lossy: false,
            result: None,
            job: Self::view(id.clone(), caller),
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
        _id: &JobId,
        _caller: Option<&SessionId>,
        _reason: Option<&str>,
    ) -> Result<KillOutcome, BoxError> {
        Ok(KillOutcome::Requested)
    }

    fn wait(
        &self,
        id: &JobId,
        _timeout_ms: f64,
        caller: Option<&SessionId>,
        _signal: Option<AbortSignal>,
    ) -> JobFuture<'static, Result<JobView, BoxError>> {
        let view = Self::view(id.clone(), caller);
        Box::pin(std::future::ready(Ok(view)))
    }

    fn remove(&self, _id: &JobId, _caller: Option<&SessionId>) -> Result<(), BoxError> {
        Ok(())
    }

    fn attach_controller(&self, ctx: &Context, _name: &str) -> Result<Disposer, KernelError> {
        ctx.effect("stub jobs controller", Undo::none)
    }
}

struct StubRegistryPlugin;

impl Plugin for StubRegistryPlugin {
    const NAME: &'static str = "StubJobRegistry";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(
        ctx: Context,
        _deps: Self::Deps,
        _config: hoh_kernel::Cfg<Self::Config>,
    ) -> Result<(), hoh_kernel::Error> {
        provide_jobs(&ctx, Arc::new(StubJobRegistry::default())).await?;
        Ok(())
    }
}

struct SecondRegistryPlugin;

impl Plugin for SecondRegistryPlugin {
    const NAME: &'static str = "SecondJobRegistry";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(
        ctx: Context,
        _deps: Self::Deps,
        _config: hoh_kernel::Cfg<Self::Config>,
    ) -> Result<(), hoh_kernel::Error> {
        provide_jobs(&ctx, Arc::new(StubJobRegistry::default())).await?;
        Ok(())
    }
}

async fn loaded() -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<StubRegistryPlugin>(serde_json::json!({}))
        .expect("stub registry plugin should be accepted");
    fiber
        .settled()
        .await
        .expect("stub registry plugin should load");
    (kernel, ctx)
}

fn pending_hooks() -> JobHooks {
    JobHooks {
        cancel: Box::new(|_| Ok(())),
        done: Box::pin(std::future::pending()),
    }
}

#[tokio::test]
async fn concrete_provider_registers_as_jobs_and_serves_the_abstract_api() {
    let (kernel, ctx) = loaded().await;
    let jobs = ctx.get::<Jobs>().expect("jobs service");
    let caller: SessionId = "session-1".into();
    let detach_controller = jobs
        .attach_controller(&ctx, "seam-test")
        .expect("controller attachment");
    let id = jobs
        .start(JobSpec {
            kind: "bash".to_owned(),
            label: "sleep 60".to_owned(),
            owner: Some(caller.clone()),
            tool_call: None,
            output_limit_bytes: None,
            output: None,
            run: Box::new(|_| Ok(pending_hooks())),
        })
        .expect("job starts");
    assert_eq!(id, "bash-1");
    assert_eq!(jobs.list(Some(&caller)).len(), 1);
    assert_eq!(
        jobs.get(&id, Some(&caller)).expect("job view").status,
        JobStatus::Running
    );
    assert_eq!(
        jobs.get(&id, Some(&caller)).expect("owned job").owner,
        Some(caller.clone())
    );
    let read = jobs.read(&id, Some(&caller)).expect("job read");
    assert!(read.chunks.is_empty());
    assert!(!read.lossy);
    assert_eq!(read.job.id, id);
    assert_eq!(
        jobs.read_at(&id, 7.0, Some(&caller))
            .expect("observer read"),
        JobOutputRead {
            chunks: Vec::new(),
            next: 7,
            lossy: false,
        }
    );
    assert_eq!(
        jobs.kill(&id, Some(&caller), Some("seam test"))
            .expect("kill request"),
        KillOutcome::Requested
    );
    assert_eq!(
        jobs.wait(&id, 5.0, Some(&caller), None)
            .await
            .expect("wait result")
            .id,
        id
    );
    assert_eq!(jobs.get(&id, None).expect("unowned projection").owner, None);
    let events = Mutex::new(Vec::<JobEvent>::new());
    let listener: JobEventListener = Arc::new(move |event| {
        lock(&events).push(event.clone());
        Ok(())
    });
    let unsubscribe = jobs
        .events()
        .subscribe(
            &ctx,
            JobEventFilter::Owners {
                owners: JobEventOwners::All,
            },
            listener,
        )
        .expect("event subscription");
    unsubscribe.dispose().await;
    detach_controller.dispose().await;
    kernel.shutdown().await;
}

#[test]
fn types_every_start_with_one_handle_and_optional_pull_sources() {
    let source = JobOutputSource {
        channel: Some(JobChannel::Stdout),
        read: Box::new(|from_byte| {
            Ok(JobSourceRead {
                text: String::new(),
                next_offset: from_byte,
                lossy: false,
                spill_path: None,
            })
        }),
    };
    let spec = JobSpec {
        kind: "bash".to_owned(),
        label: "x".to_owned(),
        owner: None,
        tool_call: None,
        output_limit_bytes: None,
        output: Some(vec![source]),
        run: Box::new(|job| {
            job.append_with_options(
                "lands in the ring",
                JobAppendOptions {
                    channel: Some(JobChannel::Log),
                    gap_before: false,
                },
            );
            job.update_progress("1/2");
            Ok(pending_hooks())
        }),
    };
    assert_eq!(spec.output.as_ref().map(Vec::len), Some(1));

    let handle = JobHandle::new("bash-1".into(), |_, _| {}, |_| {});
    handle.append("lands without options");
}

#[test]
fn keeps_projection_kind_an_open_string() {
    let view = StubJobRegistry::view("pwsh-1".into(), None);
    let mut open = view;
    open.kind = "kind-provided-by-another-crate".to_owned();
    assert_eq!(open.kind, "kind-provided-by-another-crate");
}

#[tokio::test]
async fn loading_a_second_implementation_throws() {
    let (kernel, ctx) = loaded().await;
    let second = ctx
        .plugin::<SecondRegistryPlugin>(serde_json::json!({}))
        .expect("second registry plugin should be accepted before activation");
    let error = second
        .settled()
        .await
        .expect_err("duplicate jobs service should fail");
    assert!(
        error
            .to_string()
            .contains("service \"jobs\" has been registered")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn mounting_the_abstract_seam_directly_fails_loudly_at_load() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let seam = ctx
        .plugin::<JobRegistry>(serde_json::json!({}))
        .expect("abstract seam plugin should be accepted before activation");
    let error = seam
        .settled()
        .await
        .expect_err("abstract seam should fail during activation");
    assert!(error.to_string().contains(
        "abstract job registry seam; load an implementation such as @deepseek-ai/dsh-jobs-local"
    ));
    kernel.shutdown().await;
}
