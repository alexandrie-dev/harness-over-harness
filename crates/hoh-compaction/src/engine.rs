use crate::CompactionResult;
use futures_util::future::BoxFuture;
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Context, Disposer, KernelError, Waterfall};
use hoh_session::{Session, SessionSeq};
use hoh_timeout::AbortSignal;
use std::{any::Any, fmt, sync::Arc};

/// Why automatic policy is asking a backend to consider compaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionTrigger {
    Pressure,
    ContextOverflow,
}

impl CompactionTrigger {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pressure => "pressure",
            Self::ContextOverflow => "context-overflow",
        }
    }
}

/// Expected failure classes for an explicit idle-session compaction request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualCompactionErrorCode {
    Busy,
    Cancelled,
    Changed,
    Summary,
    Commit,
    Persistence,
}

impl ManualCompactionErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Cancelled => "cancelled",
            Self::Changed => "changed",
            Self::Summary => "summary",
            Self::Commit => "commit",
            Self::Persistence => "persistence",
        }
    }
}

/// Expected manual-compaction failure suitable for a direct human-command result.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct ManualCompactionError {
    code: ManualCompactionErrorCode,
    message: String,
    #[source]
    cause: Option<SharedError>,
}

impl ManualCompactionError {
    pub const NAME: &'static str = "ManualCompactionError";

    pub fn new(code: ManualCompactionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_cause(mut self, cause: impl Into<BoxError>) -> Self {
        self.cause = Some(SharedError::new(cause));
        self
    }

    pub fn name(&self) -> &'static str {
        Self::NAME
    }

    pub fn code(&self) -> &'static str {
        self.code.as_str()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn cause(&self) -> Option<&SharedError> {
        self.cause.as_ref()
    }
}

/// Provider/model routing fields used by compaction without an agent-package dependency.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompactionAgentOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// Minimal agent context required by compaction.
#[derive(Clone, Debug)]
pub struct CompactionAgentContext {
    pub session: Session,
    pub options: CompactionAgentOptions,
}

pub type MaintenanceFuture = BoxFuture<'static, Result<Box<dyn Any + Send>, BoxError>>;
pub type MaintenanceTask = Box<dyn FnOnce(AbortSignal) -> MaintenanceFuture + Send>;
pub type MaintenanceRunner =
    Arc<dyn Fn(MaintenanceTask) -> Result<MaintenanceFuture, BoxError> + Send + Sync>;

/// Agent context capable of serializing one explicit idle-session compaction.
#[derive(Clone)]
pub struct ManualCompactAgentContext {
    base: CompactionAgentContext,
    runner: MaintenanceRunner,
}

impl fmt::Debug for ManualCompactAgentContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManualCompactAgentContext")
            .field("session", &self.base.session)
            .field("options", &self.base.options)
            .finish_non_exhaustive()
    }
}

impl ManualCompactAgentContext {
    pub fn new(
        session: Session,
        options: CompactionAgentOptions,
        runner: MaintenanceRunner,
    ) -> Self {
        Self {
            base: CompactionAgentContext { session, options },
            runner,
        }
    }

    pub fn as_compaction_context(&self) -> &CompactionAgentContext {
        &self.base
    }

    pub fn run_maintenance<T, F, Fut>(
        &self,
        task: F,
    ) -> Result<BoxFuture<'static, Result<T, BoxError>>, BoxError>
    where
        T: Send + 'static,
        F: FnOnce(AbortSignal) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, BoxError>> + Send + 'static,
    {
        let task: MaintenanceTask = Box::new(move |signal| {
            Box::pin(async move { Ok(Box::new(task(signal).await?) as Box<dyn Any + Send>) })
        });
        let future = (self.runner)(task)?;
        Ok(Box::pin(async move {
            future
                .await?
                .downcast::<T>()
                .map(|value| *value)
                .map_err(|_| BoxError::from("compaction maintenance result type mismatch"))
        }))
    }
}

impl std::ops::Deref for ManualCompactAgentContext {
    type Target = CompactionAgentContext;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl AsRef<CompactionAgentContext> for ManualCompactAgentContext {
    fn as_ref(&self) -> &CompactionAgentContext {
        &self.base
    }
}

/// Abstract compaction service implemented by backend crates.
pub trait CompactionEngine: Send + Sync + 'static {
    fn compact_if_needed<'a>(
        &'a self,
        agent: &'a CompactionAgentContext,
        trigger: CompactionTrigger,
        signal: AbortSignal,
    ) -> BoxFuture<'a, Result<Option<CompactionResult>, BoxError>>;

    fn compact_now<'a>(
        &'a self,
        agent: &'a ManualCompactAgentContext,
        signal: AbortSignal,
        source_command_id: Option<hoh_commands::CommandId>,
    ) -> BoxFuture<'a, Result<Option<CompactionResult>, BoxError>>;

    fn compact_region<'a>(
        &'a self,
        start: SessionSeq,
        end: SessionSeq,
        agent: &'a CompactionAgentContext,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<CompactionResult, BoxError>>;
}

hoh_kernel::service!(pub Compaction: dyn CompactionEngine = "compaction");

/// Publish one backend as `ctx.compaction`; disposal unregisters it.
pub fn provide_compaction(
    ctx: &Context,
    engine: Arc<dyn CompactionEngine>,
) -> impl std::future::Future<Output = Result<Disposer, KernelError>> + Send + 'static {
    ctx.provide::<Compaction>(engine)
}

/// Payload of the synchronous `compaction/summary-error` recovery waterfall.
pub struct CompactionSummaryError {
    pub session: Session,
    pub source_event_seqs: Vec<SessionSeq>,
    pub error: SharedError,
    pub signal: Option<AbortSignal>,
}

hoh_kernel::event!(
    CompactionSummaryError = "compaction/summary-error",
    Waterfall<bool>
);
