//! Ownership and execution lifetimes for in-process continuable subagents.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
};

use hoh_agent::{
    Agent, AgentOptions, Agents, CancelOptions, CreateAgentOptions, InboxTarget, Live,
    ResumeAgentOptions,
};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::Context;
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};
use hoh_session::{AgentCancelCause, SessionHeader, SessionId};
use hoh_session_persistence::{SessionPersistenceService, SessionPersistenceStatOptions};
use hoh_session_query::{SessionObservationOptions, SessionQuery};
use hoh_timeout::AbortSignal;
use tokio::sync::oneshot;

use crate::{
    archive_admission::install_subagent_archive_admission,
    assistant_output::final_assistant_output,
    catalog::{catalog_names, establish_catalog_child},
    child_agent::{
        ChildComposition, append_delegated_policy_overrides, apply_child_composition,
        capture_delegated_policy_overrides, child_session_meta, resolve_child_agent_options,
        resolve_child_depth,
    },
    continuation_messages::{create_settlement_message, with_result_delivery_guidance},
    descriptor::{
        ContinuableSubagentDescriptorData, SubagentDescriptorData, descriptor_value,
        snapshot_subagent_descriptor,
    },
    error::{SubagentError, SubagentErrorCode},
    lifecycle::{emit_subagent_end, emit_subagent_start, epoch_stop_reason},
    names::{
        ROOT_AGENT_NAME, SiblingNameReservation, SiblingNameReservations, agent_address,
        join_agent_path, parent_agent_path,
    },
    projection_types::{SubagentCatalogEntry, SubagentIdentityProjection},
    structured::{StructuredAttachment, attach_structured_runtime},
    types::{
        ContinuableCreateRequest, SubagentActivation, SubagentActivationSpec,
        SubagentDeliveryTarget, SubagentInterruptAuthority, SubagentMemberNames, SubagentResult,
        SubagentRunEndInfo, SubagentRunId, SubagentRunInfo, SubagentSendMessageOptions,
        SubagentStopReason,
    },
};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn agent_key(agent: &Arc<dyn Agent>) -> usize {
    Arc::as_ptr(agent) as *const () as usize
}

struct Resident {
    handle: hoh_agent::AgentHandle,
    root: Arc<dyn Agent>,
    ancestors: Vec<SessionId>,
    path: Option<String>,
    run: SubagentRunInfo,
    structured: Option<StructuredAttachment>,
}

struct ExternalResident {
    run: Arc<dyn crate::types::SubagentRun>,
    root: Arc<dyn Agent>,
    ancestors: Vec<SessionId>,
}

#[derive(Default)]
struct ManagerState {
    resident: HashMap<SessionId, Arc<Resident>>,
    external: HashMap<SessionId, Arc<ExternalResident>>,
    active_by_root: HashMap<usize, usize>,
    closing_roots: HashSet<usize>,
    draining: bool,
    /// Settlement tasks (child result, parent notice) still running, per tree root.
    settling: HashMap<usize, usize>,
}

type ChildAncestry = (Arc<dyn Agent>, Vec<SessionId>, Option<String>);

#[derive(Clone)]
pub struct SubagentManager {
    ctx: Context,
    state: Arc<Mutex<ManagerState>>,
    names: SiblingNameReservations,
    max_active: Arc<dyn Fn() -> usize + Send + Sync>,
    settled: Arc<tokio::sync::Notify>,
}

/// Keeps a settlement task visible to `wait_for_tree_work` until its parent notice was sent: the source
/// joins each child's result promise, which resolves only after that notice.
pub(crate) struct SettlingGuard {
    state: Arc<Mutex<ManagerState>>,
    settled: Arc<tokio::sync::Notify>,
    root: usize,
}

impl Drop for SettlingGuard {
    fn drop(&mut self) {
        {
            let mut state = lock(&self.state);
            if let Some(count) = state.settling.get_mut(&self.root) {
                *count -= 1;
                if *count == 0 {
                    state.settling.remove(&self.root);
                }
            }
        }
        self.settled.notify_waiters();
    }
}

impl SubagentManager {
    pub(crate) fn begin_settling(&self, root: &Arc<dyn Agent>) -> SettlingGuard {
        let root = agent_key(root);
        *lock(&self.state).settling.entry(root).or_default() += 1;
        SettlingGuard {
            state: self.state.clone(),
            settled: self.settled.clone(),
            root,
        }
    }

    /// Deliver a settlement notice to its parent as `notifySettlement` does: a closing tree injects
    /// it, an idle parent queues it, a running parent is steered so it reads it at its next step.
    pub(crate) fn notify_parent(
        &self,
        parent: &Arc<dyn Agent>,
        message: hoh_llm::UserMessage,
    ) -> Result<(), BoxError> {
        let root = self.root_of(parent).ok().map(|root| agent_key(&root));
        let closing = {
            let state = lock(&self.state);
            state.draining || root.is_some_and(|root| state.closing_roots.contains(&root))
        };
        if closing {
            parent.inject(message)
        } else if parent.status() == hoh_agent::AgentStatus::Idle {
            parent.followup(message)
        } else {
            parent.steer(message)
        }
    }

    pub fn new(ctx: Context, max_active: impl Fn() -> usize + Send + Sync + 'static) -> Self {
        let manager = Self {
            ctx,
            state: Arc::default(),
            names: SiblingNameReservations::default(),
            max_active: Arc::new(max_active),
            settled: Arc::default(),
        };
        let state = manager.state.clone();
        let ancestors = Arc::new(move |agent: &Arc<dyn Agent>| {
            lock(&state)
                .resident
                .get(agent.id())
                .filter(|resident| Arc::ptr_eq(&resident.handle.agent, agent))
                .map(|resident| resident.ancestors.clone())
        });
        let identity = Arc::new(|session: &hoh_session::Session| {
            let events = session.snapshot_events(None, None);
            let event = events
                .iter()
                .find(|event| event.event_type() == "subagent/descriptor")?;
            let descriptor = crate::descriptor::fold_subagent_descriptor(&events).ok()??;
            match descriptor {
                SubagentDescriptorData::OneShot(value) => Some(
                    SubagentIdentityProjection::one_shot(value.label, event.seq()),
                ),
                SubagentDescriptorData::Continuable(value) => Some(
                    SubagentIdentityProjection::continuable(value.label, event.seq()),
                ),
            }
        });
        if let Err(error) = install_subagent_archive_admission(&manager.ctx, ancestors, identity) {
            manager.ctx.logger().warn(format_args!(
                "subagent: archive admission could not be installed: {error}"
            ));
        }
        manager
    }

    pub async fn start_local(
        &self,
        spec: SubagentActivationSpec,
        provider: Arc<dyn crate::types::SubagentProvider>,
        cwd: String,
        ensure_provider_current: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>,
    ) -> Result<SubagentActivation, BoxError> {
        let agents = self.ctx.get::<Agents>().ok_or_else(|| {
            subagent_error(
                "continuable subagents require the agents service",
                SubagentErrorCode::ContinuationUnavailable,
            )
        })?;
        if !agents
            .get(spec.request.parent.id())
            .is_some_and(|live| Arc::ptr_eq(&live, &spec.request.parent))
        {
            return Err(subagent_error(
                "subagent creation requires the exact live parent agent",
                SubagentErrorCode::Unauthorized,
            ));
        }
        spec.request.signal.throw_if_aborted()?;
        let persistence = self.ctx.get::<SessionPersistenceService>();
        if spec.delivery == SubagentDeliveryTarget::Parent && persistence.is_none() {
            return Err(subagent_error(
                "continuable subagents require session persistence (load a dsh-session-persistence backend)",
                SubagentErrorCode::PersistenceUnavailable,
            ));
        }
        if let (Some(child_id), Some(persistence)) = (&spec.child_id, persistence) {
            let persisted = persistence
                .stat(
                    child_id,
                    Some(&SessionPersistenceStatOptions {
                        signal: Some(spec.request.signal.clone()),
                    }),
                )
                .await?;
            spec.request.signal.throw_if_aborted()?;
            if persisted.is_some() {
                return Err(subagent_error(
                    format!("subagent \"{child_id}\" already exists"),
                    SubagentErrorCode::DuplicateChild,
                ));
            }
        }
        let (root, ancestors, parent_path) = self.ancestry_for_child(&spec.request.parent)?;
        self.reserve_capacity(&root)?;
        let root_key = agent_key(&root);

        let committed = committed_catalog(&spec.request.parent);
        let mut reservation = match self.names.reserve(
            &spec.request.parent,
            &catalog_names(&committed),
            spec.name.as_deref(),
        ) {
            Ok(value) => value,
            Err(error) => {
                self.release_capacity(root_key);
                return Err(error);
            }
        };
        let name = reservation.name().to_owned();
        let path = join_agent_path(parent_path.as_deref(), Some(&name));
        let child_id = spec
            .child_id
            .clone()
            .unwrap_or_else(|| hoh_util_crypto::random_uuid().as_str().into());
        if agents.get(&child_id).is_some() || lock(&self.state).resident.contains_key(&child_id) {
            reservation.release();
            self.release_capacity(root_key);
            return Err(subagent_error(
                format!("subagent \"{child_id}\" already exists"),
                SubagentErrorCode::DuplicateChild,
            ));
        }

        let result = self
            .start_local_reserved(
                &agents,
                spec,
                provider,
                cwd,
                ensure_provider_current,
                root,
                ancestors,
                path,
                name.clone(),
                child_id,
            )
            .await;
        reservation.release();
        if result.is_err() {
            self.release_capacity(root_key);
        }
        result
    }

    pub(crate) fn reserve_external_capacity(
        &self,
        root: &Arc<dyn Agent>,
    ) -> Result<usize, BoxError> {
        self.reserve_capacity(root)?;
        Ok(agent_key(root))
    }

    pub(crate) fn release_external_capacity(&self, root_key: usize) {
        self.release_capacity(root_key);
    }

    pub(crate) fn reserve_child_name(
        &self,
        parent: &Arc<dyn Agent>,
        requested: Option<&str>,
    ) -> Result<SiblingNameReservation, BoxError> {
        let committed = committed_catalog(parent);
        self.names
            .reserve(parent, &catalog_names(&committed), requested)
    }

    pub(crate) fn path_for_member(
        &self,
        member: &Arc<dyn Agent>,
        root: &Arc<dyn Agent>,
    ) -> Option<String> {
        if Arc::ptr_eq(member, root) {
            Some(String::new())
        } else {
            lock(&self.state)
                .resident
                .get(member.id())
                .and_then(|resident| resident.path.clone())
        }
    }

    pub(crate) fn register_external(
        &self,
        parent: &Arc<dyn Agent>,
        root: &Arc<dyn Agent>,
        child_id: &SessionId,
        run: Arc<dyn crate::types::SubagentRun>,
    ) -> Result<(), BoxError> {
        let agents = self.ctx.get::<Agents>().ok_or_else(|| {
            subagent_error(
                "subagent creation requires the agents service",
                SubagentErrorCode::ContinuationUnavailable,
            )
        })?;
        let ancestors = if Arc::ptr_eq(parent, root) {
            vec![parent.id().clone()]
        } else {
            let state = lock(&self.state);
            let parent_resident = state.resident.get(parent.id()).ok_or_else(|| {
                subagent_error(
                    "agent has no active runtime owner",
                    SubagentErrorCode::Unauthorized,
                )
            })?;
            let mut ancestors = parent_resident.ancestors.clone();
            ancestors.push(parent.id().clone());
            ancestors
        };
        let mut state = lock(&self.state);
        if agents.get(child_id).is_some()
            || state.resident.contains_key(child_id)
            || state.external.contains_key(child_id)
        {
            return Err(subagent_error(
                format!("subagent \"{child_id}\" already exists"),
                SubagentErrorCode::DuplicateChild,
            ));
        }
        state.external.insert(
            child_id.clone(),
            Arc::new(ExternalResident {
                run,
                root: root.clone(),
                ancestors,
            }),
        );
        Ok(())
    }

    pub(crate) fn unregister_external(
        &self,
        child_id: &SessionId,
        run: &Arc<dyn crate::types::SubagentRun>,
    ) {
        let mut state = lock(&self.state);
        if state
            .external
            .get(child_id)
            .is_some_and(|resident| Arc::ptr_eq(&resident.run, run))
        {
            state.external.remove(child_id);
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_local_reserved(
        &self,
        agents: &hoh_agent::AgentRegistry,
        spec: SubagentActivationSpec,
        provider: Arc<dyn crate::types::SubagentProvider>,
        cwd: String,
        ensure_provider_current: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>,
        root: Arc<dyn Agent>,
        ancestors: Vec<SessionId>,
        path: Option<String>,
        name: String,
        child_id: SessionId,
    ) -> Result<SubagentActivation, BoxError> {
        let child_depth =
            resolve_child_depth(spec.request.parent.as_ref(), spec.request.max_depth)?;
        let agent_options = resolve_child_agent_options(
            spec.request.parent.as_ref(),
            spec.request.agent_options.as_ref(),
            child_depth,
        );
        let descriptor = snapshot_subagent_descriptor(&ContinuableSubagentDescriptorData {
            provider: spec.provider.clone(),
            label: spec.label.clone(),
            agent_provider: agent_options.provider(),
            agent_model: agent_options.model(),
            agent_reasoning_effort: agent_options.reasoning_effort(),
            persona: None,
            tool_filter: None,
        })?;
        let policies = capture_delegated_policy_overrides(
            spec.request.parent.ctx(),
            spec.request.parent.as_ref(),
        )?;
        let prepare = provider
            .prepare_continuable(ContinuableCreateRequest {
                cwd: cwd.clone(),
                session_id: child_id.clone(),
                parent: spec.request.parent.clone(),
                signal: spec.request.signal.clone(),
            })
            .ok_or_else(|| {
                subagent_error(
                    format!(
                        "subagent provider \"{}\" must implement start or prepareContinuable",
                        provider.name()
                    ),
                    SubagentErrorCode::UnsupportedCapability,
                )
            })?;
        let prepared = prepare.await?;
        ensure_provider_current()?;
        spec.request.signal.throw_if_aborted()?;

        let structured_slot = Arc::new(Mutex::new(None::<StructuredAttachment>));
        let setup_structured = structured_slot.clone();
        let setup_schema = spec.request.output_schema.clone();
        let setup_composition = ChildComposition {
            agent_preset: spec.request.agent_preset.clone(),
            persona: None,
            tool_filter: None,
        };
        let setup_parent = spec.request.parent.clone();
        let setup_policies = policies.clone();
        let setup_descriptor = descriptor.clone();
        let setup_cwd = cwd.clone();
        let setup_signal = spec.request.signal.clone();
        let setup_selected_preset = spec.request.agent_preset.clone();
        let setup = Arc::new(move |ctx: Context, child: Arc<dyn Agent>| {
            let composition = setup_composition.clone();
            let parent = setup_parent.clone();
            let policies = setup_policies.clone();
            let descriptor = setup_descriptor.clone();
            let schema = setup_schema.clone();
            let slot = setup_structured.clone();
            let cwd = setup_cwd.clone();
            let signal = setup_signal.clone();
            let selected_preset = setup_selected_preset.clone();
            Box::pin(async move {
                apply_child_composition(&ctx, Some(parent.as_ref()), &composition).await?;
                if let Some(agent_preset) = selected_preset {
                    child.session().append(
                        "agent-preset/selected",
                        serde_json::json!({ "agentPreset": agent_preset }),
                        None,
                    )?;
                }
                child.session().append(
                    "subagent/descriptor",
                    descriptor_value(&SubagentDescriptorData::Continuable(descriptor)),
                    None,
                )?;
                append_delegated_policy_overrides(child.session(), &policies)?;
                let working_directory = ctx
                    .get::<hoh_working_directory::WorkingDirectoryKey>()
                    .ok_or_else(|| {
                        subagent_error(
                            "local subagents require the working-directory service",
                            SubagentErrorCode::WorkingDirectoryUnavailable,
                        )
                    })?;
                working_directory.set(&child, &cwd, Some(&signal)).await?;
                if let Some(schema) = schema {
                    let completion_child = child.clone();
                    *lock(&slot) = Some(attach_structured_runtime(&ctx, schema, move || {
                        completion_child.inbox().next_step().is_empty()
                            && completion_child.inbox().next_turn().is_empty()
                    })?);
                }
                Ok(None)
            })
                as futures_util::future::BoxFuture<
                    'static,
                    Result<Option<hoh_agent::AgentSetupCommit>, BoxError>,
                >
        });
        let mut options = CreateAgentOptions::new(child_id.clone());
        options.meta = Some(child_session_meta(
            spec.request.parent.as_ref(),
            child_depth,
            prepared.seed.is_some(),
            spec.request.agent_preset.clone(),
        ));
        options.inherited_event_count = Some(
            hoh_session::SessionLogOffset::new(prepared.seed.as_ref().map_or(0, Vec::len) as u64)
                .expect("seed length is a safe integer"),
        );
        options.seed = prepared.seed;
        options.agent_options = Some(agent_options);
        options.signal = Some(spec.request.signal.clone());
        options.setup = Some(setup);
        let handle = agents.create(&self.ctx, options).await?;

        let header: SessionHeader =
            serde_json::from_value((*handle.agent.session().header()).clone())?;
        let output_start = handle.agent.session().seq();
        let structured = lock(&structured_slot).clone();
        let resident = Arc::new(Resident {
            handle: handle.clone(),
            root: root.clone(),
            ancestors,
            path,
            run: SubagentRunInfo {
                run_id: SubagentRunId::from(hoh_util_crypto::random_uuid().as_str()),
                provider: spec.provider.clone(),
                parent_session_id: spec.request.parent.id().clone(),
                id: child_id.clone(),
                local: true,
            },
            structured: structured.clone(),
        });
        lock(&self.state)
            .resident
            .insert(child_id.clone(), resident.clone());
        emit_subagent_start(root.ctx(), resident.run.clone());

        let prompt = if spec.delivery == SubagentDeliveryTarget::Parent {
            with_result_delivery_guidance(spec.request.prompt.clone())
        } else {
            spec.request.prompt.clone()
        };
        let message = create_user_message(NewUserMessage {
            content: prompt,
            source: MessageSource::new("user"),
        });
        let message_id = message.id.clone();
        let image_admission = self
            .assert_image_capable(&handle.agent, &message.content, &spec.request.signal)
            .await;
        let admission = image_admission
            .and_then(|()| {
                spec.request
                    .signal
                    .throw_if_aborted()
                    .map_err(|error| Box::new(error) as BoxError)
            })
            .and_then(|()| {
                if agents
                    .get(spec.request.parent.id())
                    .is_some_and(|live| Arc::ptr_eq(&live, &spec.request.parent))
                {
                    Ok(())
                } else {
                    Err(subagent_error(
                        "subagent creation requires the exact live parent agent",
                        SubagentErrorCode::Unauthorized,
                    ))
                }
            })
            .and_then(|()| handle.agent.send(message, InboxTarget::NextTurn, true))
            .and_then(|()| {
                establish_catalog_child(spec.request.parent.session(), &header, &spec.label, &name)
            });
        if let Err(error) = admission {
            let teardown_failed = handle.dispose().await.is_err();
            let mut state = lock(&self.state);
            if state
                .resident
                .get(&child_id)
                .is_some_and(|current| Arc::ptr_eq(current, &resident))
            {
                state.resident.remove(&child_id);
            }
            drop(state);
            emit_subagent_end(
                root.ctx(),
                SubagentRunEndInfo::new(resident.run.clone(), SubagentStopReason::Error, None),
            );
            if teardown_failed {
                self.ctx.logger().warn(format_args!(
                    "subagent continuation: disposal after admission or catalog append failure also failed"
                ));
            }
            return Err(error);
        }

        let (send, receive) = oneshot::channel();
        let parent = spec.request.parent.clone();
        let delivery = spec.delivery;
        let child_id_for_task = child_id.clone();
        let name = agent_address(resident.path.as_deref(), child_id.as_str());
        let name_for_task = name.clone();
        let child = handle.agent.clone();
        let lifecycle = resident.run.clone();
        let lifecycle_ctx = root.ctx().clone();
        let settle_manager = self.clone();
        let settle_resident = resident.clone();
        let settling = self.begin_settling(&root);
        tokio::spawn(async move {
            let _settling = settling;
            child.when_idle().await;
            let events = child.session().snapshot_events(Some(output_start), None);
            let stop_reason = epoch_stop_reason(&events);
            let mut terminal = SubagentResult {
                output: final_assistant_output(&events).unwrap_or_default(),
                structured: structured.as_ref().and_then(StructuredAttachment::captured),
                diagnostic: None,
                stop_reason,
            };
            if structured.is_some()
                && terminal.structured.is_none()
                && terminal.stop_reason == SubagentStopReason::Completed
            {
                terminal.stop_reason = SubagentStopReason::Error;
            }
            let teardown_failed = if let Err(error) = settle_manager
                .dispose_exact(&child_id_for_task, &settle_resident)
                .await
            {
                lifecycle_ctx.logger().warn(format_args!(
                    "subagent \"{child_id_for_task}\" activation teardown failed: {error}"
                ));
                true
            } else {
                false
            };
            if delivery == SubagentDeliveryTarget::Parent {
                let notice = create_settlement_message(
                    &child_id_for_task,
                    &name_for_task,
                    &terminal,
                    true,
                    false,
                );
                let _ = settle_manager.notify_parent(&parent, notice);
            }
            emit_subagent_end(
                &lifecycle_ctx,
                SubagentRunEndInfo::new(
                    lifecycle,
                    if teardown_failed {
                        SubagentStopReason::Error
                    } else {
                        terminal.stop_reason.clone()
                    },
                    (!teardown_failed && !terminal.output.is_empty())
                        .then(|| terminal.output.clone()),
                ),
            );
            let _ = send.send(Ok::<_, SharedError>(terminal));
        });

        let manager = self.clone();
        let dispose_id = child_id.clone();
        let dispose_resident = resident.clone();
        let dispose = Arc::new(move || {
            let manager = manager.clone();
            let id = dispose_id.clone();
            let expected = dispose_resident.clone();
            Box::pin(async move { manager.dispose_exact(&id, &expected).await })
                as futures_util::future::BoxFuture<'static, Result<(), BoxError>>
        });
        Ok(SubagentActivation {
            child_id,
            name,
            message_id: Some(message_id),
            result: Box::pin(async move {
                match receive.await {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(error)) => Err(Box::new(error) as BoxError),
                    Err(_) => Err("subagent result task ended before settlement".into()),
                }
            }),
            dispose,
        })
    }

    pub fn root_of(&self, caller: &Arc<dyn Agent>) -> Result<Arc<dyn Agent>, BoxError> {
        let agents = self.ctx.get::<Agents>().ok_or_else(|| {
            subagent_error(
                "continuable subagents require the agents service",
                SubagentErrorCode::ContinuationUnavailable,
            )
        })?;
        if !agents
            .get(caller.id())
            .is_some_and(|live| Arc::ptr_eq(&live, caller))
        {
            return Err(subagent_error(
                "subagent tree access requires the exact live caller",
                SubagentErrorCode::Unauthorized,
            ));
        }
        if let Some(resident) = lock(&self.state).resident.get(caller.id()) {
            if Arc::ptr_eq(&resident.handle.agent, caller)
                && agents
                    .get(resident.root.id())
                    .is_some_and(|live| Arc::ptr_eq(&live, &resident.root))
            {
                return Ok(resident.root.clone());
            }
            return Err(subagent_error(
                "agent has no active runtime owner",
                SubagentErrorCode::Unauthorized,
            ));
        }
        let header: SessionHeader = serde_json::from_value((*caller.session().header()).clone())?;
        let descriptor_is_own = caller
            .session()
            .snapshot_events(None, None)
            .iter()
            .find(|event| event.event_type() == "subagent/descriptor")
            .is_some_and(|event| caller.session().is_own_seq(event.seq()));
        if header.origin.as_deref() == Some("subagent") || descriptor_is_own {
            return Err(subagent_error(
                "agent has no active runtime owner",
                SubagentErrorCode::Unauthorized,
            ));
        }
        Ok(caller.clone())
    }

    pub fn names_of(&self, agent: &Arc<dyn Agent>) -> Result<SubagentMemberNames, BoxError> {
        let root = self.root_of(agent)?;
        if Arc::ptr_eq(&root, agent) {
            return Ok(SubagentMemberNames {
                self_name: ROOT_AGENT_NAME.into(),
                parent: None,
            });
        }
        let state = lock(&self.state);
        let resident = state.resident.get(agent.id()).ok_or_else(|| {
            subagent_error(
                "agent has no active runtime owner",
                SubagentErrorCode::Unauthorized,
            )
        })?;
        let parent_id = resident
            .ancestors
            .last()
            .expect("resident subagent has a creator");
        let parent_path = resident
            .path
            .as_deref()
            .map(parent_agent_path)
            .or_else(|| (parent_id == root.id()).then_some(""));
        Ok(SubagentMemberNames {
            self_name: agent_address(resident.path.as_deref(), agent.id().as_str()),
            parent: Some(agent_address(parent_path, parent_id.as_str())),
        })
    }

    pub async fn send_message(
        &self,
        sender: &Arc<dyn Agent>,
        target_id: &SessionId,
        content: Vec<ContentBlock>,
        options: SubagentSendMessageOptions,
    ) -> Result<hoh_llm::MessageId, BoxError> {
        options.signal.throw_if_aborted()?;
        if sender.id() == target_id {
            return Err(subagent_error(
                "send_message requires another agent in the subagent tree",
                SubagentErrorCode::Unauthorized,
            ));
        }
        let root = self.root_of(sender)?;
        let (target, target_resident, resumed) = if root.id() == target_id {
            (root, None, false)
        } else {
            let (resident, resumed) = self
                .resident_or_resume(&root, target_id, &options.signal)
                .await?;
            if !Arc::ptr_eq(&resident.root, &root) {
                return Err(subagent_error(
                    format!("subagent \"{target_id}\" is outside this subagent tree"),
                    SubagentErrorCode::Unauthorized,
                ));
            }
            (resident.handle.agent.clone(), Some(resident), resumed)
        };
        if let Some(resident) = &target_resident
            && resident
                .structured
                .as_ref()
                .is_some_and(|structured| !structured.accepts_input())
        {
            return Err(subagent_error(
                "subagent is submitting or has submitted its structured result; the message was not accepted",
                SubagentErrorCode::InputClosed,
            ));
        }
        self.assert_image_capable(&target, &content, &options.signal)
            .await?;
        let output_start = target.session().seq();
        let mut source = MessageSource::new("agent-message");
        source.data.insert("form".into(), "relay".into());
        source
            .data
            .insert("senderSessionId".into(), sender.id().as_str().into());
        let sender_name = self.names_of(sender)?.self_name;
        let mut blocks = vec![ContentBlock::text(format!(
            "Agent {sender_name} sent a message: "
        ))];
        blocks.extend(content);
        let message = create_user_message(NewUserMessage {
            content: blocks,
            source,
        });
        let id = message.id.clone();
        target.steer(message)?;
        if resumed {
            let resident = target_resident.expect("a resumed target is resident");
            self.spawn_resumed_settlement(resident, output_start);
        }
        Ok(id)
    }

    pub async fn deliver_prompt(
        &self,
        child_id: &SessionId,
        content: Vec<ContentBlock>,
        source: MessageSource,
        signal: &AbortSignal,
        delivery: crate::control_types::SubagentDelivery,
    ) -> Result<hoh_llm::MessageId, BoxError> {
        signal.throw_if_aborted()?;
        let root = if let Some(resident) = lock(&self.state).resident.get(child_id) {
            resident.root.clone()
        } else {
            self.resolve_live_root_for(child_id, signal).await?
        };
        let (resident, resumed) = self.resident_or_resume(&root, child_id, signal).await?;
        if resident
            .structured
            .as_ref()
            .is_some_and(|structured| !structured.accepts_input())
        {
            return Err(subagent_error(
                "subagent is submitting or has submitted its structured result; the message was not accepted",
                SubagentErrorCode::InputClosed,
            ));
        }
        let target = resident.handle.agent.clone();
        self.assert_image_capable(&target, &content, signal).await?;
        let output_start = target.session().seq();
        let message = create_user_message(NewUserMessage { content, source });
        let id = message.id.clone();
        match delivery {
            crate::control_types::SubagentDelivery::Queue => target.followup(message)?,
            crate::control_types::SubagentDelivery::Steer => target.steer(message)?,
        }
        if resumed {
            self.spawn_resumed_settlement(resident, output_start);
        }
        Ok(id)
    }

    pub fn interrupt(
        &self,
        target_id: &SessionId,
        authority: SubagentInterruptAuthority,
    ) -> Result<(), BoxError> {
        let (target, external) = {
            let state = lock(&self.state);
            (
                state.resident.get(target_id).cloned(),
                state.external.get(target_id).cloned(),
            )
        };
        if target.is_none() && external.is_none() {
            return Ok(());
        }
        if let SubagentInterruptAuthority::Ancestor(caller) = authority {
            let root = self.root_of(&caller)?;
            let (target_root, ancestors) = target
                .as_ref()
                .map(|target| (&target.root, &target.ancestors))
                .or_else(|| {
                    external
                        .as_ref()
                        .map(|target| (&target.root, &target.ancestors))
                })
                .expect("a target was checked above");
            if caller.id() == target_id
                || !Arc::ptr_eq(&root, target_root)
                || !ancestors.contains(caller.id())
            {
                return Err(subagent_error(
                    format!(
                        "subagent \"{target_id}\" is not a live descendant of agent \"{}\"",
                        caller.id()
                    ),
                    SubagentErrorCode::Unauthorized,
                ));
            }
            if let Some(target) = target {
                target
                    .handle
                    .agent
                    .cancel(AgentCancelCause::Parent, CancelOptions { keep_inbox: true })?;
            }
        } else if let Some(target) = target {
            target
                .handle
                .agent
                .cancel(AgentCancelCause::User, CancelOptions { keep_inbox: true })?;
        }
        if let Some(external) = external {
            tokio::spawn(async move {
                let _ = external.run.dispose().await;
            });
        }
        Ok(())
    }

    async fn assert_image_capable(
        &self,
        agent: &Arc<dyn Agent>,
        content: &[ContentBlock],
        signal: &AbortSignal,
    ) -> Result<(), BoxError> {
        if !hoh_llm::content_has_image(content) {
            return Ok(());
        }
        let options = agent.options();
        let (Some(provider), Some(model)) = (options.provider(), options.model()) else {
            return Ok(());
        };
        let Some(llm) = self.ctx.get::<hoh_llm::Llm>() else {
            return Ok(());
        };
        let info = llm
            .resolve_model_info(&provider, &model, Some(signal))
            .await?;
        if info
            .model
            .input_modalities
            .as_ref()
            .is_some_and(|modalities| !modalities.iter().any(|value| value == "image"))
        {
            return Err(subagent_error(
                format!("Model \"{model}\" does not support image input."),
                SubagentErrorCode::ModelDoesNotSupportImages,
            ));
        }
        Ok(())
    }

    async fn resolve_live_root_for(
        &self,
        child_id: &SessionId,
        signal: &AbortSignal,
    ) -> Result<Arc<dyn Agent>, BoxError> {
        let agents = self.ctx.get::<Agents>().ok_or_else(|| {
            subagent_error(
                "continuable subagents require the agents service",
                SubagentErrorCode::ContinuationUnavailable,
            )
        })?;
        for candidate in agents.list() {
            if !Arc::ptr_eq(&self.root_of(&candidate)?, &candidate) {
                continue;
            }
            let rows = crate::list_children::list_descendants(
                &self.ctx,
                candidate.id(),
                Some(signal.clone()),
            )
            .await?;
            if rows.iter().any(|row| match &row.entry {
                crate::control_types::SubagentListEntry::Child { id, .. } => id == child_id,
                crate::control_types::SubagentListEntry::Diagnostic { id, .. } => id == child_id,
            }) {
                return Ok(candidate);
            }
        }
        Err(subagent_error(
            "subagent membership in the active tree could not be verified",
            SubagentErrorCode::Unauthorized,
        ))
    }

    async fn resident_or_resume(
        &self,
        root: &Arc<dyn Agent>,
        child_id: &SessionId,
        signal: &AbortSignal,
    ) -> Result<(Arc<Resident>, bool), BoxError> {
        if let Some(resident) = lock(&self.state).resident.get(child_id).cloned() {
            return Ok((resident, false));
        }
        signal.throw_if_aborted()?;
        let rows =
            crate::list_children::list_descendants(&self.ctx, root.id(), Some(signal.clone()))
                .await?;
        let row = rows
            .iter()
            .find(|row| match &row.entry {
                crate::control_types::SubagentListEntry::Child { id, .. }
                | crate::control_types::SubagentListEntry::Diagnostic { id, .. } => id == child_id,
            })
            .ok_or_else(|| {
                subagent_error(
                    format!("agent \"{child_id}\" is not a member of this subagent tree"),
                    SubagentErrorCode::Unauthorized,
                )
            })?;
        let (mode, path) = match &row.entry {
            crate::control_types::SubagentListEntry::Child { fields, .. } => {
                (fields.mode.as_str(), row.path.clone())
            }
            crate::control_types::SubagentListEntry::Diagnostic { .. } => {
                return Err(subagent_error(
                    format!("subagent \"{child_id}\" cannot be resumed"),
                    SubagentErrorCode::NotResumable,
                ));
            }
        };
        if mode != "continuable" {
            return Err(subagent_error(
                format!("subagent \"{child_id}\" cannot be resumed"),
                SubagentErrorCode::NotResumable,
            ));
        }
        let query = self.ctx.get::<SessionQuery>().ok_or_else(|| {
            subagent_error(
                "continuing subagents requires the sessionQuery service",
                SubagentErrorCode::ContinuationUnavailable,
            )
        })?;
        let observation = match query
            .observe_session(
                child_id,
                SessionObservationOptions {
                    signal: Some(signal.clone()),
                    ..SessionObservationOptions::default()
                },
            )
            .await
        {
            Ok(observation) => observation,
            Err(error) => {
                signal.throw_if_aborted()?;
                return Err(Box::new(SubagentError::with_cause(
                    format!("subagent \"{child_id}\" is unavailable"),
                    SubagentErrorCode::NotResumable,
                    error,
                )));
            }
        };
        let inherited = observation.inherited_event_count().get() as usize;
        let events = observation.events();
        let own_events = events.get(inherited..).ok_or_else(|| {
            subagent_error(
                format!("subagent \"{child_id}\" has a corrupt continuation descriptor and cannot be resumed; choose a different target"),
                SubagentErrorCode::NotResumable,
            )
        })?;
        let descriptor = match crate::descriptor::fold_subagent_descriptor(own_events).map_err(|error| {
            Box::new(SubagentError::with_cause(
                format!("subagent \"{child_id}\" has a corrupt continuation descriptor and cannot be resumed; choose a different target"),
                SubagentErrorCode::NotResumable,
                error,
            )) as BoxError
        })? {
            Some(SubagentDescriptorData::Continuable(value)) => value,
            _ => {
                return Err(subagent_error(
                    format!("subagent \"{child_id}\" cannot be resumed"),
                    SubagentErrorCode::NotResumable,
                ));
            }
        };
        let mut by_child = HashMap::new();
        for row in &rows {
            let id = match &row.entry {
                crate::control_types::SubagentListEntry::Child { id, .. }
                | crate::control_types::SubagentListEntry::Diagnostic { id, .. } => id,
            };
            by_child.insert(id.clone(), row.parent_id.clone());
        }
        let mut reversed = Vec::new();
        let mut cursor = child_id.clone();
        while let Some(parent) = by_child.get(&cursor) {
            reversed.push(parent.clone());
            if parent == root.id() {
                break;
            }
            cursor = parent.clone();
        }
        reversed.reverse();
        if reversed.first() != Some(root.id()) {
            return Err(subagent_error(
                "subagent membership in the active tree could not be verified",
                SubagentErrorCode::Unauthorized,
            ));
        }
        self.reserve_capacity(root)?;
        let root_key = agent_key(root);
        let selected_preset_event = events
            .iter()
            .any(|event| event.event_type() == "agent-preset/selected");
        let projected_preset = observation
            .projections()
            .and_then(|snapshot| snapshot.values.get("agentPreset"))
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        if projected_preset.is_none() && selected_preset_event {
            self.release_capacity(root_key);
            return Err(subagent_error(
                format!("subagent \"{child_id}\" has no agent preset projection for restoration"),
                SubagentErrorCode::NotResumable,
            ));
        }
        let composition = ChildComposition {
            agent_preset: projected_preset.or_else(|| observation.header().agent_preset.clone()),
            persona: descriptor.persona.clone(),
            tool_filter: descriptor.tool_filter.clone(),
        };
        let setup = Arc::new(move |ctx: Context, _agent: Arc<dyn Agent>| {
            let composition = composition.clone();
            Box::pin(async move {
                apply_child_composition(&ctx, None, &composition).await?;
                Ok(None)
            })
                as futures_util::future::BoxFuture<
                    'static,
                    Result<Option<hoh_agent::AgentSetupCommit>, BoxError>,
                >
        });
        let mut options = ResumeAgentOptions::new(child_id.clone());
        options.agent_options = Some(AgentOptions {
            provider: descriptor.agent_provider.map(Live::Value),
            model: descriptor.agent_model.map(Live::Value),
            reasoning_effort: descriptor.agent_reasoning_effort.map(Live::Value),
            max_tokens: None,
        });
        options.signal = Some(signal.clone());
        options.setup = Some(setup);
        let agents = self.ctx.get::<Agents>().ok_or_else(|| {
            subagent_error(
                "continuable subagents require the agents service",
                SubagentErrorCode::ContinuationUnavailable,
            )
        })?;
        let handle = match agents.resume(&self.ctx, options).await {
            Ok(handle) => handle,
            Err(error) => {
                self.release_capacity(root_key);
                return Err(error);
            }
        };
        let resident = Arc::new(Resident {
            handle,
            root: root.clone(),
            ancestors: reversed,
            path,
            run: SubagentRunInfo {
                run_id: SubagentRunId::from(hoh_util_crypto::random_uuid().as_str()),
                provider: descriptor.provider,
                parent_session_id: row.parent_id.clone(),
                id: child_id.clone(),
                local: true,
            },
            structured: None,
        });
        let previous = {
            let mut state = lock(&self.state);
            if let Some(previous) = state.resident.get(child_id).cloned() {
                Some(previous)
            } else {
                state.resident.insert(child_id.clone(), resident.clone());
                None
            }
        };
        if let Some(previous) = previous {
            let _ = resident.handle.dispose().await;
            self.release_capacity(root_key);
            return Ok((previous, false));
        }
        emit_subagent_start(root.ctx(), resident.run.clone());
        Ok((resident, true))
    }

    fn spawn_resumed_settlement(
        &self,
        resident: Arc<Resident>,
        output_start: hoh_session::SessionLogOffset,
    ) {
        let manager = self.clone();
        let settling = self.begin_settling(&resident.root);
        tokio::spawn(async move {
            let _settling = settling;
            let child = resident.handle.agent.clone();
            child.when_idle().await;
            let events = child.session().snapshot_events(Some(output_start), None);
            let terminal = SubagentResult {
                output: final_assistant_output(&events).unwrap_or_default(),
                structured: None,
                diagnostic: None,
                stop_reason: epoch_stop_reason(&events),
            };
            let id = child.id().clone();
            let teardown_failed = if let Err(error) = manager.dispose_exact(&id, &resident).await {
                resident.root.ctx().logger().warn(format_args!(
                    "subagent \"{id}\" activation teardown failed: {error}"
                ));
                true
            } else {
                false
            };
            let parent = resident
                .ancestors
                .last()
                .and_then(|id| manager.ctx.get::<Agents>()?.get(id));
            if let Some(parent) = parent {
                let name = agent_address(resident.path.as_deref(), child.id().as_str());
                let _ = manager.notify_parent(
                    &parent,
                    create_settlement_message(child.id(), &name, &terminal, true, false),
                );
            }
            emit_subagent_end(
                resident.root.ctx(),
                SubagentRunEndInfo::new(
                    resident.run.clone(),
                    if teardown_failed {
                        SubagentStopReason::Error
                    } else {
                        terminal.stop_reason
                    },
                    (!teardown_failed && !terminal.output.is_empty()).then_some(terminal.output),
                ),
            );
        });
    }

    pub async fn drain_descendants(&self, roots: &[Arc<dyn Agent>]) -> Result<(), BoxError> {
        let root_keys = roots.iter().map(agent_key).collect::<HashSet<_>>();
        let targets = {
            let mut state = lock(&self.state);
            state.closing_roots.extend(root_keys.iter().copied());
            state
                .resident
                .iter()
                .filter(|(_, item)| root_keys.contains(&agent_key(&item.root)))
                .map(|(id, item)| (id.clone(), item.clone()))
                .collect::<Vec<_>>()
        };
        let external = {
            let state = lock(&self.state);
            state
                .external
                .values()
                .filter(|item| root_keys.contains(&agent_key(&item.root)))
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut errors = Vec::new();
        for (id, resident) in targets {
            if let Err(error) = self.dispose_exact(&id, &resident).await {
                errors.push(error.to_string());
            }
        }
        for resident in external {
            if let Err(error) = resident.run.dispose().await {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("subagent activation teardown failed: {}", errors.join("; ")).into())
        }
    }

    pub async fn wait_for_tree_work(&self, root: &Arc<dyn Agent>) -> Result<bool, BoxError> {
        if !Arc::ptr_eq(&self.root_of(root)?, root) {
            return Ok(false);
        }
        let children = lock(&self.state)
            .resident
            .values()
            .filter(|resident| Arc::ptr_eq(&resident.root, root))
            .map(|resident| resident.handle.agent.clone())
            .collect::<Vec<_>>();
        let external = lock(&self.state)
            .external
            .values()
            .filter(|resident| Arc::ptr_eq(&resident.root, root))
            .map(|resident| resident.run.clone())
            .collect::<Vec<_>>();
        let progressing = children
            .iter()
            .filter(|agent| agent.status() == hoh_agent::AgentStatus::Running)
            .cloned()
            .collect::<Vec<_>>();
        for agent in &progressing {
            agent.when_idle().await;
        }
        for run in &external {
            let _ = run.result().await;
        }
        let mut settled_work = false;
        loop {
            let notified = self.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if lock(&self.state)
                .settling
                .get(&agent_key(root))
                .is_none_or(|count| *count == 0)
            {
                break;
            }
            settled_work = true;
            notified.await;
        }
        Ok(!progressing.is_empty() || !external.is_empty() || settled_work)
    }

    pub async fn drain(&self) -> Result<(), BoxError> {
        let targets = {
            let mut state = lock(&self.state);
            state.draining = true;
            state
                .resident
                .iter()
                .map(|(id, resident)| (id.clone(), resident.clone()))
                .collect::<Vec<_>>()
        };
        let external = lock(&self.state)
            .external
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut errors = Vec::new();
        for (id, resident) in targets {
            if let Err(error) = self.dispose_exact(&id, &resident).await {
                errors.push(error.to_string());
            }
        }
        for resident in external {
            if let Err(error) = resident.run.dispose().await {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("subagent activation teardown failed: {}", errors.join("; ")).into())
        }
    }

    async fn dispose_exact(
        &self,
        id: &SessionId,
        expected: &Arc<Resident>,
    ) -> Result<(), BoxError> {
        let should_dispose = lock(&self.state)
            .resident
            .get(id)
            .is_some_and(|current| Arc::ptr_eq(current, expected));
        if !should_dispose {
            return Ok(());
        }
        expected.handle.dispose().await?;
        let mut state = lock(&self.state);
        if state
            .resident
            .get(id)
            .is_some_and(|current| Arc::ptr_eq(current, expected))
        {
            state.resident.remove(id);
            let key = agent_key(&expected.root);
            if let Some(active) = state.active_by_root.get_mut(&key) {
                *active = active.saturating_sub(1);
                if *active == 0 {
                    state.active_by_root.remove(&key);
                }
            }
        }
        Ok(())
    }

    fn ancestry_for_child(&self, parent: &Arc<dyn Agent>) -> Result<ChildAncestry, BoxError> {
        let root = self.root_of(parent)?;
        if Arc::ptr_eq(&root, parent) {
            return Ok((root, vec![parent.id().clone()], Some(String::new())));
        }
        let state = lock(&self.state);
        let resident = state.resident.get(parent.id()).ok_or_else(|| {
            subagent_error(
                "agent has no active runtime owner",
                SubagentErrorCode::Unauthorized,
            )
        })?;
        let mut ancestors = resident.ancestors.clone();
        ancestors.push(parent.id().clone());
        Ok((root, ancestors, resident.path.clone()))
    }

    fn reserve_capacity(&self, root: &Arc<dyn Agent>) -> Result<(), BoxError> {
        let mut state = lock(&self.state);
        let key = agent_key(root);
        if state.draining || state.closing_roots.contains(&key) {
            return Err(subagent_error(
                "subagent manager is draining",
                SubagentErrorCode::Draining,
            ));
        }
        let active = state.active_by_root.entry(key).or_default();
        let capacity = (self.max_active)();
        if *active >= capacity {
            return Err(subagent_error(
                format!(
                    "subagent limit reached (active child limit: {capacity}); wait for an existing child to finish or complete this work with the current agents"
                ),
                SubagentErrorCode::ActivationLimitReached,
            ));
        }
        *active += 1;
        Ok(())
    }

    fn release_capacity(&self, root_key: usize) {
        let mut state = lock(&self.state);
        if let Some(active) = state.active_by_root.get_mut(&root_key) {
            *active = active.saturating_sub(1);
            if *active == 0 {
                state.active_by_root.remove(&root_key);
            }
        }
    }
}

fn committed_catalog(parent: &Arc<dyn Agent>) -> Vec<SubagentCatalogEntry> {
    parent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "subagent/catalog")
        .filter_map(|event| {
            Some(SubagentCatalogEntry {
                id: serde_json::from_value(event.data().get("childId")?.clone()).ok()?,
                created_at: event.data().get("childCreatedAt")?.as_i64()?,
                mode: event.data().get("mode")?.as_str()?.to_owned(),
                label: event
                    .data()
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                name: event
                    .data()
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

fn subagent_error(message: impl Into<String>, code: SubagentErrorCode) -> BoxError {
    Box::new(SubagentError::error(message, code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_agent::{
        AgentOptions, AgentStatus, CancelOptions, InboxTarget, MaintenanceFuture, MaintenanceTask,
        Model,
    };
    use hoh_agent_loop_testkit::{
        AgentLoopTestDependenciesOptions, mount_agent_loop_test_dependencies,
        mount_agent_loop_test_harness,
    };
    use hoh_kernel::Kernel;
    use hoh_scope::ScopeKey;
    use hoh_session::{AgentCancelCause, Session};
    use std::sync::Mutex as StdMutex;

    #[test]
    fn catalog_reader_preserves_committed_names_only() {
        assert_eq!(ROOT_AGENT_NAME, "lead");
    }

    /// A live agent that reports a chosen status and records how a notice was routed to it.
    struct Probe {
        inner: Arc<dyn Agent>,
        status: AgentStatus,
        routes: StdMutex<Vec<&'static str>>,
    }

    impl Agent for Probe {
        fn id(&self) -> &SessionId {
            self.inner.id()
        }
        fn scope_key(&self) -> ScopeKey {
            self.inner.scope_key()
        }
        fn options(&self) -> &AgentOptions {
            self.inner.options()
        }
        fn session(&self) -> &Session {
            self.inner.session()
        }
        fn model(&self) -> &dyn Model {
            self.inner.model()
        }
        fn inbox(&self) -> &dyn hoh_agent::Inbox {
            self.inner.inbox()
        }
        fn status(&self) -> AgentStatus {
            self.status
        }
        fn ctx(&self) -> &Context {
            self.inner.ctx()
        }
        fn cancel(&self, cause: AgentCancelCause, options: CancelOptions) -> Result<(), BoxError> {
            self.inner.cancel(cause, options)
        }
        fn when_idle(&self) -> futures_util::future::BoxFuture<'static, ()> {
            self.inner.when_idle()
        }
        fn run_maintenance_erased(
            &self,
            task: MaintenanceTask,
        ) -> Result<MaintenanceFuture, BoxError> {
            self.inner.run_maintenance_erased(task)
        }
        fn send(
            &self,
            message: hoh_llm::UserMessage,
            target: InboxTarget,
            wakeup: bool,
        ) -> Result<(), BoxError> {
            self.inner.send(message, target, wakeup)
        }
        fn followup(&self, _message: hoh_llm::UserMessage) -> Result<(), BoxError> {
            lock_std(&self.routes).push("followup");
            Ok(())
        }
        fn steer(&self, _message: hoh_llm::UserMessage) -> Result<(), BoxError> {
            lock_std(&self.routes).push("steer");
            Ok(())
        }
        fn inject(&self, _message: hoh_llm::UserMessage) -> Result<(), BoxError> {
            lock_std(&self.routes).push("inject");
            Ok(())
        }
    }

    fn lock_std<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn route_of(status: AgentStatus) -> Vec<&'static str> {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount_agent_loop_test_dependencies(&ctx, AgentLoopTestDependenciesOptions::default())
            .await
            .unwrap();
        let harness = mount_agent_loop_test_harness(&ctx).await.unwrap();
        let inner = harness.create(SessionId::from("parent")).await.unwrap();
        let probe = Arc::new(Probe {
            inner,
            status,
            routes: StdMutex::new(Vec::new()),
        });
        let manager = SubagentManager::new(ctx.clone(), || 1);
        let parent: Arc<dyn Agent> = probe.clone();
        let message = hoh_llm::create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("notice")],
            source: MessageSource::new("subagent-settled"),
        });
        manager.notify_parent(&parent, message).unwrap();
        let routes = lock_std(&probe.routes).clone();
        kernel.shutdown().await;
        routes
    }

    /// `notifySettlement`: a parent that is mid-turn is steered so `wait_agent` and the next step
    /// see the notice; an idle parent gets it as a queued follow-up.
    #[tokio::test]
    async fn a_running_parent_is_steered_and_an_idle_parent_is_queued() {
        assert_eq!(route_of(AgentStatus::Running).await, ["steer"]);
        assert_eq!(route_of(AgentStatus::Idle).await, ["followup"]);
    }
}
