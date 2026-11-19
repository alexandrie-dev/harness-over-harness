//! Plugin-owned human-command registry shared by interactive UI adapters.

pub mod brand;
pub mod builtin;
pub mod types;

pub use brand::{CommandDefinitionId, CommandId};
pub use types::*;

use futures_util::{FutureExt, future::BoxFuture};
use hoh_agent::Agent;
use hoh_attachment::{
    AttachmentError, AttachmentErrorCode, Attachments, EncodedImageAttachment, FileAttachmentRef,
    admit_encoded_images, is_attachment_error,
};
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Contained, Context, Disposer, ListenerFailure, NoConfig, Plugin, Service};
use hoh_llm::{ContentBlock, FileBlock, ImageBlock};
use hoh_scope::store::{EffectOptions, EntryUndo};
use hoh_scope::{NamedEntries, ScopeLayer, ScopedLayers};
use hoh_session::Session;
use hoh_timeout::AbortSignal;
use serde::Serialize;
use std::{
    any::Any,
    error::Error,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, MutexGuard},
};

pub const NAME: &str = "commands";
const COMMAND_NAME_PATTERN: &str = "/^[a-z][a-z0-9_-]*$/u";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug)]
struct CommandError(String);
impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for CommandError {}

fn error(message: impl Into<String>) -> BoxError {
    Box::new(CommandError(message.into()))
}

#[derive(Clone)]
struct RegisteredCommand {
    definition: CommandDefinition,
    descriptor: CommandDescriptor,
}

struct CommandLayer {
    commands: NamedEntries<Arc<RegisteredCommand>>,
}

impl CommandLayer {
    fn new(scoped: bool) -> Self {
        Self {
            commands: NamedEntries::new(move |name| {
                error(if scoped {
                    format!("command \"{name}\" is already registered in this scope")
                } else {
                    format!(
                        "command \"{name}\" is already registered (for a per-agent variant, mount a command-injected plugin under that agent's `agent.ctx`)"
                    )
                })
            }),
        }
    }
}

impl ScopeLayer for CommandLayer {
    fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

pub struct CommandsChange;
hoh_kernel::event!(CommandsChange = "commands/change", Contained);

pub type CommandFileReceiptResolver =
    dyn Fn(&Arc<dyn Agent>, &str) -> Option<FileAttachmentRef> + Send + Sync;

struct RuntimeInner {
    ctx: Context,
    layers: ScopedLayers<CommandLayer>,
    command_seq: Mutex<u64>,
    instance_token: String,
    file_receipts: Arc<Mutex<Option<Arc<CommandFileReceiptResolver>>>>,
}

#[derive(Clone)]
pub struct CommandRuntime {
    inner: Arc<RuntimeInner>,
}

hoh_kernel::service!(pub Commands: CommandRuntime = "commands");

impl CommandRuntime {
    pub fn new(ctx: Context) -> Result<Self, BoxError> {
        let notify_ctx = ctx.clone();
        let layers = ScopedLayers::new(
            |scope| Ok(CommandLayer::new(scope.is_some())),
            move || {
                notify_change(&notify_ctx);
                Ok(())
            },
        )?;
        let uuid = hoh_util_crypto::random_uuid();
        Ok(Self {
            inner: Arc::new(RuntimeInner {
                ctx,
                layers,
                command_seq: Mutex::new(0),
                instance_token: uuid.as_str()[..8].to_owned(),
                file_receipts: Arc::new(Mutex::new(None)),
            }),
        })
    }

    /// Register a global or calling-agent-scoped command.
    pub fn register(
        &self,
        ctx: &Context,
        definition: CommandDefinition,
    ) -> Result<Disposer, BoxError> {
        let registered = Arc::new(normalize_definition(definition)?);
        self.inner.layers.effect(
            ctx,
            move |layer| {
                layer
                    .commands
                    .insert(registered.definition.name.clone(), registered)
            },
            EffectOptions::new("commands.register()"),
        )
    }

    /// Register the sole authority resolving Session-scoped staged file receipts.
    pub fn register_file_receipt_resolver(
        &self,
        resolver: Arc<CommandFileReceiptResolver>,
    ) -> Result<EntryUndo, BoxError> {
        let mut slot = lock(&self.inner.file_receipts);
        if slot.is_some() {
            return Err(error(
                "commands: a file receipt resolver is already registered",
            ));
        }
        *slot = Some(resolver.clone());
        drop(slot);
        let receipts = self.inner.file_receipts.clone();
        Ok(EntryUndo::new(move || {
            let mut slot = lock(&receipts);
            if slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &resolver))
            {
                *slot = None;
            }
        }))
    }

    /// Name-sorted descriptors after exact agent-scope shadowing.
    pub fn list(&self, agent: &Arc<dyn Agent>) -> Vec<CommandDescriptor> {
        let mut descriptors = self
            .view(agent)
            .into_values()
            .map(|registered| registered.descriptor.clone())
            .collect::<Vec<_>>();
        descriptors.sort_by(|left, right| left.name.cmp(&right.name));
        descriptors
    }

    /// Resolve one effective command definition.
    pub fn find(&self, agent: &Arc<dyn Agent>, name: &str) -> Option<CommandDefinition> {
        self.view(agent)
            .get(name)
            .map(|registered| registered.definition.clone())
    }

    /// Parse and execute a known command without sending it to the model.
    ///
    /// Parsing, resolution, cancellation admission, `command/run`, and a plain
    /// handler invocation happen before this method returns, matching the
    /// synchronous prefix of the source async function.
    pub fn execute(
        &self,
        agent: Arc<dyn Agent>,
        line: impl Into<String>,
        submitted_attachments: Vec<CommandSubmitAttachment>,
        signal: AbortSignal,
    ) -> BoxFuture<'static, Result<Option<CommandExecution>, BoxError>> {
        let line = line.into();
        let Some(parsed) = parse_command(&line) else {
            return Box::pin(async { Ok(None) });
        };
        let Some(command) = self.view(&agent).get(&parsed.name).cloned() else {
            return Box::pin(async { Ok(None) });
        };
        if let Err(reason) = signal.throw_if_aborted() {
            return Box::pin(async move { Err(reason.into()) });
        }
        let command_id = match self.start_lifecycle(&agent, &parsed, &command.definition) {
            Ok(command_id) => command_id,
            Err(failure) => return Box::pin(async move { Err(failure) }),
        };

        if submitted_attachments.is_empty() {
            let invocation = CommandInvocation {
                command_id: command_id.clone(),
                agent: agent.clone(),
                raw_input: parsed.raw_input,
                attachments: Arc::from([]),
                signal: signal.clone(),
            };
            let handler = match invoke_handler(&command.definition, invocation) {
                Ok(handler) => handler,
                Err(failure) => {
                    self.settle_thrown(
                        agent.session(),
                        &parsed.name,
                        &command_id,
                        failure.as_ref(),
                    );
                    return Box::pin(async move { Err(failure) });
                }
            };
            let runtime = self.clone();
            let name = parsed.name;
            return Box::pin(async move {
                match with_abort(handler, &signal).await {
                    Ok(result) => match normalize_result(&name, result) {
                        Ok(result) => runtime
                            .settle(agent.session(), command_id, result)
                            .map(Some),
                        Err(failure) => {
                            runtime.settle_thrown(
                                agent.session(),
                                &name,
                                &command_id,
                                failure.as_ref(),
                            );
                            Err(failure)
                        }
                    },
                    Err(failure) => {
                        runtime.settle_thrown(
                            agent.session(),
                            &name,
                            &command_id,
                            failure.as_ref(),
                        );
                        Err(failure)
                    }
                }
            });
        }

        if !command
            .definition
            .input
            .as_ref()
            .is_some_and(CommandInputDescriptor::accepts_attachments)
        {
            let result =
                CommandResult::error(format!("/{} does not accept attachments", parsed.name));
            return ready_settlement(self, agent.session(), command_id, result);
        }
        let Some(store) = self.inner.ctx.get::<Attachments>() else {
            let result = CommandResult::error(format!(
                "/{}: attachments are unavailable because no attachment store is composed",
                parsed.name
            ));
            return ready_settlement(self, agent.session(), command_id, result);
        };

        let runtime = self.clone();
        Box::pin(async move {
            let attachments = match runtime
                .admit_command_attachments(&agent, &store, &submitted_attachments)
                .await
            {
                Ok(attachments) => attachments,
                Err(failure) if is_attachment_error(failure.as_ref()) => {
                    let result = CommandResult::error(render_error(failure.as_ref()));
                    return runtime
                        .settle(agent.session(), command_id, result)
                        .map(Some);
                }
                Err(failure) => {
                    runtime.settle_thrown(
                        agent.session(),
                        &parsed.name,
                        &command_id,
                        failure.as_ref(),
                    );
                    return Err(failure);
                }
            };
            if let Err(reason) = signal.throw_if_aborted() {
                runtime.settle_thrown(agent.session(), &parsed.name, &command_id, &reason);
                return Err(reason.into());
            }
            let invocation = CommandInvocation {
                command_id: command_id.clone(),
                agent: agent.clone(),
                raw_input: parsed.raw_input,
                attachments,
                signal: signal.clone(),
            };
            let handler = match invoke_handler(&command.definition, invocation) {
                Ok(handler) => handler,
                Err(failure) => {
                    runtime.settle_thrown(
                        agent.session(),
                        &parsed.name,
                        &command_id,
                        failure.as_ref(),
                    );
                    return Err(failure);
                }
            };
            match with_abort(handler, &signal).await {
                Ok(result) => match normalize_result(&parsed.name, result) {
                    Ok(result) => runtime
                        .settle(agent.session(), command_id, result)
                        .map(Some),
                    Err(failure) => {
                        runtime.settle_thrown(
                            agent.session(),
                            &parsed.name,
                            &command_id,
                            failure.as_ref(),
                        );
                        Err(failure)
                    }
                },
                Err(failure) => {
                    runtime.settle_thrown(
                        agent.session(),
                        &parsed.name,
                        &command_id,
                        failure.as_ref(),
                    );
                    Err(failure)
                }
            }
        })
    }

    fn view(
        &self,
        agent: &Arc<dyn Agent>,
    ) -> indexmap::map::IndexMap<String, Arc<RegisteredCommand>> {
        let scope = agent.scope_key();
        self.inner
            .layers
            .merge(Some(&scope), |layer| &layer.commands)
    }

    fn start_lifecycle(
        &self,
        agent: &Arc<dyn Agent>,
        parsed: &ParsedCommand,
        definition: &CommandDefinition,
    ) -> Result<CommandId, BoxError> {
        let sequence = {
            let mut sequence = lock(&self.inner.command_seq);
            *sequence += 1;
            *sequence
        };
        let command_id = CommandId::from(format!("cmd-{}-{sequence}", self.inner.instance_token));
        let event = CommandRun {
            command_id: command_id.clone(),
            name: parsed.name.clone(),
            args: (definition.record_input != Some(false)).then(|| parsed.raw_input.clone()),
            source: CommandSource::User,
        };
        append_lifecycle(agent.session(), "command/run", &event)?;
        Ok(command_id)
    }

    fn settle(
        &self,
        session: &Session,
        command_id: CommandId,
        result: CommandResult,
    ) -> Result<CommandExecution, BoxError> {
        append_lifecycle(
            session,
            "command/done",
            &CommandDone {
                command_id: command_id.clone(),
                kind: result.kind(),
                text: result.text().map(str::to_owned),
                source_event_seq: result.source_event_seq(),
            },
        )?;
        Ok(CommandExecution { command_id, result })
    }

    fn settle_thrown(
        &self,
        session: &Session,
        command: &str,
        command_id: &CommandId,
        failure: &(dyn Error + Send + Sync + 'static),
    ) {
        let append = append_lifecycle(
            session,
            "command/done",
            &CommandDone {
                command_id: command_id.clone(),
                kind: "error",
                text: Some(render_error(failure)),
                source_event_seq: None,
            },
        );
        if let Err(append_failure) = append {
            self.inner.ctx.logger().warn(format_args!(
                "command \"{command}\": command/done append failed: {}",
                render_listener_error(append_failure.as_ref())
            ));
        }
    }

    async fn admit_command_attachments(
        &self,
        agent: &Arc<dyn Agent>,
        store: &Service<Attachments>,
        attachments: &[CommandSubmitAttachment],
    ) -> Result<Arc<[ContentBlock]>, BoxError> {
        let mut files: Vec<(String, FileAttachmentRef)> = Vec::new();
        for attachment in attachments {
            let CommandSubmitAttachment::File { receipt_id } = attachment else {
                continue;
            };
            if files.iter().any(|(known, _)| known == receipt_id) {
                continue;
            }
            let resolver = lock(&self.inner.file_receipts).clone();
            let file = resolver
                .map(|resolver| {
                    catch_unwind(AssertUnwindSafe(|| resolver(agent, receipt_id)))
                        .map_err(panic_payload_error)
                })
                .transpose()?
                .flatten()
                .ok_or_else(|| {
                    BoxError::from(AttachmentError::new(
                        "File upload receipt is unknown for this session.",
                        AttachmentErrorCode::AttachmentNotFound,
                    ))
                })?;
            files.push((receipt_id.clone(), file));
        }
        let images = attachments
            .iter()
            .filter_map(|attachment| match attachment {
                CommandSubmitAttachment::Image {
                    media_type,
                    data,
                    name,
                } => Some(EncodedImageAttachment {
                    media_type: *media_type,
                    data: data.clone(),
                    name: name.clone(),
                }),
                CommandSubmitAttachment::File { .. } => None,
            })
            .collect::<Vec<_>>();
        let image_refs = if images.is_empty() {
            Vec::new()
        } else {
            admit_encoded_images(&**store, &images).await?
        };
        let mut images = image_refs.into_iter();
        let mut blocks = Vec::with_capacity(attachments.len());
        for attachment in attachments {
            match attachment {
                CommandSubmitAttachment::Image { .. } => {
                    let reference = images.next().ok_or_else(|| {
                        error("attachment store returned fewer image references than inputs")
                    })?;
                    blocks.push(
                        ImageBlock {
                            attachment: reference,
                            offloaded: None,
                        }
                        .into(),
                    );
                }
                CommandSubmitAttachment::File { receipt_id } => {
                    let reference = files
                        .iter()
                        .find(|(known, _)| known == receipt_id)
                        .map(|(_, reference)| reference.clone())
                        .ok_or_else(|| error("resolved file receipt disappeared"))?;
                    blocks.push(
                        FileBlock {
                            attachment: reference,
                        }
                        .into(),
                    );
                }
            }
        }
        Ok(blocks.into())
    }
}

impl Plugin for CommandRuntime {
    const NAME: &'static str = NAME;
    type Config = NoConfig;
    type Deps = ();

    async fn apply(
        ctx: Context,
        _deps: Self::Deps,
        _config: Cfg<Self::Config>,
    ) -> Result<(), hoh_kernel::Error> {
        let runtime = Arc::new(Self::new(ctx.clone())?);
        ctx.provide::<Commands>(runtime).await?;
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandRun {
    command_id: CommandId,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<String>,
    source: CommandSource,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandDone {
    command_id: CommandId,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_event_seq: Option<hoh_session::SessionSeq>,
}

fn append_lifecycle(
    session: &Session,
    event_type: &str,
    data: &impl Serialize,
) -> Result<(), BoxError> {
    session.append(event_type, serde_json::to_value(data)?, None)?;
    Ok(())
}

fn ready_settlement(
    runtime: &CommandRuntime,
    session: &Session,
    command_id: CommandId,
    result: CommandResult,
) -> BoxFuture<'static, Result<Option<CommandExecution>, BoxError>> {
    let settled = runtime.settle(session, command_id, result).map(Some);
    Box::pin(async move { settled })
}

fn valid_command_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
}

fn normalize_definition(definition: CommandDefinition) -> Result<RegisteredCommand, BoxError> {
    if !valid_command_name(&definition.name) {
        return Err(error(format!(
            "command name \"{}\" must match {COMMAND_NAME_PATTERN}",
            definition.name
        )));
    }
    if hoh_util_values::js_trim(&definition.description).is_empty() {
        return Err(error(format!(
            "command \"{}\" description must not be empty",
            definition.name
        )));
    }
    let input = definition
        .input
        .as_ref()
        .map(|input| {
            if hoh_util_values::js_trim(&input.hint).is_empty() {
                return Err(error(format!(
                    "command \"{}\" input hint must not be empty",
                    definition.name
                )));
            }
            Ok(CommandInputDescriptor {
                hint: input.hint.clone(),
                attachments: (input.attachments == Some(true)).then_some(true),
            })
        })
        .transpose()?;
    let mut normalized = definition;
    normalized.input = input.clone();
    let descriptor = CommandDescriptor {
        definition_id: normalized.definition_id.clone(),
        name: normalized.name.clone(),
        description: normalized.description.clone(),
        input,
    };
    Ok(RegisteredCommand {
        definition: normalized,
        descriptor,
    })
}

fn normalize_result(command: &str, result: CommandResult) -> Result<CommandResult, BoxError> {
    if let CommandResult::Error { text } = &result
        && hoh_util_values::js_trim(text).is_empty()
    {
        return Err(error(format!(
            "command \"{command}\" error text must be a non-empty string"
        )));
    }
    Ok(result)
}

async fn with_abort(
    handler: CommandFuture,
    signal: &AbortSignal,
) -> Result<CommandResult, BoxError> {
    signal
        .throw_if_aborted()
        .map_err(|reason| Box::new(reason) as BoxError)?;
    let handler = AssertUnwindSafe(handler).catch_unwind();
    tokio::select! {
        biased;
        reason = signal.cancelled() => Err(reason.into()),
        result = handler => {
            if let Some(reason) = signal.reason() {
                Err(reason.into())
            } else {
                result.map_err(panic_payload_error)?
            }
        }
    }
}

fn invoke_handler(
    definition: &CommandDefinition,
    invocation: CommandInvocation,
) -> Result<CommandFuture, BoxError> {
    catch_unwind(AssertUnwindSafe(|| (definition.handler)(invocation))).map_err(panic_payload_error)
}

fn panic_payload_error(payload: Box<dyn Any + Send>) -> BoxError {
    let message = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "command callback panicked".to_owned());
    error(message)
}

fn render_error(failure: &(dyn Error + 'static)) -> String {
    catch_unwind(AssertUnwindSafe(|| failure.to_string()))
        .unwrap_or_else(|_| "<unrenderable thrown value>".to_owned())
}

fn render_listener_error(failure: &(dyn Error + 'static)) -> String {
    if let Some(failure) = HarnessError::of(failure) {
        format!("{}: {}", failure.name(), failure.message())
    } else {
        format!("Error: {}", render_error(failure))
    }
}

fn notify_change(ctx: &Context) {
    let log = ctx.logger();
    ctx.root().dispatch(CommandsChange).invoke(move |failure| {
        let (kind, failure) = match failure {
            ListenerFailure::Threw(failure) => ("threw", failure),
            ListenerFailure::Rejected(failure) => ("rejected", failure),
        };
        log.warn(format_args!(
            "commands/change listener {kind}: {}",
            render_listener_error(failure.as_ref())
        ));
    });
}

pub fn parse_command(line: &str) -> Option<ParsedCommand> {
    let bytes = line.as_bytes();
    if bytes.first() != Some(&b'/') {
        return None;
    }
    let first = *bytes.get(1)?;
    if !first.is_ascii_lowercase() {
        return None;
    }
    let mut end = 2;
    while let Some(byte) = bytes.get(end)
        && (byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(byte))
    {
        end += 1;
    }
    if bytes
        .get(end)
        .is_some_and(|byte| !matches!(byte, b'\t' | b'\n' | b'\r' | b' '))
    {
        return None;
    }
    Some(ParsedCommand {
        name: line[1..end].to_owned(),
        raw_input: line[end..].to_owned(),
    })
}
