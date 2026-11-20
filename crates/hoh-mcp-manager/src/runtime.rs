use crate::{
    Config, McpManagedServerEntry, McpManagedServerStatus, McpManagedServerView, McpManagerStatus,
    McpTransport,
    config::{validate_entry, validate_servers},
};
use hoh_credentials::{
    CredentialFuture, CredentialInfo, CredentialKey, CredentialMutator, CredentialProvider,
    CredentialRecord, CredentialRecordEntry, CredentialRecordInfo, CredentialRef, Credentials,
    ResolvedCredential,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Fiber, Inject, Plugin, Service, Undo, VolatileUpdate};
use hoh_mcp_client::{
    Config as ClientConfig, DEFAULT_CLOSE_TIMEOUT_MS, DEFAULT_TOOL_CALL_TIMEOUT_MS, McpCloseResult,
    McpConnectionStatus, McpConnectionStatusHandle, McpFailureReason, McpProbeOptions,
    McpProbeResult, connection_status_of, probe_server,
};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tools::Tools;
use indexmap::IndexMap;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::{mpsc, oneshot, watch};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum McpManagerError {
    #[error("mcp-manager: the manager is disposed")]
    Disposed,
    #[error("mcp-manager: unknown server \"{0}\"")]
    UnknownServer(String),
    #[error("mcp-manager: unknown server \"{0}\" — it is neither configured nor registered")]
    UnknownRestartServer(String),
    #[error("mcp-manager: configuration for \"{0}\" is changing")]
    Changing(String),
    #[error("mcp-manager: cleanup for \"{0}\" is incomplete; restart the Host")]
    Cleanup(String),
    #[error("mcp-manager: server \"{0}\" is already configured or registered")]
    Duplicate(String),
    #[error("servers.{0}: a plugin already registers this server name — rename the entry key")]
    RegistrationConflict(String),
    #[error("mcp-manager: operation queue stopped")]
    QueueStopped,
    #[error("{0}")]
    Invalid(String),
}

struct ManagedRecord {
    entry: McpManagedServerEntry,
    generation: u64,
    changing: bool,
    fiber: Option<Fiber>,
    listener: Option<hoh_mcp_client::status::StatusListener>,
    failure: Option<McpCloseResult>,
}

struct State {
    stopped: bool,
    configured: IndexMap<String, McpManagedServerEntry>,
    registrations: IndexMap<String, McpManagedServerEntry>,
    records: IndexMap<String, ManagedRecord>,
    statuses: IndexMap<String, McpManagedServerStatus>,
    probes: HashMap<(String, u64), ProbeWork>,
    next_id: u64,
}

struct ProbeWork {
    controller: Arc<AbortController>,
    done: watch::Receiver<bool>,
}

struct ProbeCompletion {
    inner: std::sync::Weak<Inner>,
    key: (String, u64),
    done: watch::Sender<bool>,
}

impl Drop for ProbeCompletion {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            lock(&inner.state).probes.remove(&self.key);
        }
        self.done.send_replace(true);
    }
}

struct Inner {
    owner: Context,
    state: Mutex<State>,
    changed: Mutex<Option<watch::Sender<Vec<McpManagedServerView>>>>,
    commands: mpsc::UnboundedSender<Command>,
    lifetime: AbortController,
}

enum Command {
    Reconcile(Option<oneshot::Sender<Result<(), McpManagerError>>>),
    Restart(String, oneshot::Sender<Result<(), McpManagerError>>),
    Unregister(String, oneshot::Sender<()>),
    MountSettled {
        name: String,
        entry: Box<McpManagedServerEntry>,
        generation: u64,
        fiber: Fiber,
        result: Result<(), String>,
    },
}

#[derive(Clone)]
pub struct McpServerManager {
    inner: Arc<Inner>,
}

impl McpServerManager {
    pub fn list(&self) -> Vec<McpManagedServerView> {
        views(&lock(&self.inner.state))
    }

    pub fn watch(&self) -> watch::Receiver<Vec<McpManagedServerView>> {
        if let Some(sender) = lock(&self.inner.changed).as_ref() {
            sender.subscribe()
        } else {
            let (sender, receiver) = watch::channel(self.list());
            drop(sender);
            receiver
        }
    }

    pub async fn restart(&self, name: &str) -> Result<(), McpManagerError> {
        if lock(&self.inner.state).stopped {
            return Err(McpManagerError::Disposed);
        }
        let (send, receive) = oneshot::channel();
        self.inner
            .commands
            .send(Command::Restart(name.to_owned(), send))
            .map_err(|_| McpManagerError::QueueStopped)?;
        receive.await.unwrap_or_else(|_| {
            if lock(&self.inner.state).stopped {
                Err(McpManagerError::Disposed)
            } else {
                Err(McpManagerError::QueueStopped)
            }
        })
    }

    pub async fn probe(
        &self,
        name: &str,
        mut options: McpProbeOptions,
    ) -> Result<McpProbeResult, McpManagerError> {
        let (entry, controller, completion) = {
            let mut state = lock(&self.inner.state);
            if state.stopped {
                return Err(McpManagerError::Disposed);
            }
            let entry = current_entry(&state, name)
                .cloned()
                .ok_or_else(|| McpManagerError::UnknownServer(name.to_owned()))?;
            if state.records.get(name).is_some_and(|record| {
                record.changing || (record.entry != entry && record.fiber.is_some())
            }) {
                return Err(McpManagerError::Changing(name.to_owned()));
            }
            if state
                .records
                .get(name)
                .and_then(|record| record.failure.as_ref())
                .is_some()
            {
                return Err(McpManagerError::Cleanup(name.to_owned()));
            }
            let id = state.next_id;
            state.next_id = state.next_id.wrapping_add(1);
            if !state.records.contains_key(name) {
                state.records.insert(
                    name.to_owned(),
                    ManagedRecord {
                        entry: entry.clone(),
                        generation: id,
                        changing: false,
                        fiber: None,
                        listener: None,
                        failure: None,
                    },
                );
            }
            let controller = Arc::new(AbortController::new());
            let (done, waiting) = watch::channel(false);
            let key = (name.to_owned(), id);
            state.probes.insert(
                key.clone(),
                ProbeWork {
                    controller: controller.clone(),
                    done: waiting,
                },
            );
            let completion = ProbeCompletion {
                inner: Arc::downgrade(&self.inner),
                key,
                done,
            };
            (entry, controller, completion)
        };
        let caller = options.signal.take();
        options.signal = Some(AbortSignal::any(
            [
                Some(controller.signal()),
                Some(self.inner.lifetime.signal()),
                caller,
            ]
            .into_iter()
            .flatten(),
        ));
        let provider = self
            .inner
            .owner
            .get::<Credentials>()
            .map(|service| Arc::new(CredentialService(service)) as Arc<dyn CredentialProvider>);
        let result = probe_server(provider, client_config(name, &entry), options)
            .await
            .map_err(|error| McpManagerError::Invalid(error.to_string()))?;
        if !result.close_confirmed || result.cleanup_error.is_some() {
            record_probe_failure(
                &self.inner,
                name,
                entry,
                McpCloseResult {
                    close_confirmed: result.close_confirmed,
                    cleanup_error: result.cleanup_error.clone(),
                },
            );
        }
        drop(completion);
        Ok(result)
    }

    pub fn register(
        &self,
        owner: &Context,
        name: impl Into<String>,
        mut entry: McpManagedServerEntry,
    ) -> Result<Disposer, McpManagerError> {
        let name = name.into();
        entry.enabled = true;
        validate_entry(&name, &entry)
            .map_err(|error| McpManagerError::Invalid(error.to_string()))?;
        {
            let mut state = lock(&self.inner.state);
            if state.stopped {
                return Err(McpManagerError::Disposed);
            }
            if state.configured.contains_key(&name) || state.registrations.contains_key(&name) {
                return Err(McpManagerError::Duplicate(name));
            }
            state.registrations.insert(name.clone(), entry);
        }
        publish_views(&self.inner);
        let _ = self.inner.commands.send(Command::Reconcile(None));
        let inner = self.inner.clone();
        let effect_name = name.clone();
        match owner.effect("mcpManager.register()", move || {
            Undo::future(move || async move {
                let (send, receive) = oneshot::channel();
                {
                    let mut state = lock(&inner.state);
                    state.registrations.shift_remove(&effect_name);
                    cancel_probes(&mut state, &effect_name);
                    if let Some(record) = state.records.get_mut(&effect_name) {
                        record.changing = true;
                        record.listener.take();
                    }
                }
                publish_views(&inner);
                if inner
                    .commands
                    .send(Command::Unregister(effect_name, send))
                    .is_ok()
                {
                    let _ = receive.await;
                }
                Ok(())
            })
        }) {
            Ok(disposer) => Ok(disposer),
            Err(error) => {
                {
                    let mut state = lock(&self.inner.state);
                    state.registrations.shift_remove(&name);
                    cancel_probes(&mut state, &name);
                    if let Some(record) = state.records.get_mut(&name) {
                        record.changing = true;
                        record.listener.take();
                    }
                }
                publish_views(&self.inner);
                let _ = self.inner.commands.send(Command::Reconcile(None));
                Err(McpManagerError::Invalid(error.to_string()))
            }
        }
    }
}

hoh_kernel::service!(pub McpManager: McpServerManager = "mcpManager");

#[derive(Inject)]
pub struct Deps {
    pub tools: Service<Tools>,
}

impl Plugin for McpManager {
    const NAME: &'static str = "mcp-manager";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let _ = deps.tools;
        validate_servers(&config.get().servers)?;
        let (commands, receiver) = mpsc::unbounded_channel();
        let initial = config.get().servers.0.clone();
        let (changed, _) = watch::channel(Vec::new());
        let inner = Arc::new(Inner {
            owner: ctx.clone(),
            state: Mutex::new(State {
                stopped: false,
                configured: initial,
                registrations: IndexMap::new(),
                records: IndexMap::new(),
                statuses: IndexMap::new(),
                probes: HashMap::new(),
                next_id: 1,
            }),
            changed: Mutex::new(Some(changed)),
            commands,
            lifetime: AbortController::new(),
        });
        publish_views(&inner);
        let candidate_inner = inner.clone();
        ctx.on_candidate::<Config>(move |candidate| {
            validate_servers(&candidate.servers)?;
            let state = lock(&candidate_inner.state);
            for name in candidate.servers.keys() {
                if state.registrations.contains_key(name) {
                    return Err(Box::new(McpManagerError::RegistrationConflict(
                        name.clone(),
                    )));
                }
            }
            Ok(())
        })?;
        let update_inner = inner.clone();
        let update_config = config.clone();
        ctx.on::<VolatileUpdate>(move |_| {
            let next = update_config.get().servers.0.clone();
            replace_config(&update_inner, next);
        })?;
        let actor_inner = inner.clone();
        ctx.spawn("mcp-manager.actor", move |token| async move {
            run_actor(actor_inner, receiver, token).await;
            Ok(())
        })?;
        ctx.provide::<McpManager>(Arc::new(McpServerManager { inner }))
            .await?;
        Ok(())
    }
}

fn replace_config(inner: &Arc<Inner>, next: IndexMap<String, McpManagedServerEntry>) {
    {
        let mut state = lock(&inner.state);
        let changed = state
            .configured
            .iter()
            .filter(|(name, old)| next.get(*name) != Some(*old))
            .map(|(name, _)| name.clone())
            .chain(
                next.keys()
                    .filter(|name| !state.configured.contains_key(*name))
                    .cloned(),
            )
            .collect::<Vec<_>>();
        for name in changed {
            cancel_probes(&mut state, &name);
            if let Some(record) = state.records.get_mut(&name) {
                record.changing = true;
                record.listener.take();
            }
        }
        state.configured = next;
    }
    publish_views(inner);
    let _ = inner.commands.send(Command::Reconcile(None));
}

async fn run_actor(
    inner: Arc<Inner>,
    mut receiver: mpsc::UnboundedReceiver<Command>,
    token: tokio_util::sync::CancellationToken,
) {
    let stop_watcher = tokio::spawn({
        let inner = inner.clone();
        let token = token.clone();
        async move {
            token.cancelled().await;
            signal_stop(&inner);
        }
    });
    let _ = reconcile(&inner).await;
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            command = receiver.recv() => match command {
                Some(Command::Reconcile(reply)) => {
                    let result = reconcile(&inner).await;
                    if let Some(reply) = reply { let _ = reply.send(result); }
                }
                Some(Command::Restart(name, reply)) => { let _ = reply.send(remount(&inner, &name).await); }
                Some(Command::Unregister(name, reply)) => {
                    let _ = unmount_name(&inner, &name).await;
                    let _ = reconcile(&inner).await;
                    let _ = reply.send(());
                }
                Some(Command::MountSettled { name, entry, generation, fiber, result }) => {
                    finish_mount(&inner, name, *entry, generation, fiber, result).await;
                }
                None => break,
            }
        }
    }
    signal_stop(&inner);
    stop_watcher.abort();
    stop(&inner).await;
}

async fn reconcile(inner: &Arc<Inner>) -> Result<(), McpManagerError> {
    let names = {
        let state = lock(&inner.state);
        state.records.keys().cloned().collect::<Vec<_>>()
    };
    for name in names {
        let remove = {
            let state = lock(&inner.state);
            let current = current_entry(&state, &name);
            state.records.get(&name).is_some_and(|record| {
                record.changing
                    || current.is_none_or(|entry| {
                        entry != &record.entry || (!entry.enabled && record.fiber.is_some())
                    })
            })
        };
        if remove {
            unmount_name(inner, &name).await?;
        }
    }
    let entries = {
        let state = lock(&inner.state);
        all_entries(&state)
    };
    for (name, entry) in entries {
        let (has_fiber, failed) = {
            let state = lock(&inner.state);
            (
                state
                    .records
                    .get(&name)
                    .and_then(|record| record.fiber.as_ref())
                    .is_some(),
                state
                    .records
                    .get(&name)
                    .and_then(|r| r.failure.as_ref())
                    .is_some(),
            )
        };
        if failed {
            continue;
        }
        if !entry.enabled {
            set_status(inner, &name, None, McpManagedServerStatus::Disabled);
        } else if !has_fiber {
            mount(inner, name, entry);
        }
    }
    prune(inner);
    Ok(())
}

async fn remount(inner: &Arc<Inner>, name: &str) -> Result<(), McpManagerError> {
    let entry = {
        let state = lock(&inner.state);
        if state.stopped {
            return Err(McpManagerError::Disposed);
        }
        current_entry(&state, name)
            .cloned()
            .ok_or_else(|| McpManagerError::UnknownRestartServer(name.to_owned()))?
    };
    // Even a record already marked with incomplete cleanup may still own a live Fiber. The
    // source always calls unmount before reporting Cleanup, so restart must make the same final
    // disposal attempt instead of abandoning the child's tools and registrations.
    if !unmount_name(inner, name).await? {
        return Err(McpManagerError::Cleanup(name.to_owned()));
    }
    if entry.enabled {
        mount(inner, name.to_owned(), entry);
    } else {
        set_status(inner, name, None, McpManagedServerStatus::Disabled);
    }
    Ok(())
}

fn mount(inner: &Arc<Inner>, name: String, entry: McpManagedServerEntry) {
    let generation = {
        let mut state = lock(&inner.state);
        if state.stopped || current_entry(&state, &name) != Some(&entry) {
            return;
        }
        let generation = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        if let Some(record) = state.records.get_mut(&name) {
            if record.changing || record.failure.is_some() || record.fiber.is_some() {
                return;
            }
            record.entry = entry.clone();
            record.generation = generation;
            record.listener.take();
        } else {
            state.records.insert(
                name.clone(),
                ManagedRecord {
                    entry: entry.clone(),
                    generation,
                    changing: false,
                    fiber: None,
                    listener: None,
                    failure: None,
                },
            );
        }
        generation
    };
    set_status(
        inner,
        &name,
        Some(generation),
        McpManagedServerStatus::Connection(McpConnectionStatus::Connecting { attempt: 0 }),
    );
    let raw = match serde_json::to_value(client_config(&name, &entry)) {
        Ok(raw) => raw,
        Err(error) => {
            fail_load(inner, &name, generation, error.to_string());
            return;
        }
    };
    let fiber = match inner.owner.plugin::<hoh_mcp_client::McpClient>(raw) {
        Ok(fiber) => fiber,
        Err(error) => {
            fail_load(inner, &name, generation, error.to_string());
            return;
        }
    };
    let retained = {
        let mut state = lock(&inner.state);
        state
            .records
            .get_mut(&name)
            .filter(|r| r.generation == generation)
            .map(|record| {
                record.fiber = Some(fiber.clone());
            })
            .is_some()
    };
    if !retained {
        let stale = fiber.clone();
        tokio::spawn(async move { stale.dispose().await });
        return;
    }
    let commands = inner.commands.clone();
    tokio::spawn(async move {
        let result = fiber.settled().await.map_err(|error| error.to_string());
        let _ = commands.send(Command::MountSettled {
            name,
            entry: Box::new(entry),
            generation,
            fiber,
            result,
        });
    });
}

async fn finish_mount(
    inner: &Arc<Inner>,
    name: String,
    entry: McpManagedServerEntry,
    generation: u64,
    fiber: Fiber,
    result: Result<(), String>,
) {
    let disposition = {
        let state = lock(&inner.state);
        mount_disposition(&state, &name, &entry, generation)
    };
    match disposition {
        MountDisposition::Current => {}
        MountDisposition::RetiredOwned => {
            let _ = unmount_name(inner, &name).await;
            return;
        }
        MountDisposition::Stale => {
            fiber.dispose().await;
            return;
        }
    }
    if let Err(error) = result {
        finish_failed_mount(inner, &name, generation, error).await;
        return;
    }
    let Some(cell) = connection_status_of(&fiber) else {
        finish_failed_mount(
            inner,
            &name,
            generation,
            "mcp-client activation did not publish connection status".into(),
        )
        .await;
        return;
    };
    let status_inner = inner.clone();
    let status_name = name.clone();
    let status_cell = cell.clone();
    let listener = cell.on_change(Arc::new(move |status| {
        if matches!(
            &status,
            McpConnectionStatus::Failed {
                reason: McpFailureReason::UnclosedGeneration,
                ..
            }
        ) {
            record_unclosed_generation(
                &status_inner,
                &status_name,
                generation,
                status_cell.closure(),
                status,
            );
        } else {
            set_status(
                &status_inner,
                &status_name,
                Some(generation),
                McpManagedServerStatus::Connection(status),
            );
        }
    }));
    let active = attach_status_listener(inner, &name, &entry, generation, listener);
    if active {
        let status = cell.current();
        if matches!(
            &status,
            McpConnectionStatus::Failed {
                reason: McpFailureReason::UnclosedGeneration,
                ..
            }
        ) {
            record_unclosed_generation(inner, &name, generation, cell.closure(), status);
        } else {
            set_status(
                inner,
                &name,
                Some(generation),
                McpManagedServerStatus::Connection(status),
            );
        }
    } else {
        fiber.dispose().await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MountDisposition {
    Current,
    RetiredOwned,
    Stale,
}

fn mount_disposition(
    state: &State,
    name: &str,
    entry: &McpManagedServerEntry,
    generation: u64,
) -> MountDisposition {
    let Some(record) = state
        .records
        .get(name)
        .filter(|record| record.generation == generation)
    else {
        return MountDisposition::Stale;
    };
    if state.stopped || record.changing || current_entry(state, name) != Some(entry) {
        MountDisposition::RetiredOwned
    } else {
        MountDisposition::Current
    }
}

fn attach_status_listener(
    inner: &Arc<Inner>,
    name: &str,
    entry: &McpManagedServerEntry,
    generation: u64,
    listener: hoh_mcp_client::status::StatusListener,
) -> bool {
    let mut state = lock(&inner.state);
    if state.stopped || current_entry(&state, name) != Some(entry) {
        return false;
    }
    let Some(record) = state
        .records
        .get_mut(name)
        .filter(|record| record.generation == generation && !record.changing)
    else {
        return false;
    };
    record.listener = Some(listener);
    true
}

async fn finish_failed_mount(inner: &Arc<Inner>, name: &str, generation: u64, error: String) {
    if unmount_name(inner, name).await.unwrap_or(false) {
        fail_load(inner, name, generation, error);
    }
}

async fn unmount_name(inner: &Arc<Inner>, name: &str) -> Result<bool, McpManagerError> {
    {
        let mut state = lock(&inner.state);
        if let Some(record) = state.records.get_mut(name) {
            record.changing = true;
            record.listener.take();
        }
        cancel_probes(&mut state, name);
    }
    wait_probes(inner, name).await;
    let taken = {
        let mut state = lock(&inner.state);
        state.records.get_mut(name).map(|record| {
            (
                record.generation,
                record.fiber.take(),
                record.listener.take(),
                record.failure.clone(),
            )
        })
    };
    let Some((generation, fiber, listener, failure)) = taken else {
        return Ok(true);
    };
    drop(listener);
    let Some(fiber) = fiber else {
        if let Some(failure) = failure {
            retain_failure(inner, name, generation, failure);
            return Ok(false);
        }
        remove_record(inner, name, generation);
        return Ok(true);
    };
    fiber.dispose().await;
    let close = connection_status_of(&fiber).map_or(
        McpCloseResult {
            close_confirmed: true,
            cleanup_error: None,
        },
        |cell| cell.closure(),
    );
    let close = merge_close(failure, close);
    if !close.close_confirmed || close.cleanup_error.is_some() {
        retain_failure(inner, name, generation, close);
        Ok(false)
    } else {
        remove_record(inner, name, generation);
        Ok(true)
    }
}

fn remove_record(inner: &Arc<Inner>, name: &str, generation: u64) {
    let mut state = lock(&inner.state);
    if state
        .records
        .get(name)
        .is_some_and(|record| record.generation == generation)
    {
        state.records.shift_remove(name);
    }
    drop(state);
    publish_views(inner);
}

fn signal_stop(inner: &Arc<Inner>) {
    {
        let mut state = lock(&inner.state);
        state.stopped = true;
        for record in state.records.values_mut() {
            record.changing = true;
            record.listener.take();
        }
        for probe in state.probes.values() {
            probe.controller.abort();
        }
    }
    inner.lifetime.abort();
    lock(&inner.changed).take();
}

async fn stop(inner: &Arc<Inner>) {
    signal_stop(inner);
    let mut waiting = lock(&inner.state)
        .probes
        .values()
        .map(|probe| probe.done.clone())
        .collect::<Vec<_>>();
    wait_for_probe_receivers(&mut waiting).await;
    let names = lock(&inner.state)
        .records
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for name in names {
        let _ = unmount_name(inner, &name).await;
    }
}

fn fail_load(inner: &Arc<Inner>, name: &str, generation: u64, error: String) {
    {
        let mut state = lock(&inner.state);
        if state
            .records
            .get(name)
            .is_some_and(|r| r.generation == generation)
        {
            state.records.shift_remove(name);
        }
    }
    set_status(
        inner,
        name,
        None,
        McpManagedServerStatus::LoadFailed { last_error: error },
    );
}

fn retain_failure(inner: &Arc<Inner>, name: &str, generation: u64, result: McpCloseResult) {
    let (status, emit) = {
        let mut state = lock(&inner.state);
        if state
            .records
            .get(name)
            .is_some_and(|record| record.generation != generation)
        {
            return;
        }
        let entry = current_entry(&state, name)
            .cloned()
            .unwrap_or_else(|| McpManagedServerEntry::stdio("retired"));
        let previous = state.records.get(name).and_then(|r| r.failure.clone());
        let result = merge_close(previous, result);
        state.records.insert(
            name.to_owned(),
            ManagedRecord {
                entry,
                generation,
                changing: false,
                fiber: None,
                listener: None,
                failure: Some(result.clone()),
            },
        );
        let status = McpManagedServerStatus::CleanupFailed { result };
        let changed = state.statuses.get(name) != Some(&status);
        state.statuses.insert(name.to_owned(), status.clone());
        (status, changed && !state.stopped)
    };
    publish_views(inner);
    if emit {
        emit_status(inner, name, status);
    }
}

fn record_unclosed_generation(
    inner: &Arc<Inner>,
    name: &str,
    generation: u64,
    result: McpCloseResult,
    connection_status: McpConnectionStatus,
) {
    let (status, emit) = {
        let mut state = lock(&inner.state);
        let stopped = state.stopped;
        let Some(record) = state
            .records
            .get_mut(name)
            .filter(|record| record.generation == generation && !record.changing)
        else {
            return;
        };
        let previous = record.failure.clone();
        let result = merge_close(previous.clone(), result);
        record.failure = Some(result.clone());
        record.listener.take();
        let status = if previous.is_some() {
            McpManagedServerStatus::CleanupFailed { result }
        } else {
            McpManagedServerStatus::Connection(connection_status)
        };
        let changed = state.statuses.get(name) != Some(&status);
        state.statuses.insert(name.to_owned(), status.clone());
        (status, changed && !stopped)
    };
    publish_views(inner);
    if emit {
        emit_status(inner, name, status);
    }
}

fn record_probe_failure(
    inner: &Arc<Inner>,
    name: &str,
    fallback_entry: McpManagedServerEntry,
    result: McpCloseResult,
) {
    let (status, emit) = {
        let mut state = lock(&inner.state);
        let previous = state
            .records
            .get(name)
            .and_then(|record| record.failure.clone());
        let result = merge_close(previous, result);
        let current = current_entry(&state, name)
            .cloned()
            .unwrap_or(fallback_entry);
        let generation = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        let record = state
            .records
            .entry(name.to_owned())
            .or_insert(ManagedRecord {
                entry: current,
                generation,
                changing: false,
                fiber: None,
                listener: None,
                failure: None,
            });
        record.failure = Some(result.clone());
        let status = McpManagedServerStatus::CleanupFailed { result };
        let changed = state.statuses.get(name) != Some(&status);
        state.statuses.insert(name.to_owned(), status.clone());
        (status, changed && !state.stopped)
    };
    publish_views(inner);
    if emit {
        emit_status(inner, name, status);
    }
}

fn set_status(
    inner: &Arc<Inner>,
    name: &str,
    generation: Option<u64>,
    status: McpManagedServerStatus,
) {
    let changed = {
        let mut state = lock(&inner.state);
        if state.stopped || current_entry(&state, name).is_none() {
            return;
        }
        if let Some(generation) = generation
            && !state.records.get(name).is_some_and(|record| {
                record.generation == generation && !record.changing && record.failure.is_none()
            })
        {
            return;
        }
        if state.statuses.get(name) == Some(&status) {
            false
        } else {
            state.statuses.insert(name.to_owned(), status.clone());
            true
        }
    };
    if !changed {
        return;
    }
    publish_views(inner);
    emit_status(inner, name, status);
}

fn emit_status(inner: &Arc<Inner>, name: &str, status: McpManagedServerStatus) {
    let ctx = inner.owner.clone();
    let event_name = name.to_owned();
    let event = Arc::new(McpManagerStatus {
        name: event_name.clone(),
        status,
    });
    tokio::spawn(async move {
        if let Err(error) = ctx.parallel::<McpManagerStatus>(event).await {
            ctx.logger().error(format_args!(
                "mcp-manager: a mcp-manager/status listener failed for \"{event_name}\""
            ));
            ctx.logger().error(error);
        }
    });
}

fn prune(inner: &Arc<Inner>) {
    let mut state = lock(&inner.state);
    let names = state.statuses.keys().cloned().collect::<Vec<_>>();
    for name in names {
        if current_entry(&state, &name).is_none() && !state.records.contains_key(&name) {
            state.statuses.shift_remove(&name);
        }
    }
    drop(state);
    publish_views(inner);
}

fn current_entry<'a>(state: &'a State, name: &str) -> Option<&'a McpManagedServerEntry> {
    state
        .configured
        .get(name)
        .or_else(|| state.registrations.get(name))
}

fn all_entries(state: &State) -> IndexMap<String, McpManagedServerEntry> {
    state
        .configured
        .iter()
        .chain(state.registrations.iter())
        .map(|(name, entry)| (name.clone(), entry.clone()))
        .collect()
}

fn views(state: &State) -> Vec<McpManagedServerView> {
    let mut result = Vec::new();
    for (name, entry) in state.configured.iter().chain(state.registrations.iter()) {
        let status = state.statuses.get(name).cloned().unwrap_or({
            if entry.enabled {
                McpManagedServerStatus::Connection(McpConnectionStatus::Connecting { attempt: 0 })
            } else {
                McpManagedServerStatus::Disabled
            }
        });
        result.push(McpManagedServerView {
            name: name.clone(),
            configured: true,
            status,
        });
    }
    for (name, record) in &state.records {
        if current_entry(state, name).is_none() {
            let status = state.statuses.get(name).cloned().unwrap_or_else(|| {
                McpManagedServerStatus::CleanupFailed {
                    result: record.failure.clone().unwrap_or_default(),
                }
            });
            result.push(McpManagedServerView {
                name: name.clone(),
                configured: false,
                status,
            });
        }
    }
    result
}

fn publish_views(inner: &Arc<Inner>) {
    let next = views(&lock(&inner.state));
    let changed = lock(&inner.changed);
    if let Some(sender) = changed.as_ref()
        && *sender.borrow() != next
    {
        sender.send_replace(next);
    }
}

fn cancel_probes(state: &mut State, name: &str) {
    for ((server, _), probe) in &state.probes {
        if server == name {
            probe.controller.abort();
        }
    }
}

async fn wait_probes(inner: &Arc<Inner>, name: &str) {
    let mut waiting = {
        let state = lock(&inner.state);
        state
            .probes
            .iter()
            .filter(|((server, _), _)| server == name)
            .map(|(_, probe)| {
                probe.controller.abort();
                probe.done.clone()
            })
            .collect::<Vec<_>>()
    };
    wait_for_probe_receivers(&mut waiting).await;
}

async fn wait_for_probe_receivers(waiting: &mut [watch::Receiver<bool>]) {
    for receiver in waiting {
        while !*receiver.borrow() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    }
}

fn merge_close(previous: Option<McpCloseResult>, mut next: McpCloseResult) -> McpCloseResult {
    if let Some(previous) = previous {
        next.close_confirmed &= previous.close_confirmed;
        if previous.cleanup_error.is_some() {
            next.cleanup_error = previous.cleanup_error;
        }
    }
    next
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use hoh_kernel::{Cfg, Kernel, NoConfig};
    use serde_json::json;
    use std::sync::OnceLock;
    use tokio::sync::Semaphore;

    struct IdlePlugin;

    impl Plugin for IdlePlugin {
        const NAME: &'static str = "mcp-manager-test-idle";
        type Config = NoConfig;
        type Deps = ();

        async fn apply(_: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct CleanupGate {
        started: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    fn cleanup_gate() -> &'static Mutex<Option<CleanupGate>> {
        static GATE: OnceLock<Mutex<Option<CleanupGate>>> = OnceLock::new();
        GATE.get_or_init(Default::default)
    }

    struct GatedCleanupPlugin;

    impl Plugin for GatedCleanupPlugin {
        const NAME: &'static str = "mcp-manager-test-gated-cleanup";
        type Config = NoConfig;
        type Deps = ();

        async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
            let gate = lock(cleanup_gate())
                .clone()
                .expect("test installs cleanup gate before mounting");
            ctx.effect("gated cleanup", move || {
                Undo::future(move || async move {
                    gate.started.add_permits(1);
                    gate.release.acquire().await.unwrap().forget();
                    Ok(())
                })
            })?;
            Ok(())
        }
    }

    fn test_inner(owner: Context) -> Arc<Inner> {
        let (commands, _) = mpsc::unbounded_channel();
        let (changed, _) = watch::channel(Vec::new());
        Arc::new(Inner {
            owner,
            state: Mutex::new(State {
                stopped: false,
                configured: IndexMap::new(),
                registrations: IndexMap::new(),
                records: IndexMap::new(),
                statuses: IndexMap::new(),
                probes: HashMap::new(),
                next_id: 1,
            }),
            changed: Mutex::new(Some(changed)),
            commands,
            lifetime: AbortController::new(),
        })
    }

    fn actor_inner(owner: Context) -> (Arc<Inner>, mpsc::UnboundedReceiver<Command>) {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (changed, _) = watch::channel(Vec::new());
        (
            Arc::new(Inner {
                owner,
                state: Mutex::new(State {
                    stopped: false,
                    configured: IndexMap::new(),
                    registrations: IndexMap::new(),
                    records: IndexMap::new(),
                    statuses: IndexMap::new(),
                    probes: HashMap::new(),
                    next_id: 1,
                }),
                changed: Mutex::new(Some(changed)),
                commands,
                lifetime: AbortController::new(),
            }),
            receiver,
        )
    }

    fn add_gated_probe(
        inner: &Arc<Inner>,
        name: &str,
        id: u64,
    ) -> (watch::Sender<bool>, AbortSignal) {
        let controller = Arc::new(AbortController::new());
        let signal = controller.signal();
        let (done, waiting) = watch::channel(false);
        lock(&inner.state).probes.insert(
            (name.to_owned(), id),
            ProbeWork {
                controller,
                done: waiting,
            },
        );
        (done, signal)
    }

    async fn wait_until_aborted(signal: &AbortSignal) {
        for _ in 0..100 {
            if signal.aborted() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("probe was not aborted");
    }

    #[tokio::test]
    async fn stop_waits_for_a_probe_without_a_managed_record() {
        let kernel = Kernel::start();
        let inner = test_inner(kernel.root());
        let (done, signal) = add_gated_probe(&inner, "off", 1);

        let stopping = tokio::spawn({
            let inner = inner.clone();
            async move { stop(&inner).await }
        });
        wait_until_aborted(&signal).await;
        assert!(!stopping.is_finished());

        done.send_replace(true);
        stopping.await.unwrap();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn stop_closes_watch_before_probe_cleanup_settles() {
        let kernel = Kernel::start();
        let inner = test_inner(kernel.root());
        let mut receiver = McpServerManager {
            inner: inner.clone(),
        }
        .watch();
        let (done, signal) = add_gated_probe(&inner, "off", 1);

        let stopping = tokio::spawn({
            let inner = inner.clone();
            async move { stop(&inner).await }
        });
        wait_until_aborted(&signal).await;
        assert!(!stopping.is_finished());
        assert!(receiver.changed().await.is_err());

        done.send_replace(true);
        stopping.await.unwrap();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn cancellation_signals_stop_while_actor_is_blocked_in_child_cleanup() {
        let kernel = Kernel::start();
        let root = kernel.root();
        let gate = CleanupGate {
            started: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
        };
        *lock(cleanup_gate()) = Some(gate.clone());
        let fiber = root.plugin::<GatedCleanupPlugin>(json!({})).unwrap();
        fiber.settled().await.unwrap();

        let (inner, receiver) = actor_inner(root.clone());
        let previous = McpManagedServerEntry::stdio("old");
        let current = McpManagedServerEntry::stdio("new");
        let other = McpManagedServerEntry {
            enabled: false,
            ..McpManagedServerEntry::stdio("other")
        };
        {
            let mut state = lock(&inner.state);
            state.configured.insert("srv".into(), current);
            state.configured.insert("other".into(), other);
            state.records.insert(
                "srv".into(),
                ManagedRecord {
                    entry: previous,
                    generation: 7,
                    changing: true,
                    fiber: Some(fiber),
                    listener: None,
                    failure: None,
                },
            );
        }
        let (probe_done, probe_signal) = add_gated_probe(&inner, "other", 8);
        let manager = McpServerManager {
            inner: inner.clone(),
        };
        let mut views = manager.watch();
        let token = tokio_util::sync::CancellationToken::new();
        let actor = tokio::spawn(run_actor(inner.clone(), receiver, token.clone()));
        gate.started.acquire().await.unwrap().forget();

        token.cancel();
        for _ in 0..100 {
            if lock(&inner.state).stopped {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(lock(&inner.state).stopped);
        assert!(probe_signal.aborted());
        assert_eq!(
            manager.probe("other", McpProbeOptions::default()).await,
            Err(McpManagerError::Disposed)
        );
        assert!(matches!(
            manager.register(&root, "new", McpManagedServerEntry::stdio("new")),
            Err(McpManagerError::Disposed)
        ));
        assert!(views.changed().await.is_err());
        assert!(!actor.is_finished());

        probe_done.send_replace(true);
        gate.release.add_permits(1);
        actor.await.unwrap();
        *lock(cleanup_gate()) = None;
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn reconcile_replaces_a_retired_record_after_a_to_b_to_a_coalesces() {
        let kernel = Kernel::start();
        let root = kernel.root();
        let old_fiber = root.plugin::<IdlePlugin>(json!({})).unwrap();
        old_fiber.settled().await.unwrap();
        let inner = test_inner(root);
        let entry = McpManagedServerEntry::stdio("/definitely/not/a/real/mcp-server");
        {
            let mut state = lock(&inner.state);
            state.configured.insert("srv".into(), entry.clone());
            state.next_id = 8;
            state.records.insert(
                "srv".into(),
                ManagedRecord {
                    entry: entry.clone(),
                    generation: 7,
                    changing: true,
                    fiber: Some(old_fiber.clone()),
                    listener: None,
                    failure: None,
                },
            );
        }

        reconcile(&inner).await.unwrap();

        assert!(old_fiber.is_disposed());
        {
            let state = lock(&inner.state);
            let replacement = &state.records["srv"];
            assert_eq!(replacement.entry, entry);
            assert!(replacement.generation > 7);
            assert!(!replacement.changing);
            assert!(replacement.fiber.is_some());
            assert_ne!(replacement.fiber.as_ref().unwrap().uid(), old_fiber.uid());
        }
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn restart_disposes_a_live_fiber_before_reporting_retained_cleanup_failure() {
        let kernel = Kernel::start();
        let root = kernel.root();
        let fiber = root.plugin::<IdlePlugin>(json!({})).unwrap();
        fiber.settled().await.unwrap();
        let inner = test_inner(root);
        let entry = McpManagedServerEntry::stdio("unused");
        {
            let mut state = lock(&inner.state);
            state.configured.insert("srv".into(), entry.clone());
            state.records.insert(
                "srv".into(),
                ManagedRecord {
                    entry,
                    generation: 7,
                    changing: false,
                    fiber: Some(fiber.clone()),
                    listener: None,
                    failure: Some(McpCloseResult {
                        close_confirmed: false,
                        cleanup_error: Some("previous cleanup failed".into()),
                    }),
                },
            );
        }

        assert_eq!(
            remount(&inner, "srv").await,
            Err(McpManagerError::Cleanup("srv".into()))
        );
        assert!(fiber.is_disposed());
        {
            let state = lock(&inner.state);
            assert!(state.records["srv"].fiber.is_none());
            assert!(state.records["srv"].failure.is_some());
        }
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn failed_startup_waits_for_probe_cleanup_before_load_failure() {
        let kernel = Kernel::start();
        let root = kernel.root();
        let fiber = root.plugin::<IdlePlugin>(json!({})).unwrap();
        fiber.settled().await.unwrap();
        let inner = test_inner(root);
        let entry = McpManagedServerEntry::stdio("echo");
        {
            let mut state = lock(&inner.state);
            state.configured.insert("srv".into(), entry.clone());
            state.records.insert(
                "srv".into(),
                ManagedRecord {
                    entry: entry.clone(),
                    generation: 7,
                    changing: false,
                    fiber: Some(fiber.clone()),
                    listener: None,
                    failure: None,
                },
            );
        }
        let (done, signal) = add_gated_probe(&inner, "srv", 8);

        let finishing = tokio::spawn({
            let inner = inner.clone();
            async move {
                finish_mount(&inner, "srv".into(), entry, 7, fiber, Err("boom".into())).await;
            }
        });
        wait_until_aborted(&signal).await;
        assert!(!finishing.is_finished());
        assert!(!matches!(
            lock(&inner.state).statuses.get("srv"),
            Some(McpManagedServerStatus::LoadFailed { .. })
        ));

        done.send_replace(true);
        finishing.await.unwrap();
        assert_eq!(
            lock(&inner.state).statuses.get("srv"),
            Some(&McpManagedServerStatus::LoadFailed {
                last_error: "boom".into()
            })
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn probe_cleanup_failure_takes_precedence_over_startup_failure() {
        let kernel = Kernel::start();
        let root = kernel.root();
        let fiber = root.plugin::<IdlePlugin>(json!({})).unwrap();
        fiber.settled().await.unwrap();
        let inner = test_inner(root);
        let entry = McpManagedServerEntry::stdio("echo");
        {
            let mut state = lock(&inner.state);
            state.configured.insert("srv".into(), entry.clone());
            state.records.insert(
                "srv".into(),
                ManagedRecord {
                    entry: entry.clone(),
                    generation: 7,
                    changing: false,
                    fiber: Some(fiber.clone()),
                    listener: None,
                    failure: None,
                },
            );
        }
        let (done, signal) = add_gated_probe(&inner, "srv", 8);
        let finishing = tokio::spawn({
            let inner = inner.clone();
            async move {
                finish_mount(&inner, "srv".into(), entry, 7, fiber, Err("boom".into())).await;
            }
        });
        wait_until_aborted(&signal).await;
        record_probe_failure(
            &inner,
            "srv",
            McpManagedServerEntry::stdio("echo"),
            McpCloseResult {
                close_confirmed: false,
                cleanup_error: Some("probe cleanup failed".into()),
            },
        );
        done.send_replace(true);
        finishing.await.unwrap();

        assert!(matches!(
            lock(&inner.state).statuses.get("srv"),
            Some(McpManagedServerStatus::CleanupFailed { result })
                if !result.close_confirmed
                    && result.cleanup_error.as_deref() == Some("probe cleanup failed")
        ));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn stale_mount_completion_waits_for_probe_retirement() {
        let kernel = Kernel::start();
        let root = kernel.root();
        let fiber = root.plugin::<IdlePlugin>(json!({})).unwrap();
        fiber.settled().await.unwrap();
        let inner = test_inner(root);
        let old_entry = McpManagedServerEntry::stdio("old");
        let new_entry = McpManagedServerEntry::stdio("new");
        {
            let mut state = lock(&inner.state);
            state.configured.insert("srv".into(), new_entry);
            state.records.insert(
                "srv".into(),
                ManagedRecord {
                    entry: old_entry.clone(),
                    generation: 7,
                    changing: true,
                    fiber: Some(fiber.clone()),
                    listener: None,
                    failure: None,
                },
            );
        }
        let (done, signal) = add_gated_probe(&inner, "srv", 8);
        let finishing = tokio::spawn({
            let inner = inner.clone();
            async move {
                finish_mount(&inner, "srv".into(), old_entry, 7, fiber, Ok(())).await;
            }
        });

        wait_until_aborted(&signal).await;
        assert!(!finishing.is_finished());
        done.send_replace(true);
        finishing.await.unwrap();
        assert!(lock(&inner.state).records.is_empty());
        kernel.shutdown().await;
    }
}

fn client_config(name: &str, entry: &McpManagedServerEntry) -> ClientConfig {
    let tool_call_timeout_ms = entry
        .tool_call_timeout_ms
        .unwrap_or(DEFAULT_TOOL_CALL_TIMEOUT_MS);
    let close_timeout_ms = entry.close_timeout_ms.unwrap_or(DEFAULT_CLOSE_TIMEOUT_MS);
    match entry.transport {
        McpTransport::Stdio => ClientConfig::Stdio {
            server_name: name.to_owned(),
            command: entry.command.clone(),
            args: entry.args.clone(),
            env: entry.env.clone(),
            cwd: entry.cwd.clone(),
            tool_call_timeout_ms,
            startup_timeout_ms: Some(entry.startup_timeout_ms),
            close_timeout_ms,
            expand_templates: true,
            fail_on_startup_error: false,
            max_instruction_bytes: None,
            reconnect: entry.reconnect,
        },
        McpTransport::StreamableHttp => ClientConfig::StreamableHttp {
            server_name: name.to_owned(),
            url: entry.url.clone(),
            headers: entry.headers.clone(),
            tool_call_timeout_ms,
            startup_timeout_ms: Some(entry.startup_timeout_ms),
            close_timeout_ms,
            expand_templates: true,
            fail_on_startup_error: false,
            max_instruction_bytes: None,
            reconnect: entry.reconnect,
        },
    }
}

struct CredentialService(Service<Credentials>);
impl CredentialProvider for CredentialService {
    fn resolve<'a>(
        &'a self,
        reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, Option<ResolvedCredential>> {
        self.0.resolve(reference)
    }
    fn describe<'a>(
        &'a self,
        reference: &'a CredentialRef,
    ) -> CredentialFuture<'a, CredentialInfo> {
        self.0.describe(reference)
    }
    fn set<'a>(&'a self, reference: &'a CredentialRef, value: &'a str) -> CredentialFuture<'a, ()> {
        self.0.set(reference, value)
    }
    fn unset<'a>(&'a self, reference: &'a CredentialRef) -> CredentialFuture<'a, ()> {
        self.0.unset(reference)
    }
    fn read_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        self.0.read_record(key)
    }
    fn describe_record<'a>(
        &'a self,
        key: &'a CredentialKey,
    ) -> CredentialFuture<'a, CredentialRecordInfo> {
        self.0.describe_record(key)
    }
    fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>> {
        self.0.list_records()
    }
    fn modify_record<'a>(
        &'a self,
        key: &'a CredentialKey,
        mutate: CredentialMutator,
    ) -> CredentialFuture<'a, Option<CredentialRecord>> {
        self.0.modify_record(key, mutate)
    }
    fn delete_record<'a>(&'a self, key: &'a CredentialKey) -> CredentialFuture<'a, ()> {
        self.0.delete_record(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_kernel::Kernel;

    fn fixture() -> (Kernel, Arc<Inner>) {
        let kernel = Kernel::start();
        let (commands, _receiver) = mpsc::unbounded_channel();
        let (changed, _) = watch::channel(Vec::new());
        let entry = McpManagedServerEntry::stdio("echo");
        let mut configured = IndexMap::new();
        configured.insert("srv".into(), entry.clone());
        let mut records = IndexMap::new();
        records.insert(
            "srv".into(),
            ManagedRecord {
                entry,
                generation: 7,
                changing: false,
                fiber: None,
                listener: None,
                failure: None,
            },
        );
        let inner = Arc::new(Inner {
            owner: kernel.root(),
            state: Mutex::new(State {
                stopped: false,
                configured,
                registrations: IndexMap::new(),
                records,
                statuses: IndexMap::new(),
                probes: HashMap::new(),
                next_id: 8,
            }),
            changed: Mutex::new(Some(changed)),
            commands,
            lifetime: AbortController::new(),
        });
        (kernel, inner)
    }

    #[tokio::test]
    async fn probe_cleanup_failure_cannot_be_overwritten_by_a_late_status() {
        let (kernel, inner) = fixture();
        record_probe_failure(
            &inner,
            "srv",
            McpManagedServerEntry::stdio("echo"),
            McpCloseResult {
                close_confirmed: false,
                cleanup_error: Some("late close".into()),
            },
        );
        set_status(
            &inner,
            "srv",
            Some(7),
            McpManagedServerStatus::Connection(McpConnectionStatus::Connected {
                tool_count: 1,
                connected_at: 1,
            }),
        );
        assert!(matches!(
            lock(&inner.state).statuses.get("srv"),
            Some(McpManagedServerStatus::CleanupFailed { .. })
        ));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn probe_failure_keeps_live_listener_for_later_closure_failure() {
        let (kernel, inner) = fixture();
        let cell = hoh_mcp_client::McpStatusCell::new(|_| {});
        let callback_inner = inner.clone();
        let callback_cell = cell.clone();
        let listener = cell.on_change(Arc::new(move |status| {
            record_unclosed_generation(&callback_inner, "srv", 7, callback_cell.closure(), status);
        }));
        lock(&inner.state).records.get_mut("srv").unwrap().listener = Some(listener);

        record_probe_failure(
            &inner,
            "srv",
            McpManagedServerEntry::stdio("echo"),
            McpCloseResult {
                close_confirmed: true,
                cleanup_error: Some("first probe diagnostic".into()),
            },
        );
        cell.record_closure(McpCloseResult {
            close_confirmed: false,
            cleanup_error: Some("later live diagnostic".into()),
        });
        cell.update(McpConnectionStatus::Failed {
            reason: McpFailureReason::UnclosedGeneration,
            last_error: Some("live close failed".into()),
        });

        assert!(matches!(
            lock(&inner.state).statuses.get("srv"),
            Some(McpManagedServerStatus::CleanupFailed { result })
                if !result.close_confirmed
                    && result.cleanup_error.as_deref() == Some("first probe diagnostic")
        ));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn probe_failure_before_activation_keeps_the_child_record_active() {
        let (kernel, inner) = fixture();
        lock(&inner.state).records.get_mut("srv").unwrap().failure = Some(McpCloseResult {
            close_confirmed: true,
            cleanup_error: Some("probe cleanup failed".into()),
        });
        let cell = hoh_mcp_client::McpStatusCell::new(|_| {});
        let listener = cell.on_change(Arc::new(|_| {}));
        let entry = lock(&inner.state).configured["srv"].clone();

        assert!(attach_status_listener(&inner, "srv", &entry, 7, listener));
        assert!(lock(&inner.state).records["srv"].listener.is_some());
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn retired_record_cannot_revive_after_a_to_b_to_a_change() {
        let (kernel, inner) = fixture();
        lock(&inner.state).records.get_mut("srv").unwrap().changing = true;
        let entry = lock(&inner.state).configured["srv"].clone();

        assert_eq!(
            mount_disposition(&lock(&inner.state), "srv", &entry, 7),
            MountDisposition::RetiredOwned
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn unclosed_generation_keeps_its_public_reason_and_blocks_replacement() {
        let (kernel, inner) = fixture();
        let connection = McpConnectionStatus::Failed {
            reason: McpFailureReason::UnclosedGeneration,
            last_error: Some("cleanup".into()),
        };
        record_unclosed_generation(
            &inner,
            "srv",
            7,
            McpCloseResult {
                close_confirmed: false,
                cleanup_error: Some("cleanup".into()),
            },
            connection.clone(),
        );
        assert_eq!(
            lock(&inner.state).statuses.get("srv"),
            Some(&McpManagedServerStatus::Connection(connection))
        );
        assert!(lock(&inner.state).records["srv"].failure.is_some());
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn unmount_waits_for_probe_completion_after_abort() {
        let (kernel, inner) = fixture();
        let (done, waiting) = watch::channel(false);
        let key = ("srv".to_owned(), 9);
        lock(&inner.state).probes.insert(
            key.clone(),
            ProbeWork {
                controller: Arc::new(AbortController::new()),
                done: waiting,
            },
        );
        let completion = ProbeCompletion {
            inner: Arc::downgrade(&inner),
            key,
            done,
        };
        let waiting_inner = inner.clone();
        let task = tokio::spawn(async move { wait_probes(&waiting_inner, "srv").await });
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        drop(completion);
        task.await.unwrap();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn config_removal_waits_for_a_probe_only_record() {
        let (kernel, inner) = fixture();
        let controller = Arc::new(AbortController::new());
        let signal = controller.signal();
        let (done, waiting) = watch::channel(false);
        lock(&inner.state).probes.insert(
            ("srv".into(), 9),
            ProbeWork {
                controller,
                done: waiting,
            },
        );
        replace_config(&inner, IndexMap::new());

        let reconciling = tokio::spawn({
            let inner = inner.clone();
            async move { reconcile(&inner).await }
        });
        tokio::task::yield_now().await;
        assert!(signal.aborted());
        assert!(!reconciling.is_finished());

        done.send_replace(true);
        reconciling.await.unwrap().unwrap();
        assert!(lock(&inner.state).records.is_empty());
        assert!(views(&lock(&inner.state)).is_empty());
        kernel.shutdown().await;
    }

    #[test]
    fn cleanup_composition_keeps_the_first_error_and_any_unconfirmed_close() {
        for first_confirmed in [false, true] {
            let first = McpCloseResult {
                close_confirmed: first_confirmed,
                cleanup_error: Some("first cleanup failure".into()),
            };
            let merged = merge_close(
                Some(first),
                McpCloseResult {
                    close_confirmed: !first_confirmed,
                    cleanup_error: Some("second cleanup failure".into()),
                },
            );
            assert!(!merged.close_confirmed);
            assert_eq!(
                merged.cleanup_error.as_deref(),
                Some("first cleanup failure")
            );
        }
    }

    #[tokio::test]
    async fn tombstone_rejects_a_probe_during_cleanup() {
        let (kernel, inner) = fixture();
        lock(&inner.state).records.get_mut("srv").unwrap().changing = true;
        let manager = McpServerManager { inner };
        assert_eq!(
            manager.probe("srv", McpProbeOptions::default()).await,
            Err(McpManagerError::Changing("srv".into()))
        );
        kernel.shutdown().await;
    }
}
