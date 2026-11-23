//! Session working directories, durable changes, and model context.
//!
//! Port of `@deepseek-ai/dsh-working-directory` (`packages/session/working-directory`).

pub mod error;
pub mod shim;

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use hoh_error::{BoxError, HarnessError};
use hoh_fs::{Fs, FsResolveOptions, FsType};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo, Waterfall};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};
use hoh_session::{Session, SessionEvent, SessionId};
use hoh_system_prompt::{
    AssembledContext, PromptContext, PromptContextOrderName, PromptPresentation, PromptText,
    SystemPrompt, SystemPromptAssemble, SystemPromptKey,
};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_util_values::js_json_stringify;
use hoh_util_workspace_path::node_path::is_absolute;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Notify, oneshot};

pub use error::{WORKING_DIRECTORY_ERROR_CODES, WorkingDirectoryError, WorkingDirectoryErrorCode};
use hoh_agent::{Agent, Agents, AssembleAgent};
use hoh_session_projection::{
    ProjectionDefinition, ProjectionHandle, ProjectionWire, SessionProjections,
};
use shim::AsLiveAgent;

/// Name of the model-context entry this service contributes.
const CONTEXT_NAME: &str = "working-directory:current";

/// `working-directory/access`: coordinate directory validation and publication with a resource
/// operation on the directory, such as a checkout being removed. A listener that must wait awaits
/// that operation, then runs `next`; every other listener runs `next` at once. A rejection fails
/// this access and leaves the recorded directory unchanged; it is not a fallback.
///
/// `path` is the requested path (`None` validates the current directory); `signal` is the
/// access's cancellation. The waterfall yields the published directory.
pub struct WorkingDirectoryAccess {
    /// Owner of the Session whose directory is accessed.
    pub agent: Arc<dyn Agent>,
    /// The requested path; `None` validates the current directory.
    pub path: Option<String>,
    /// The access's cancellation.
    pub signal: AbortSignal,
}
hoh_kernel::event!(
    WorkingDirectoryAccess = "working-directory/access",
    Waterfall<String>
);

/// Session event `working-directory/change`: effective working directory; the immutable Session
/// header retains the original project. `cwd` is the absolute directory used by subsequent
/// directory-based operations.
pub const WORKING_DIRECTORY_CHANGE: &str = "working-directory/change";

/// Message source kind of committed working-directory transition notices.
pub const WORKING_DIRECTORY_SOURCE: &str = "working-directory";

/// Projection key (`SessionProjectionMap.workingDirectory`): the recorded current directory for
/// Session observations, or `null` when none is recorded.
pub const WORKING_DIRECTORY_PROJECTION: &str = "workingDirectory";

/// Deployment default for Sessions without an original project directory.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Absolute execution-world fallback directory; omitted values use the launch directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_directory: Option<String>,
}

/// One owner for each Session's effective directory and its model-visible changes.
#[derive(Clone)]
pub struct WorkingDirectoryService {
    inner: Arc<Inner>,
}

hoh_kernel::service!(pub WorkingDirectoryKey: WorkingDirectoryService = "workingDirectory");

/// Services the plugin needs (`static inject = ['fs', 'sessionProjections', 'systemPrompt']`).
#[derive(Inject)]
pub struct Deps {
    fs: Service<Fs>,
    projections: Service<SessionProjections>,
    system_prompt: Service<SystemPromptKey>,
}

/// The plugin: registers the projection, the model context and the assembly listener, then
/// provides [`WorkingDirectoryService`].
pub struct WorkingDirectory;

impl Plugin for WorkingDirectory {
    const NAME: &'static str = "WorkingDirectoryService";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let service = WorkingDirectoryService::start(ctx.clone(), deps, &config.get()).await?;
        ctx.provide::<WorkingDirectoryKey>(Arc::new(service))
            .await?;
        Ok(())
    }
}

/// The `workingDirectory` projection: state is the recorded absolute directory or `null`.
fn directory_projection() -> ProjectionDefinition<Option<String>> {
    let schema = |value: Option<&Value>| -> Result<Arc<Option<String>>, BoxError> {
        match value {
            Some(Value::Null) => Ok(Arc::new(None)),
            Some(Value::String(directory)) if !directory.is_empty() && is_absolute(directory) => {
                Ok(Arc::new(Some(directory.clone())))
            }
            _ => Err(
                "working-directory: the recorded directory must be an absolute path or null".into(),
            ),
        }
    };
    ProjectionDefinition {
        key: WORKING_DIRECTORY_PROJECTION.into(),
        state_version: 1,
        state_schema: Arc::new(schema),
        init: Arc::new(|header, _| Ok(Arc::new(header.cwd.clone()))),
        apply: Arc::new(|state, event: &SessionEvent| {
            if event.event_type() == WORKING_DIRECTORY_CHANGE {
                Ok(Arc::new(
                    event
                        .data()
                        .get("cwd")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                ))
            } else {
                Ok(state.clone())
            }
        }),
        wire: Some(ProjectionWire::new(
            Arc::new(move |value| schema(value).map(|state| Arc::new(json!(*state)))),
            Arc::new(|state| Ok(Some(Arc::new(json!(**state))))),
        )),
    }
}

/// The live Agent that owns `session`, from the `agents` registry. A tool execution still carries
/// only a session-level view of its agent (`hoh_tools::Agent`); the working directory and the
/// approval service need the whole Agent (context, inbox), which the registry holds under the
/// session id. `None` when no registry is mounted or the Agent is not registered.
pub fn live_agent(ctx: &Context, session: &Session) -> Option<Arc<dyn Agent>> {
    ctx.get::<Agents>()?.get(session.id())
}

/// Completion of one queued operation: dropping the sender wakes the next operation.
type Done = oneshot::Receiver<()>;

struct Queued {
    generation: u64,
    tail: Done,
}

#[derive(Default)]
struct Queues {
    next_generation: u64,
    tails: HashMap<SessionId, Queued>,
}

#[derive(Default)]
struct Operations {
    running: Mutex<usize>,
    idle: Notify,
    /// Live relay tasks (the chain links of operations cancelled while queued).
    relays: AtomicUsize,
}

/// One live relay task; counted until it ends.
struct RelayCount(Arc<Operations>);

impl RelayCount {
    fn new(operations: &Arc<Operations>) -> Self {
        operations.relays.fetch_add(1, Ordering::SeqCst);
        Self(operations.clone())
    }
}

impl Drop for RelayCount {
    fn drop(&mut self) {
        self.0.relays.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Inner {
    ctx: Context,
    fs: Service<Fs>,
    default_directory: String,
    initialized_default_directory: OnceLock<String>,
    handle: ProjectionHandle<Option<String>>,
    lifecycle: AbortController,
    queues: Mutex<Queues>,
    operations: Arc<Operations>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `JSON.stringify(text)`.
fn json_string(text: &str) -> String {
    js_json_stringify(&Value::String(text.to_owned()))
}

/// `String(error)` of a caught value.
fn error_string(error: &BoxError) -> String {
    match HarnessError::of(error.as_ref()) {
        Some(harness) => format!("{}: {}", harness.name(), harness.message()),
        None => format!("Error: {error}"),
    }
}

impl WorkingDirectoryService {
    /// Validate the configuration and register everything the service contributes.
    async fn start(ctx: Context, deps: Deps, config: &Config) -> Result<Self, BoxError> {
        let directory = match &config.default_directory {
            Some(directory) => directory.clone(),
            None => std::env::current_dir()?.to_string_lossy().into_owned(),
        };
        if !is_absolute(&directory) {
            return Err("working-directory: defaultDirectory must be absolute".into());
        }
        let handle = deps
            .projections
            .register(&ctx, directory_projection())
            .await?;
        let inner = Arc::new(Inner {
            ctx: ctx.clone(),
            fs: deps.fs,
            default_directory: directory,
            initialized_default_directory: OnceLock::new(),
            handle,
            lifecycle: AbortController::new(),
            queues: Mutex::new(Queues::default()),
            operations: Arc::new(Operations::default()),
        });

        let context_inner = inner.clone();
        deps.system_prompt.context(
            &ctx,
            PromptContext {
                name: CONTEXT_NAME.into(),
                order: SystemPrompt::get_context_order(PromptContextOrderName::WorkingDirectory),
                text: PromptText::provider(move |context| {
                    Ok(match context.extension::<AssembleAgent>() {
                        None => String::new(),
                        Some(agent) => context_inner.context_text(agent.0.session()),
                    })
                }),
                interpolate: Some(false),
                required: true,
            },
        )?;

        let listener_inner = inner.clone();
        ctx.listen()
            .prepend()
            .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(move |event, next| {
                let inner = listener_inner.clone();
                async move {
                    let Some(agent) = event
                        .context
                        .extension::<AssembleAgent>()
                        .map(|agent| agent.0.clone())
                    else {
                        return next.run().await;
                    };
                    let signal = event.context.signal.clone();
                    WorkingDirectoryService {
                        inner: inner.clone(),
                    }
                    .ensure(&agent, signal.as_ref())
                    .await?;
                    let assembly = next.run().await?;
                    inner.lifecycle.signal().throw_if_aborted()?;
                    let text = inner.context_text(agent.session());
                    assembly.update(|data| {
                        match data
                            .contexts
                            .iter_mut()
                            .find(|entry| entry.name == CONTEXT_NAME)
                        {
                            None => data.contexts.insert(
                                0,
                                AssembledContext {
                                    name: CONTEXT_NAME.into(),
                                    text,
                                    interpolate: Some(false),
                                },
                            ),
                            Some(contribution) => {
                                contribution.text = text;
                                contribution.interpolate = Some(false);
                            }
                        }
                    });
                    Ok(assembly)
                }
            })?;

        let dispose_inner = inner.clone();
        ctx.effect("workingDirectory.dispose", move || {
            Undo::future(move || async move {
                dispose_inner
                    .lifecycle
                    .abort_with(WorkingDirectoryError::new(
                        "working-directory service disposed",
                        WorkingDirectoryErrorCode::Unavailable,
                    ));
                dispose_inner.operations.wait_idle().await;
                Ok(())
            })
        })?;
        Ok(Self { inner })
    }

    /// Number of live relay tasks: chain links of changes cancelled while queued, each held only
    /// until its predecessor settles or the service is disposed. For tests.
    #[doc(hidden)]
    pub fn live_relay_tasks(&self) -> usize {
        self.inner.operations.relays.load(Ordering::SeqCst)
    }

    /// Absolute deployment fallback for Sessions with no recorded original directory.
    pub fn default_directory(&self) -> &str {
        self.inner.default_directory()
    }

    /// Set the launch fallback once the SDK initialization route has been validated.
    pub fn set_initial_default_directory(&self, directory: String) {
        debug_assert!(is_absolute(&directory));
        let _ = self.inner.initialized_default_directory.set(directory);
    }

    /// Read the committed directory without filesystem I/O.
    ///
    /// `session` is the Session whose directory is requested; the result is its effective absolute
    /// directory.
    pub fn get(&self, session: &Session) -> String {
        self.inner.get(session)
    }

    /// The recorded current directory of `session` as the `workingDirectory` projection view
    /// carries it: `None` when none is recorded.
    pub fn recorded(&self, session: &Session) -> Option<String> {
        self.inner.recorded(session)
    }

    /// Validate the current directory and restore the original project when it disappeared.
    ///
    /// `agent` is the live or unpublished Agent owning the Session; `signal` cancels filesystem
    /// inspection. Returns the existing directory; recovery is committed before fulfillment. A
    /// notice failure is warned without reverting the committed state.
    ///
    /// # Errors
    /// `WORKING_DIRECTORY_UNAVAILABLE` when the original project is also unavailable or the service
    /// is disposed, or the rejection of a `working-directory/access` listener.
    pub fn ensure(
        &self,
        agent: &impl AsLiveAgent,
        signal: Option<&AbortSignal>,
    ) -> impl Future<Output = Result<String, BoxError>> + Send + 'static {
        let inner = self.inner.clone();
        let owner = agent.live();
        self.inner
            .serialize(owner.clone(), signal, move |control| async move {
                let event = WorkingDirectoryAccess {
                    agent: owner.clone(),
                    path: None,
                    signal: control.clone(),
                };
                let tail = inner.clone();
                inner
                    .ctx
                    .waterfall(event, move |_| async move {
                        tail.ensure_now(&owner, &control).await
                    })
                    .await
            })
    }

    /// Change one Session's directory without changing existing processes or permissions.
    ///
    /// `agent` is the live or unpublished Agent owning the Session; `path` is absolute or relative
    /// to its current directory; `signal` cancels before the durable change. Returns the canonical
    /// absolute directory, committed before fulfillment. A notice failure is warned; the next
    /// request still receives the committed directory.
    ///
    /// # Errors
    /// `WORKING_DIRECTORY_INVALID` for an empty path and `WORKING_DIRECTORY_NOT_FOUND` when the path
    /// is not an existing directory, or the rejection of a `working-directory/access` listener.
    pub fn set(
        &self,
        agent: &impl AsLiveAgent,
        path: &str,
        signal: Option<&AbortSignal>,
    ) -> impl Future<Output = Result<String, BoxError>> + Send + 'static {
        let inner = self.inner.clone();
        let owner = agent.live();
        let path = path.to_owned();
        self.inner
            .serialize(owner.clone(), signal, move |control| async move {
                let event = WorkingDirectoryAccess {
                    agent: owner.clone(),
                    path: Some(path.clone()),
                    signal: control.clone(),
                };
                let tail = inner.clone();
                inner
                    .ctx
                    .waterfall(event, move |_| async move {
                        tail.set_now(&owner, &path, &control).await
                    })
                    .await
            })
    }
}

impl Operations {
    fn begin(self: &Arc<Self>) -> OperationGuard {
        *lock(&self.running) += 1;
        OperationGuard(self.clone())
    }

    /// `await Promise.allSettled(this.operations)`.
    async fn wait_idle(&self) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if *lock(&self.running) == 0 {
                return;
            }
            notified.await;
        }
    }
}

/// One tracked operation: dropped when the operation settles or its future is dropped.
struct OperationGuard(Arc<Operations>);

impl Drop for OperationGuard {
    fn drop(&mut self) {
        let mut running = lock(&self.0.running);
        *running -= 1;
        if *running == 0 {
            self.0.idle.notify_waiters();
        }
    }
}

impl Inner {
    fn default_directory(&self) -> &str {
        self.initialized_default_directory
            .get()
            .unwrap_or(&self.default_directory)
    }

    /// The directory the projection records for `session`; a projection that cannot be read is
    /// warned about and treated as unrecorded.
    fn recorded(&self, session: &Session) -> Option<String> {
        match self.handle.state_of(session) {
            Ok(state) => (*state).clone(),
            Err(error) => {
                self.ctx.logger().warn(format_args!(
                    "working-directory: cannot read the recorded directory: {}",
                    error_string(&error)
                ));
                None
            }
        }
    }

    fn get(&self, session: &Session) -> String {
        self.recorded(session)
            .unwrap_or_else(|| self.default_directory().to_owned())
    }

    fn context_text(&self, session: &Session) -> String {
        format!(
            "Current working directory: {}.",
            json_string(&self.get(session))
        )
    }

    /// Resolve `path` against `cwd` and require an existing directory; `missing` builds the
    /// refusal. Returns the process path of the directory.
    async fn directory(
        &self,
        path: &str,
        cwd: &str,
        signal: &AbortSignal,
        missing: impl FnOnce() -> HarnessError,
    ) -> Result<String, BoxError> {
        let target = self
            .fs
            .resolve(
                path,
                FsResolveOptions {
                    cwd: Some(cwd.to_owned()),
                    signal: Some(signal.clone()),
                },
            )
            .await?;
        let info = self.fs.stat(&target, Some(signal.clone())).await?;
        signal.throw_if_aborted()?;
        if !info.is_some_and(|info| info.r#type == FsType::Directory) {
            return Err(missing().into());
        }
        Ok(self.fs.process_path(&target))
    }

    async fn ensure_now(
        &self,
        agent: &Arc<dyn Agent>,
        control: &AbortSignal,
    ) -> Result<String, BoxError> {
        let session = agent.session();
        let current = self.get(session);
        let target = self
            .fs
            .resolve(
                &current,
                FsResolveOptions {
                    cwd: None,
                    signal: Some(control.clone()),
                },
            )
            .await?;
        let info = self.fs.stat(&target, Some(control.clone())).await?;
        control.throw_if_aborted()?;
        agent.ctx().ensure_active()?;
        if info.is_some_and(|info| info.r#type == FsType::Directory) {
            if self.recorded(session).is_none() {
                session.append(WORKING_DIRECTORY_CHANGE, json!({ "cwd": current }), None)?;
            }
            return Ok(current);
        }
        let original = session
            .header()
            .get("cwd")
            .and_then(Value::as_str)
            .map_or_else(|| self.default_directory().to_owned(), str::to_owned);
        let restored = self
            .directory(&original, &original, control, || {
                WorkingDirectoryError::new(
                    format!(
                        "working-directory: directory does not exist: {}; the current working directory {} is also unavailable",
                        json_string(&original),
                        json_string(&current),
                    ),
                    WorkingDirectoryErrorCode::Unavailable,
                )
            })
            .await?;
        self.commit(agent, &restored, true, control)?;
        Ok(restored)
    }

    async fn set_now(
        &self,
        agent: &Arc<dyn Agent>,
        path: &str,
        control: &AbortSignal,
    ) -> Result<String, BoxError> {
        if path.is_empty() {
            return Err(WorkingDirectoryError::new(
                "working-directory: cd must not be empty",
                WorkingDirectoryErrorCode::Invalid,
            )
            .into());
        }
        let directory = self
            .directory(path, &self.get(agent.session()), control, || {
                WorkingDirectoryError::new(
                    format!(
                        "working-directory: directory does not exist: {}",
                        json_string(path)
                    ),
                    WorkingDirectoryErrorCode::NotFound,
                )
            })
            .await?;
        self.commit(agent, &directory, false, control)?;
        Ok(directory)
    }

    fn commit(
        &self,
        agent: &Arc<dyn Agent>,
        directory: &str,
        recovered: bool,
        signal: &AbortSignal,
    ) -> Result<(), BoxError> {
        signal.throw_if_aborted()?;
        agent.ctx().ensure_active()?;
        let session = agent.session();
        let previous = self.get(session);
        if previous == directory {
            return Ok(());
        }
        session.append(WORKING_DIRECTORY_CHANGE, json!({ "cwd": directory }), None)?;
        let text = if recovered {
            format!(
                "The working directory {} is unavailable. The working directory is now {}.",
                json_string(&previous),
                json_string(directory),
            )
        } else {
            format!(
                "The working directory changed from {} to {}.",
                json_string(&previous),
                json_string(directory),
            )
        };
        let notice = create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(text)],
            source: MessageSource::new(WORKING_DIRECTORY_SOURCE),
        });
        if let Err(error) = agent.inject(notice) {
            self.ctx.logger().warn(format_args!(
                "working-directory: committed {}, but could not queue its notice: {}",
                json_string(directory),
                error_string(&error),
            ));
        }
        Ok(())
    }

    /// Run `action` after every earlier operation on the same Session has settled, whatever its
    /// outcome. The place in the queue is taken when this is called, not when the future is first
    /// polled, like the promise chain of the source.
    fn serialize<T, F, Fut>(
        self: &Arc<Self>,
        agent: Arc<dyn Agent>,
        signal: Option<&AbortSignal>,
        action: F,
    ) -> impl Future<Output = Result<T, BoxError>> + Send + 'static
    where
        T: Send + 'static,
        F: FnOnce(AbortSignal) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, BoxError>> + Send + 'static,
    {
        let session = agent.session();
        let control = match signal {
            None => self.lifecycle.signal(),
            Some(signal) => AbortSignal::any([self.lifecycle.signal(), signal.clone()]),
        };
        let operations = self.operations.clone();
        let (done, tail) = oneshot::channel::<()>();
        let (previous, generation) = {
            let mut queues = lock(&self.queues);
            queues.next_generation += 1;
            let generation = queues.next_generation;
            let previous = queues
                .tails
                .insert(session.id().clone(), Queued { generation, tail })
                .map(|queued| queued.tail);
            (previous, generation)
        };
        let inner = self.clone();
        let id = session.id().clone();
        let owner = agent;
        async move {
            // Counted from the first poll, not from the call: a returned future nobody polls must
            // not keep `dispose` waiting. The position in the queue is still taken at the call.
            let _operation = operations.begin();
            let mut done = Some(done);
            // An operation cancelled while it waits fails at once but stays a link of the chain
            // until its predecessor settles (a promise chain of the source): its place is handed to
            // a task that releases it then, and the tail entry is left for the next operation.
            let mut cancelled_in_queue = false;
            if let Some(mut previous) = previous {
                // `Err` means the earlier operation settled by dropping its sender. An abort (the
                // caller's or the service's disposal) ends this operation's own wait, so a
                // predecessor that is never polled cannot hold disposal.
                tokio::select! {
                    _ = &mut previous => {}
                    _ = control.cancelled() => {
                        cancelled_in_queue = true;
                        if let Some(link) = done.take() {
                            // Counted by disposal and ended by it: the relay waits for the
                            // predecessor or the service's lifecycle, whichever comes first.
                            let tracked = operations.begin();
                            let relay = RelayCount::new(&operations);
                            let lifecycle = inner.lifecycle.signal();
                            tokio::spawn(async move {
                                let _tracked = tracked;
                                let _relay = relay;
                                tokio::select! {
                                    _ = previous => {}
                                    _ = lifecycle.cancelled() => {}
                                }
                                drop(link);
                            });
                        }
                    }
                }
            }
            let result = async {
                control.throw_if_aborted()?;
                owner.ctx().ensure_active()?;
                action(control).await
            }
            .await;
            if !cancelled_in_queue {
                let mut queues = lock(&inner.queues);
                if queues
                    .tails
                    .get(&id)
                    .is_some_and(|queued| queued.generation == generation)
                {
                    queues.tails.remove(&id);
                }
            }
            drop(done);
            result
        }
    }
}
