//! Background-job service definition (`jobs`) for HoH.

pub mod archive_admission;
pub mod brand;
pub mod error;
pub mod types;
pub mod view;

pub use brand::JobId;
pub use error::{JOB_ERROR_CODES, JobError, JobErrorCode};
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, KernelError, NoConfig, Plugin};
use hoh_session::SessionId;
use hoh_timeout::AbortSignal;
use std::sync::Arc;
pub use types::*;
pub use view::*;

/// Abstract background-job registry contract implemented by provider crates.
pub trait JobRegistryApi: Send + Sync + 'static {
    fn events(&self) -> &dyn JobEvents;
    fn start(&self, spec: JobSpec) -> Result<JobId, BoxError>;
    fn list(&self, caller: Option<&SessionId>) -> Vec<JobView>;
    fn get(&self, id: &JobId, caller: Option<&SessionId>) -> Result<JobView, BoxError>;
    fn read(&self, id: &JobId, caller: Option<&SessionId>) -> Result<JobRead, BoxError>;
    fn read_at(
        &self,
        id: &JobId,
        from: f64,
        caller: Option<&SessionId>,
    ) -> Result<JobOutputRead, BoxError>;
    fn kill(
        &self,
        id: &JobId,
        caller: Option<&SessionId>,
        reason: Option<&str>,
    ) -> Result<KillOutcome, BoxError>;
    fn wait(
        &self,
        id: &JobId,
        timeout_ms: f64,
        caller: Option<&SessionId>,
        signal: Option<AbortSignal>,
    ) -> JobFuture<'static, Result<JobView, BoxError>>;
    fn remove(&self, id: &JobId, caller: Option<&SessionId>) -> Result<(), BoxError>;
    fn attach_controller(&self, ctx: &Context, name: &str) -> Result<Disposer, KernelError>;
}

hoh_kernel::service!(pub Jobs: dyn JobRegistryApi = "jobs");

/// Install the mandatory archive listeners and provide one `jobs` service.
pub async fn provide_jobs(
    ctx: &Context,
    registry: Arc<dyn JobRegistryApi>,
) -> Result<Disposer, BoxError> {
    let _archive = archive_admission::install_job_archive_admission(ctx, registry.clone())?;
    Ok(ctx.provide::<Jobs>(registry).await?)
}

/// Fail-loud abstract seam. Load a concrete provider such as `hoh-jobs-local`.
pub struct JobRegistry;

impl Plugin for JobRegistry {
    const NAME: &'static str = "JobRegistry";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(
        _ctx: Context,
        _deps: Self::Deps,
        _config: hoh_kernel::Cfg<Self::Config>,
    ) -> Result<(), hoh_kernel::Error> {
        Err(std::io::Error::other(
            "@deepseek-ai/dsh-jobs is the abstract job registry seam; load an implementation such as @deepseek-ai/dsh-jobs-local instead",
        )
        .into())
    }
}
