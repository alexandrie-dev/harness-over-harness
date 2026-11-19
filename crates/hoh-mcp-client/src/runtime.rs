use crate::{
    Config,
    connection::{ResolvedReconnectPolicy, reconnect_delay, resolve_reconnect_policy},
    credentials::McpTemplateBinding,
    diagnostic::{
        McpLocalError, McpLocalErrorCode, McpToolError, McpToolErrorCode, describe_error,
        disconnected_message,
    },
    generation::{GenerationConnector, McpGeneration},
    status::{McpConnectionStatusHandle, McpStatusCell},
    tools::{McpRedact, definitions_from_generation, register_definitions},
    types::{McpConnectionStatus, McpFailureReason},
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Fiber, Inject, Plugin, Service, Undo};
use hoh_mcp_resources::{McpResourceFuture, McpResourceProvider, McpResourceRequest, McpResources};
use hoh_scope::{WeakScopeKey, scope_of};
use hoh_system_prompt::{
    PromptSection, PromptSectionOrderName, PromptText, SystemPrompt, SystemPromptKey,
};
use hoh_tools::{ToolExecution, Tools};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn statuses() -> &'static Mutex<HashMap<u32, McpStatusCell>> {
    static VALUE: OnceLock<Mutex<HashMap<u32, McpStatusCell>>> = OnceLock::new();
    VALUE.get_or_init(Default::default)
}
fn reservations() -> &'static Mutex<HashMap<Option<WeakScopeKey>, HashSet<String>>> {
    static VALUE: OnceLock<Mutex<HashMap<Option<WeakScopeKey>, HashSet<String>>>> = OnceLock::new();
    VALUE.get_or_init(Default::default)
}

pub fn connection_status_of(fiber: &Fiber) -> Option<McpStatusCell> {
    lock(statuses()).get(&fiber.uid()).cloned()
}

#[derive(Inject)]
pub struct Deps {
    pub tools: Service<Tools>,
}
#[derive(Inject)]
struct ResourceDeps {
    resources: Service<McpResources>,
}
#[derive(Inject)]
struct PromptDeps {
    system_prompt: Service<SystemPromptKey>,
}
pub struct McpClient;

impl Plugin for McpClient {
    const NAME: &'static str = "mcp-client";
    type Config = Config;
    type Deps = Deps;
    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = (*config.get()).clone();
        config
            .validate()
            .map_err(|error| -> BoxError { error.into() })?;
        if !crate::valid_server_name(config.server_name()) {
            return Err(format!(
                "mcp-client: serverName \"{}\" must match [A-Za-z0-9_-]{{1,32}}",
                config.server_name()
            )
            .into());
        }
        let policy = resolve_reconnect_policy(
            config.reconnect(),
            &format!("mcp-client({}): reconnect", config.server_name()),
        )?;
        reserve_name(&ctx, config.server_name())?;
        let status = {
            let mut values = lock(statuses());
            values
                .entry(ctx.fiber().uid())
                .or_insert_with(|| McpStatusCell::new(|_| {}))
                .clone()
        };
        let warning_logger = ctx.logger();
        status.set_warning_sink(move |message| warning_logger.warn(message));
        let status_uid = ctx.fiber().uid();
        let detached_status = status.clone();
        ctx.effect("mcp-client.status", move || {
            Undo::sync(move || {
                detached_status.detach();
                lock(statuses()).remove(&status_uid);
            })
        })?;
        if !status.closure().close_confirmed || status.closure().cleanup_error.is_some() {
            return Err(format!(
                "mcp-client({}): previous connection cleanup is incomplete; restart the Host",
                config.server_name()
            )
            .into());
        }
        let binding = Arc::new(McpTemplateBinding::from_service(
            ctx.get::<hoh_credentials::Credentials>(),
            config.clone(),
        )?);
        let (credential_updates, credential_changes) = watch::channel(0_u64);
        if !binding.refs().is_empty() {
            let refs = binding.refs().clone();
            let credential_updates = credential_updates.clone();
            ctx.on_contained::<hoh_credentials::CredentialsReferenceUpdated>(move |event| {
                if refs.contains(event.reference.as_str()) {
                    credential_updates.send_modify(|revision| {
                        *revision = revision.wrapping_add(1);
                    });
                }
                Ok(())
            })?;
        }
        let live = Arc::new(Mutex::new(None::<McpGeneration>));
        let instructions = Arc::new(Mutex::new(String::new()));
        register_optional_context(
            &ctx,
            config.server_name(),
            live.clone(),
            instructions.clone(),
            config.tool_call_timeout_ms(),
            redactor(&binding, config.expand_templates()),
        )
        .await?;
        let (ready_tx, ready_rx) = oneshot::channel();
        let task_ctx = ctx.clone();
        let task_status = status.clone();
        let task_config = config.clone();
        ctx.spawn("mcp-client.connection", move |token| {
            supervisor(
                task_ctx,
                deps.tools,
                task_config,
                policy,
                task_status,
                binding,
                live,
                instructions,
                GenerationConnector::default(),
                token,
                ready_tx,
                credential_changes,
                credential_updates,
            )
        })?;
        let initial = ready_rx.await.unwrap_or_else(|_| {
            Err("connection supervisor stopped before startup settled".to_owned())
        });
        if config.fail_on_startup_error()
            && let Err(message) = initial
        {
            return Err(format!(
                "mcp-client({}): initial connection or tool synchronization failed: {message}",
                config.server_name()
            )
            .into());
        }
        Ok(())
    }
}

fn reserve_name(ctx: &Context, server: &str) -> Result<(), BoxError> {
    let key = scope_of(ctx).map(|key| key.downgrade());
    {
        let mut all = lock(reservations());
        let names = all.entry(key.clone()).or_default();
        if !names.insert(server.to_owned()) {
            return Err(format!("mcp-client: serverName \"{server}\" is already in use by another mcp-client instance — pick a unique serverName in cordis.yml").into());
        }
    }
    let server = server.to_owned();
    ctx.effect("mcp-client.serverName", move || {
        Undo::sync(move || {
            let mut all = lock(reservations());
            if let Some(names) = all.get_mut(&key) {
                names.remove(&server);
                if names.is_empty() {
                    all.remove(&key);
                }
            }
        })
    })?;
    Ok(())
}

async fn register_optional_context(
    ctx: &Context,
    server: &str,
    live: Arc<Mutex<Option<McpGeneration>>>,
    instructions: Arc<Mutex<String>>,
    timeout_ms: u64,
    redact: Option<McpRedact>,
) -> Result<(), BoxError> {
    let resource_server = server.to_owned();
    let resource_live = live.clone();
    let resource_redact = redact.clone();
    let resources = ctx.inject::<ResourceDeps, _>(move |child, deps| {
        let server = resource_server.clone();
        let live = resource_live.clone();
        let redact = resource_redact.clone();
        async move {
            deps.resources.register(
                &child,
                server.clone(),
                Arc::new(LiveResourceProvider {
                    live,
                    timeout_ms,
                    redact,
                    server,
                }),
            )?;
            Ok(())
        }
    })?;
    let prompt_server = server.to_owned();
    let prompt_instructions = instructions.clone();
    let prompt = ctx.inject::<PromptDeps, _>(move |child, deps| {
        let server = prompt_server.clone();
        let instructions = prompt_instructions.clone();
        async move {
            deps.system_prompt.section(
                &child,
                PromptSection {
                    name: format!("mcp:{server}"),
                    order: SystemPrompt::get_section_order(PromptSectionOrderName::McpServers),
                    interpolate: Some(false),
                    text: PromptText::provider(move |_| Ok(lock(&instructions).clone())),
                    complete: false,
                },
            )?;
            Ok(())
        }
    })?;
    if ctx.has("mcpResources") {
        resources.settled().await?;
    }
    if ctx.has("systemPrompt") {
        prompt.settled().await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn supervisor(
    ctx: Context,
    tools: Service<Tools>,
    config: Config,
    policy: ResolvedReconnectPolicy,
    status: McpStatusCell,
    binding: Arc<McpTemplateBinding>,
    live: Arc<Mutex<Option<McpGeneration>>>,
    instructions: Arc<Mutex<String>>,
    connector: GenerationConnector,
    token: CancellationToken,
    ready: oneshot::Sender<Result<(), String>>,
    mut credential_changes: watch::Receiver<u64>,
    _credential_updates: watch::Sender<u64>,
) -> Result<(), BoxError> {
    let mut ready = Some(ready);
    let mut failed = 0_u64;
    let mut disposers = Vec::new();
    let mut resolved_config = None::<Config>;
    let mut last_error: Option<String>;
    loop {
        if token.is_cancelled() {
            break;
        }
        status.update(McpConnectionStatus::Connecting { attempt: failed });
        let attempt = connect_once(
            &ctx,
            &tools,
            &config,
            &status,
            &binding,
            &connector,
            &token,
            &mut disposers,
            &mut resolved_config,
        )
        .await;
        match attempt {
            Ok((generation, prompt)) => {
                *lock(&live) = Some(generation.clone());
                *lock(&instructions) = prompt;
                let connected_at = now_ms();
                status.update(McpConnectionStatus::Connected {
                    tool_count: disposers.len(),
                    connected_at,
                });
                if let Some(sender) = ready.take() {
                    let _ = sender.send(Ok(()));
                }
                let connected = Instant::now();
                let mut resync_error = None::<String>;
                let mut credential_refresh = false;
                loop {
                    tokio::select! {
                        _ = token.cancelled() => break,
                        changed = credential_changes.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            match resolve_refresh(&binding, &token).await {
                                Ok(next) if !binding_config_changed(resolved_config.as_ref(), next.as_ref()) => continue,
                                Ok(next) => {
                                    resolved_config = next;
                                    credential_refresh = true;
                                    break;
                                }
                                Err(error) => {
                                    ctx.logger().error(format_args!(
                                        "mcp-client({}): credential refresh failed: {}",
                                        config.server_name(),
                                        binding.redact(&describe_error(error.as_ref()))
                                    ));
                                    continue;
                                }
                            }
                        }
                        changed = generation.wait_tool_change() => {
                            if !changed {
                                break;
                            }
                            if generation.subscription_ended() {
                                resync_error = Some("MCP tool-list subscription ended".to_owned());
                                break;
                            }
                            ctx.logger().info(format_args!(
                                "mcp-client({}): tool list changed, re-syncing",
                                config.server_name()
                            ));
                            let refresh = definitions_from_generation(
                                &ctx,
                                &generation,
                                config.server_name(),
                                config.tool_call_timeout_ms(),
                                redactor(&binding, config.expand_templates()),
                            );
                            let definitions = tokio::select! {
                                _ = token.cancelled() => break,
                                result = tokio::time::timeout(
                                    Duration::from_millis(config.tool_call_timeout_ms().max(1)),
                                    refresh,
                                ) => match result {
                                    Ok(result) => result,
                                    Err(_) => continue,
                                },
                            };
                            let definitions = match definitions {
                                Ok(definitions) => definitions,
                                Err(error) => {
                                    ctx.logger().error(format_args!(
                                        "mcp-client({}): tool re-sync failed: {}",
                                        config.server_name(),
                                        binding.redact(&describe_error(error.as_ref()))
                                    ));
                                    continue;
                                }
                            };
                            for disposer in std::mem::take(&mut disposers) {
                                disposer.dispose().await;
                            }
                            match register_definitions(&ctx, &tools, definitions).await {
                                Ok(next) => {
                                    disposers = next;
                                    status.update(McpConnectionStatus::Connected {
                                        tool_count: disposers.len(),
                                        connected_at,
                                    });
                                }
                                Err(error) => {
                                    let message = binding.redact(&describe_error(error.as_ref()));
                                    ctx.logger().error(format_args!(
                                        "mcp-client({}): tool re-sync failed: {message}",
                                        config.server_name()
                                    ));
                                    resync_error = Some(message);
                                    break;
                                }
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(50)) => if generation.is_closed().await { break; }
                    }
                }
                *lock(&live) = None;
                *lock(&instructions) = String::new();
                let close = generation.close().await;
                status.record_closure(close.clone());
                if token.is_cancelled() {
                    break;
                }
                if !close.close_confirmed || close.cleanup_error.is_some() {
                    last_error = close.cleanup_error.clone();
                    ctx.logger().error(format_args!(
                        "mcp-client({}): cleanup incomplete",
                        config.server_name()
                    ));
                    status.update(McpConnectionStatus::Failed {
                        reason: McpFailureReason::UnclosedGeneration,
                        last_error: last_error.clone(),
                    });
                    break;
                }
                if credential_refresh {
                    if resolved_config.is_none() {
                        for disposer in std::mem::take(&mut disposers) {
                            disposer.dispose().await;
                        }
                    }
                    failed = 0;
                    continue;
                } else if let Some(message) = resync_error {
                    last_error = Some(message);
                    failed = failed_after_loss(
                        failed,
                        connected.elapsed(),
                        Duration::from_millis(policy.max_delay_ms),
                    );
                } else {
                    last_error = Some("connection closed".to_owned());
                    failed = failed_after_loss(
                        failed,
                        connected.elapsed(),
                        Duration::from_millis(policy.max_delay_ms),
                    );
                }
            }
            Err(error) => {
                let message = binding.redact(&describe_error(error.as_ref()));
                last_error = Some(message.clone());
                if let Some(sender) = ready.take() {
                    let _ = sender.send(Err(message.clone()));
                }
                if hoh_error::HarnessError::of(error.as_ref())
                    .is_some_and(|error| error.code() == "MCP_UNCLOSED_GENERATION")
                {
                    status.record_closure(crate::types::McpCloseResult {
                        close_confirmed: false,
                        cleanup_error: Some(message.clone()),
                    });
                    status.update(McpConnectionStatus::Failed {
                        reason: McpFailureReason::UnclosedGeneration,
                        last_error: Some(message),
                    });
                    ctx.logger().error(format_args!(
                        "mcp-client({}): cleanup incomplete",
                        config.server_name()
                    ));
                    break;
                }
                if hoh_error::HarnessError::of(error.as_ref())
                    .is_some_and(|error| error.code() == "MCP_CREDENTIAL_NOT_FOUND")
                {
                    resolved_config = None;
                    status.update(McpConnectionStatus::WaitingCredentials {
                        last_error: message,
                    });
                    if wait_for_changed_credentials(
                        &ctx,
                        &binding,
                        &token,
                        &mut credential_changes,
                        &mut resolved_config,
                    )
                    .await
                    {
                        failed = 0;
                        continue;
                    }
                    break;
                }
                failed += 1;
            }
        }
        if !policy.enabled {
            status.update(McpConnectionStatus::Failed {
                reason: McpFailureReason::ReconnectDisabled,
                last_error: last_error.clone(),
            });
            if wait_for_changed_credentials(
                &ctx,
                &binding,
                &token,
                &mut credential_changes,
                &mut resolved_config,
            )
            .await
            {
                failed = 0;
                continue;
            }
            break;
        }
        if failed > policy.max_attempts {
            for disposer in std::mem::take(&mut disposers) {
                disposer.dispose().await;
            }
            status.update(McpConnectionStatus::Failed {
                reason: McpFailureReason::GaveUp,
                last_error: last_error.clone(),
            });
            ctx.logger().error(format_args!(
                "mcp-client({}): giving up after {} consecutive failed reconnect attempts",
                config.server_name(),
                policy.max_attempts
            ));
            if wait_for_changed_credentials(
                &ctx,
                &binding,
                &token,
                &mut credential_changes,
                &mut resolved_config,
            )
            .await
            {
                failed = 0;
                continue;
            }
            break;
        }
        let delay = reconnect_delay(policy, failed);
        status.update(McpConnectionStatus::Reconnecting {
            attempt: failed,
            max_attempts: policy.max_attempts,
            next_retry_at_ms: now_ms().saturating_add(delay),
            last_error: last_error
                .clone()
                .unwrap_or_else(|| "connection unavailable".to_owned()),
        });
        ctx.logger().warn(format_args!(
            "mcp-client({}): retrying in {delay}ms (attempt {failed}/{})",
            config.server_name(),
            policy.max_attempts
        ));
        let sleep = tokio::time::sleep(Duration::from_millis(delay));
        tokio::pin!(sleep);
        let mut binding_changed = false;
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                changed = credential_changes.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    match resolve_refresh(&binding, &token).await {
                        Ok(next) if !binding_config_changed(resolved_config.as_ref(), next.as_ref()) => continue,
                        Ok(next) => {
                            resolved_config = next;
                            binding_changed = true;
                            break;
                        }
                        Err(error) => ctx.logger().error(format_args!(
                            "mcp-client({}): credential refresh failed: {}",
                            config.server_name(),
                            binding.redact(&describe_error(error.as_ref()))
                        )),
                    }
                }
                _ = &mut sleep => break,
            }
        }
        if token.is_cancelled() {
            break;
        }
        if binding_changed {
            failed = 0;
            continue;
        }
    }
    *lock(&live) = None;
    *lock(&instructions) = String::new();
    if token.is_cancelled() {
        for disposer in std::mem::take(&mut disposers) {
            disposer.dispose().await;
        }
        let close = status.closure();
        status.update(McpConnectionStatus::Closed { result: close });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn connect_once(
    ctx: &Context,
    tools: &Service<Tools>,
    config: &Config,
    status: &McpStatusCell,
    binding: &Arc<McpTemplateBinding>,
    connector: &GenerationConnector,
    token: &CancellationToken,
    previous: &mut Vec<hoh_kernel::Disposer>,
    resolved_config: &mut Option<Config>,
) -> Result<(McpGeneration, String), BoxError> {
    let started = Instant::now();
    let timeout_ms = config.startup_timeout_ms();
    let controller = hoh_timeout::AbortController::new();
    let resolved = startup_step(
        token,
        timeout_ms,
        started,
        config.server_name(),
        binding.resolve(&controller.signal()),
    )
    .await??;
    *resolved_config = Some(resolved.clone());
    let remaining_ms = timeout_ms.map(|total| {
        total
            .saturating_sub(started.elapsed().as_millis() as u64)
            .max(1)
    });
    let connect_signal = hoh_timeout::AbortSignal::from_token(token.clone());
    let generation = connector
        .connect(&resolved, &connect_signal, remaining_ms)
        .await?;
    status.opened();
    let setup = async {
        let instructions = generation.instructions().await.unwrap_or_default();
        let prompt = crate::server_context::instruction_text(
            resolved.server_name(),
            &instructions,
            resolved.max_instruction_bytes(),
        )?;
        let definitions = definitions_from_generation(
            ctx,
            &generation,
            resolved.server_name(),
            resolved.tool_call_timeout_ms(),
            redactor(binding, resolved.expand_templates()),
        )
        .await?;
        Ok::<_, BoxError>((definitions, prompt))
    };
    let setup = startup_step(token, timeout_ms, started, config.server_name(), setup).await;
    let (definitions, prompt) = match setup {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            let close = generation.close().await;
            status.record_closure(close.clone());
            return Err(prefer_close_failure(error, close, config.server_name()));
        }
        Err(error) => {
            let close = generation.close().await;
            status.record_closure(close.clone());
            return Err(prefer_close_failure(error, close, config.server_name()));
        }
    };
    if token.is_cancelled() || generation.is_closed().await {
        let close = generation.close().await;
        status.record_closure(close.clone());
        let error = Box::new(McpLocalError::new(
            format!(
                "mcp-client({}): startup was abandoned",
                config.server_name()
            ),
            McpLocalErrorCode::ConnectionClosed,
        ));
        return Err(prefer_close_failure(error, close, config.server_name()));
    }
    for disposer in std::mem::take(previous) {
        disposer.dispose().await;
    }
    *previous = match register_definitions(ctx, tools, definitions).await {
        Ok(disposers) => disposers,
        Err(error) => {
            let close = generation.close().await;
            status.record_closure(close.clone());
            return Err(prefer_close_failure(error, close, config.server_name()));
        }
    };
    Ok((generation, prompt))
}

fn prefer_close_failure(
    original: BoxError,
    close: crate::types::McpCloseResult,
    server: &str,
) -> BoxError {
    if close.close_confirmed && close.cleanup_error.is_none() {
        return original;
    }
    Box::new(McpLocalError::with_cause(
        format!(
            "mcp-client({server}): startup cleanup is incomplete: {}",
            close
                .cleanup_error
                .as_deref()
                .unwrap_or("transport closure was not confirmed")
        ),
        McpLocalErrorCode::UnclosedGeneration,
        original,
    ))
}

async fn resolve_refresh(
    binding: &McpTemplateBinding,
    token: &CancellationToken,
) -> Result<Option<Config>, BoxError> {
    match binding
        .resolve(&hoh_timeout::AbortSignal::from_token(token.clone()))
        .await
    {
        Ok(config) => Ok(Some(config)),
        Err(error) if is_missing_credential(error.as_ref()) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_missing_credential(error: &(dyn std::error::Error + 'static)) -> bool {
    hoh_error::HarnessError::of(error)
        .is_some_and(|error| error.code() == "MCP_CREDENTIAL_NOT_FOUND")
}

fn binding_config_changed(current: Option<&Config>, next: Option<&Config>) -> bool {
    current != next
}

fn failed_after_loss(current: u64, uptime: Duration, stability_window: Duration) -> u64 {
    if uptime >= stability_window {
        1
    } else {
        current.saturating_add(1)
    }
}

async fn wait_for_changed_credentials(
    ctx: &Context,
    binding: &McpTemplateBinding,
    token: &CancellationToken,
    changes: &mut watch::Receiver<u64>,
    current: &mut Option<Config>,
) -> bool {
    loop {
        tokio::select! {
            _ = token.cancelled() => return false,
            changed = changes.changed() => if changed.is_err() { return false; },
        }
        match resolve_refresh(binding, token).await {
            Ok(next) if !binding_config_changed(current.as_ref(), next.as_ref()) => {}
            Ok(next) => {
                *current = next;
                return true;
            }
            Err(error) => ctx.logger().error(format_args!(
                "credential refresh failed: {}",
                binding.redact(&describe_error(error.as_ref()))
            )),
        }
    }
}

async fn startup_step<T>(
    token: &CancellationToken,
    timeout_ms: Option<u64>,
    started: Instant,
    server_name: &str,
    future: impl std::future::Future<Output = T>,
) -> Result<T, BoxError> {
    let timeout_error = || {
        Box::new(McpLocalError::new(
            format!(
                "mcp-client({server_name}): startup exceeded {}ms",
                timeout_ms.unwrap_or_default()
            ),
            McpLocalErrorCode::Timeout,
        )) as BoxError
    };
    if let Some(total) = timeout_ms {
        let elapsed = started.elapsed().as_millis() as u64;
        if elapsed >= total {
            return Err(timeout_error());
        }
        tokio::select! {
            _ = token.cancelled() => Err(Box::new(McpLocalError::new(
                format!("mcp-client({server_name}): startup cancelled"),
                McpLocalErrorCode::ConnectionClosed,
            ))),
            result = tokio::time::timeout(Duration::from_millis((total - elapsed).max(1)), future) => {
                result.map_err(|_| timeout_error())
            }
        }
    } else {
        tokio::select! {
            _ = token.cancelled() => Err(Box::new(McpLocalError::new(
                format!("mcp-client({server_name}): startup cancelled"),
                McpLocalErrorCode::ConnectionClosed,
            ))),
            result = future => Ok(result),
        }
    }
}

struct LiveResourceProvider {
    live: Arc<Mutex<Option<McpGeneration>>>,
    timeout_ms: u64,
    redact: Option<McpRedact>,
    server: String,
}
impl McpResourceProvider for LiveResourceProvider {
    fn request<'a>(
        &'a self,
        request: McpResourceRequest,
        exec: Arc<ToolExecution>,
    ) -> McpResourceFuture<'a> {
        let generation = lock(&self.live).clone();
        Box::pin(async move {
            let generation = generation.ok_or_else(|| {
                Box::new(McpToolError::new(
                    disconnected_message(&self.server),
                    McpToolErrorCode::ServerUnavailable,
                )) as BoxError
            })?;
            crate::server_context::GenerationResourceProvider::with_policy(
                generation,
                self.timeout_ms,
                self.redact.clone(),
            )
            .request(request, exec)
            .await
        })
    }
}

fn redactor(binding: &Arc<McpTemplateBinding>, enabled: bool) -> Option<McpRedact> {
    enabled.then(|| {
        let binding = binding.clone();
        Arc::new(move |text: &str| binding.redact(text)) as McpRedact
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_credentials::{
        CredentialFuture, CredentialInfo, CredentialKey, CredentialMutator, CredentialProvider,
        CredentialRecord, CredentialRecordEntry, CredentialRecordInfo, CredentialRef,
        ResolvedCredential,
    };
    use hoh_error::BoxError;
    use hoh_kernel::Kernel;
    use hoh_system_prompt::{AssembleContext, SystemPrompt};
    use hoh_tools::{ToolBatchInput, ToolBatchResult, ToolExecutor, ToolModelInput};
    use rmcp::{
        ServerHandler, ServiceExt,
        model::{
            ErrorData, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
            Tool,
        },
        service::{RequestContext, RoleServer},
    };
    use serde_json::json;
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[derive(Clone)]
    struct StaticServer {
        instructions: String,
    }

    impl ServerHandler for StaticServer {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_instructions(self.instructions.clone())
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            let mut result = ListToolsResult::default();
            result.tools.push(Tool::new(
                "echo",
                "fixture",
                serde_json::Map::from_iter([("type".into(), json!("object"))]),
            ));
            Ok(result)
        }
    }

    struct TestCredentials {
        value: Mutex<Option<String>>,
    }

    impl TestCredentials {
        fn new(value: &str) -> Self {
            Self {
                value: Mutex::new(Some(value.to_owned())),
            }
        }

        fn clear(&self) {
            *lock(&self.value) = None;
        }
    }

    impl CredentialProvider for TestCredentials {
        fn resolve<'a>(
            &'a self,
            _reference: &'a CredentialRef,
        ) -> CredentialFuture<'a, Option<ResolvedCredential>> {
            let value = lock(&self.value).clone();
            Box::pin(async move {
                Ok(value.map(|value| ResolvedCredential {
                    value,
                    source: "test".into(),
                }))
            })
        }

        fn describe<'a>(
            &'a self,
            _reference: &'a CredentialRef,
        ) -> CredentialFuture<'a, CredentialInfo> {
            let configured = lock(&self.value).is_some();
            Box::pin(async move {
                Ok(CredentialInfo {
                    configured,
                    source: configured.then(|| "test".into()),
                    writable: true,
                    masked: None,
                })
            })
        }

        fn set<'a>(
            &'a self,
            _reference: &'a CredentialRef,
            value: &'a str,
        ) -> CredentialFuture<'a, ()> {
            *lock(&self.value) = Some(value.to_owned());
            Box::pin(async { Ok(()) })
        }

        fn unset<'a>(&'a self, _reference: &'a CredentialRef) -> CredentialFuture<'a, ()> {
            self.clear();
            Box::pin(async { Ok(()) })
        }

        fn read_record<'a>(
            &'a self,
            _key: &'a CredentialKey,
        ) -> CredentialFuture<'a, Option<CredentialRecord>> {
            Box::pin(async { Ok(None) })
        }

        fn describe_record<'a>(
            &'a self,
            _key: &'a CredentialKey,
        ) -> CredentialFuture<'a, CredentialRecordInfo> {
            Box::pin(async {
                Ok(CredentialRecordInfo {
                    configured: false,
                    kind: None,
                    writable: false,
                })
            })
        }

        fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn modify_record<'a>(
            &'a self,
            _key: &'a CredentialKey,
            _mutate: CredentialMutator,
        ) -> CredentialFuture<'a, Option<CredentialRecord>> {
            Box::pin(async { Ok(None) })
        }

        fn delete_record<'a>(&'a self, _key: &'a CredentialKey) -> CredentialFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    async fn test_generation(
        instructions: &str,
        appears_closed: bool,
        forced_close_result: Option<crate::types::McpCloseResult>,
    ) -> (McpGeneration, tokio::task::JoinHandle<()>) {
        let (server_transport, client_transport) = tokio::io::duplex(16 * 1024);
        let server = StaticServer {
            instructions: instructions.to_owned(),
        };
        let task = tokio::spawn(async move {
            server
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let generation = McpGeneration::from_test_transport(
            client_transport,
            appears_closed,
            forced_close_result,
        )
        .await;
        (generation, task)
    }

    fn scripted_connector(
        generations: Vec<McpGeneration>,
    ) -> (GenerationConnector, Arc<AtomicUsize>) {
        let generations = Arc::new(Mutex::new(VecDeque::from(generations)));
        let attempts = Arc::new(AtomicUsize::new(0));
        let connector_attempts = attempts.clone();
        let connector = GenerationConnector::from_fn(move |_, _, _| {
            connector_attempts.fetch_add(1, Ordering::SeqCst);
            let generation = lock(&generations).pop_front();
            Box::pin(async move {
                generation.ok_or_else(|| {
                    Box::<dyn std::error::Error + Send + Sync>::from(
                        "scripted generation exhausted",
                    )
                })
            })
        });
        (connector, attempts)
    }

    async fn wait_for_status(
        status: &McpStatusCell,
        predicate: impl Fn(&McpConnectionStatus) -> bool,
    ) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if predicate(&status.current()) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn runtime_tools() -> (Kernel, Context, Service<Tools>) {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let tools = ctx.get::<Tools>().unwrap();
        (kernel, ctx, tools)
    }

    fn http_test_config(server_name: &str) -> Config {
        Config::StreamableHttp {
            server_name: server_name.into(),
            url: "https://example.invalid".into(),
            headers: Default::default(),
            tool_call_timeout_ms: 100,
            startup_timeout_ms: Some(1_000),
            close_timeout_ms: 10,
            expand_templates: false,
            fail_on_startup_error: false,
            max_instruction_bytes: None,
            reconnect: None,
        }
    }

    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn launch_supervisor(
        ctx: Context,
        tools: Service<Tools>,
        config: Config,
        status: McpStatusCell,
        binding: Arc<McpTemplateBinding>,
        connector: GenerationConnector,
        credential_changes: watch::Receiver<u64>,
        credential_updates: watch::Sender<u64>,
    ) -> (
        CancellationToken,
        oneshot::Receiver<Result<(), String>>,
        tokio::task::JoinHandle<Result<(), BoxError>>,
    ) {
        let policy = resolve_reconnect_policy(
            config.reconnect(),
            &format!("mcp-client({}): reconnect", config.server_name()),
        )
        .unwrap();
        let token = CancellationToken::new();
        let (ready_tx, ready_rx) = oneshot::channel();
        let task_token = token.clone();
        let task = tokio::spawn(supervisor(
            ctx,
            tools,
            config,
            policy,
            status,
            binding,
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(String::new())),
            connector,
            task_token,
            ready_tx,
            credential_changes,
            credential_updates,
        ));
        (token, ready_rx, task)
    }

    struct EmptyExecutor;
    impl ToolExecutor for EmptyExecutor {
        fn describe(
            &self,
            _tools: &[Arc<hoh_tools::ToolDefinition>],
        ) -> Result<ToolModelInput, BoxError> {
            Ok(ToolModelInput::default())
        }

        fn execute(
            &self,
            _input: ToolBatchInput,
        ) -> hoh_tools::shim::BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
            Box::pin(async { Ok(ToolBatchResult::default()) })
        }
    }

    fn status_test_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    #[test]
    fn failed_startup_prefers_unclosed_generation_over_original_error() {
        let error = prefer_close_failure(
            Box::<dyn std::error::Error + Send + Sync>::from("instructions too large"),
            crate::types::McpCloseResult {
                close_confirmed: false,
                cleanup_error: Some("close timed out".into()),
            },
            "docs",
        );
        let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), "MCP_UNCLOSED_GENERATION");
        assert!(error.message().contains("close timed out"));
    }

    #[tokio::test]
    async fn supervisor_stops_after_setup_failure_with_unconfirmed_close() {
        let _serial = status_test_lock().lock().await;
        let (kernel, ctx, tools) = runtime_tools().await;
        let close = crate::types::McpCloseResult {
            close_confirmed: false,
            cleanup_error: Some("close timed out".into()),
        };
        let (generation, server) = test_generation("too large", false, Some(close)).await;
        let (connector, attempts) = scripted_connector(vec![generation]);
        let mut config = http_test_config("instructions-limit");
        if let Config::StreamableHttp {
            max_instruction_bytes,
            reconnect,
            ..
        } = &mut config
        {
            *max_instruction_bytes = Some(1);
            *reconnect = Some(crate::ReconnectConfig {
                enabled: Some(true),
                initial_delay_ms: Some(1),
                max_delay_ms: Some(1),
                max_attempts: Some(3),
            });
        }
        let binding = Arc::new(McpTemplateBinding::new(None, config.clone()).unwrap());
        let status = McpStatusCell::new(|_| {});
        let (updates, changes) = watch::channel(0);
        let (token, ready, task) = launch_supervisor(
            ctx,
            tools,
            config,
            status.clone(),
            binding,
            connector,
            changes,
            updates,
        );
        assert!(ready.await.unwrap().is_err());
        wait_for_status(&status, |value| {
            matches!(
                value,
                McpConnectionStatus::Failed {
                    reason: McpFailureReason::UnclosedGeneration,
                    ..
                }
            )
        })
        .await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        token.cancel();
        task.await.unwrap().unwrap();
        server.await.unwrap();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn abandoned_setup_with_unconfirmed_close_is_terminal() {
        let _serial = status_test_lock().lock().await;
        let (kernel, ctx, tools) = runtime_tools().await;
        let close = crate::types::McpCloseResult {
            close_confirmed: false,
            cleanup_error: Some("close timed out".into()),
        };
        let (generation, server) = test_generation("ready", true, Some(close)).await;
        let (connector, attempts) = scripted_connector(vec![generation]);
        let mut config = http_test_config("abandoned-setup");
        if let Config::StreamableHttp { reconnect, .. } = &mut config {
            *reconnect = Some(crate::ReconnectConfig {
                enabled: Some(true),
                initial_delay_ms: Some(1),
                max_delay_ms: Some(1),
                max_attempts: Some(3),
            });
        }
        let binding = Arc::new(McpTemplateBinding::new(None, config.clone()).unwrap());
        let status = McpStatusCell::new(|_| {});
        let (updates, changes) = watch::channel(0);
        let (token, ready, task) = launch_supervisor(
            ctx,
            tools,
            config,
            status.clone(),
            binding,
            connector,
            changes,
            updates,
        );
        assert!(ready.await.unwrap().is_err());
        wait_for_status(&status, |value| {
            matches!(
                value,
                McpConnectionStatus::Failed {
                    reason: McpFailureReason::UnclosedGeneration,
                    ..
                }
            )
        })
        .await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        token.cancel();
        task.await.unwrap().unwrap();
        server.await.unwrap();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn credential_events_preserve_same_connection_and_missing_value_disconnects() {
        let _serial = status_test_lock().lock().await;
        let (kernel, ctx, tools) = runtime_tools().await;
        let (generation, server) = test_generation("ready", false, None).await;
        let (connector, attempts) = scripted_connector(vec![generation]);
        let credentials = Arc::new(TestCredentials::new("same-secret"));
        let mut config = http_test_config("credential-refresh");
        if let Config::StreamableHttp {
            headers,
            expand_templates,
            reconnect,
            ..
        } = &mut config
        {
            headers.insert("Authorization".into(), "Bearer ${credential:TOKEN}".into());
            *expand_templates = true;
            *reconnect = Some(crate::ReconnectConfig {
                enabled: Some(true),
                initial_delay_ms: Some(1),
                max_delay_ms: Some(1),
                max_attempts: Some(3),
            });
        }
        let provider: Arc<dyn CredentialProvider> = credentials.clone();
        let binding = Arc::new(McpTemplateBinding::new(Some(provider), config.clone()).unwrap());
        let status = McpStatusCell::new(|_| {});
        let (updates, changes) = watch::channel(0);
        let tools_for_assertion = tools.clone();
        let (token, ready, task) = launch_supervisor(
            ctx,
            tools,
            config,
            status.clone(),
            binding,
            connector,
            changes,
            updates.clone(),
        );
        ready.await.unwrap().unwrap();
        let connected_at = match status.current() {
            McpConnectionStatus::Connected { connected_at, .. } => connected_at,
            other => panic!("expected connected, got {other:?}"),
        };
        assert_eq!(tools_for_assertion.known_names(None).len(), 1);

        updates.send_modify(|revision| *revision += 1);
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(matches!(
            status.current(),
            McpConnectionStatus::Connected {
                connected_at: current,
                ..
            } if current == connected_at
        ));

        credentials.clear();
        updates.send_modify(|revision| *revision += 1);
        wait_for_status(&status, |value| {
            matches!(value, McpConnectionStatus::WaitingCredentials { .. })
        })
        .await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(tools_for_assertion.known_names(None).is_empty());

        token.cancel();
        task.await.unwrap().unwrap();
        server.await.unwrap();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn failed_transport_keeps_the_resolved_binding_for_refresh_equality() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let tools = ctx.get::<Tools>().unwrap();
        let config = Config::Stdio {
            server_name: "missing".into(),
            command: "/definitely/not/a/real/mcp-server".into(),
            args: Vec::new(),
            env: Default::default(),
            cwd: String::new(),
            tool_call_timeout_ms: 10,
            startup_timeout_ms: None,
            close_timeout_ms: 10,
            expand_templates: false,
            fail_on_startup_error: false,
            max_instruction_bytes: None,
            reconnect: None,
        };
        let binding = Arc::new(McpTemplateBinding::new(None, config.clone()).unwrap());
        let mut resolved = None;
        let result = connect_once(
            &ctx,
            &tools,
            &config,
            &McpStatusCell::new(|_| {}),
            &binding,
            &GenerationConnector::default(),
            &CancellationToken::new(),
            &mut Vec::new(),
            &mut resolved,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(resolved, Some(config));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn optional_prompt_and_resources_attach_when_mounted_later() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let instructions = Arc::new(Mutex::new("### MCP server: docs\n\nUse docs".to_owned()));
        register_optional_context(
            &ctx,
            "docs",
            Arc::new(Mutex::new(None)),
            instructions,
            100,
            None,
        )
        .await
        .unwrap();

        ctx.plugin::<SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.get::<Tools>()
            .unwrap()
            .set_executor(&ctx, Arc::new(EmptyExecutor))
            .unwrap();
        ctx.plugin::<hoh_mcp_resources::McpResourceRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();

        let prompt = ctx.get::<SystemPromptKey>().unwrap();
        let mut sections = Vec::new();
        for _ in 0..20 {
            sections = prompt
                .assemble(AssembleContext::default())
                .await
                .unwrap()
                .sections;
            if sections.iter().any(|section| section.name == "mcp:docs")
                && sections.iter().any(|section| section.text.contains("docs"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            sections
                .iter()
                .any(|section| section.name == "mcp:docs" && section.text.contains("Use docs")),
            "{sections:?}"
        );
        assert!(
            sections.iter().any(|section| {
                section.name == "mcp-resource-servers" && section.text.contains("docs")
            }),
            "{sections:?}"
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn supervisor_retries_logs_gives_up_and_detaches_status_on_unload() {
        let _serial = status_test_lock().lock().await;
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let fiber = ctx
            .plugin::<McpClient>(json!({
                "transport":"stdio",
                "serverName":"missing",
                "command":"/definitely/not/a/real/mcp-server",
                "reconnect":{"initialDelayMs":1,"maxDelayMs":1,"maxAttempts":1}
            }))
            .unwrap();
        fiber.settled().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if connection_status_of(&fiber).is_some_and(|status| {
                    matches!(
                        status.current(),
                        McpConnectionStatus::Failed {
                            reason: McpFailureReason::GaveUp,
                            ..
                        }
                    )
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let logs = ctx
            .logger()
            .buffer()
            .into_iter()
            .map(|record| record.message)
            .collect::<Vec<_>>();
        assert!(
            logs.iter().any(|message| message.contains("retrying")),
            "{logs:?}"
        );
        assert!(
            logs.iter().any(|message| message.contains("giving up")),
            "{logs:?}"
        );
        fiber.dispose().await;
        assert!(connection_status_of(&fiber).is_none());
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn fail_on_startup_error_rejects_activation() {
        let _serial = status_test_lock().lock().await;
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let fiber = ctx
            .plugin::<McpClient>(json!({
                "transport":"stdio",
                "serverName":"strict",
                "command":"/definitely/not/a/real/mcp-server",
                "failOnStartupError":true,
                "reconnect":{"enabled":false}
            }))
            .unwrap();
        let error = fiber.settled().await.unwrap_err();
        assert!(error.to_string().contains("initial connection"), "{error}");
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn server_name_reservation_is_scoped_and_released_on_unload() {
        let _serial = status_test_lock().lock().await;
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let config = json!({
            "transport":"stdio",
            "serverName":"reserved",
            "command":"/definitely/not/a/real/mcp-server",
            "reconnect":{"enabled":false}
        });
        let first = ctx.plugin::<McpClient>(config.clone()).unwrap();
        first.settled().await.unwrap();
        let duplicate = ctx.plugin::<McpClient>(config.clone()).unwrap();
        assert!(
            duplicate
                .settled()
                .await
                .unwrap_err()
                .to_string()
                .contains("already in use")
        );
        first.dispose().await;
        let replacement = ctx.plugin::<McpClient>(config).unwrap();
        replacement.settled().await.unwrap();
        replacement.dispose().await;
        kernel.shutdown().await;
    }

    #[test]
    fn credential_config_equality_skips_only_unchanged_bindings() {
        let current = Config::StreamableHttp {
            server_name: "docs".into(),
            url: "https://one.invalid".into(),
            headers: Default::default(),
            tool_call_timeout_ms: 10,
            startup_timeout_ms: None,
            close_timeout_ms: 10,
            expand_templates: true,
            fail_on_startup_error: false,
            max_instruction_bytes: None,
            reconnect: None,
        };
        assert!(!binding_config_changed(Some(&current), Some(&current)));
        let mut changed = current.clone();
        if let Config::StreamableHttp { url, .. } = &mut changed {
            *url = "https://two.invalid".into();
        }
        assert!(binding_config_changed(Some(&current), Some(&changed)));
        assert!(binding_config_changed(Some(&current), None));
    }

    #[test]
    fn stable_resync_failure_starts_a_fresh_retry_budget() {
        assert_eq!(
            failed_after_loss(7, Duration::from_secs(30), Duration::from_secs(30)),
            1
        );
        assert_eq!(
            failed_after_loss(7, Duration::from_secs(29), Duration::from_secs(30)),
            8
        );
    }
}
