//! Named provider registry and managed activation entry point.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use hoh_attachment::{
    AdmittedPromptContentPart, AttachmentAdmissionPart, Attachments, PromptContentPart,
};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_llm::ContentBlock;
use hoh_session::SessionId;
use hoh_tools::assert_object_json_schema;
use hoh_util_workspace_path::node_path::{is_absolute, resolve};
use hoh_working_directory::WorkingDirectoryKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::{
    catalog::establish_external_catalog_child,
    error::{SubagentError, SubagentErrorCode},
    lifecycle::{
        SubagentProviderAdded, emit_provider_removed, emit_subagent_end, emit_subagent_start,
    },
    list_children::{list_descendants, resolve_agent_path},
    manager::SubagentManager,
    types::{
        ResolvedSubagentStartRequest, SubagentActivation, SubagentActivationSpec,
        SubagentCapabilities, SubagentDeliveryTarget, SubagentInterruptAuthority,
        SubagentMemberNames, SubagentProvider, SubagentRunEndInfo, SubagentRunId, SubagentRunInfo,
        SubagentSendMessageOptions, SubagentStopReason,
    },
};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone)]
pub struct SubagentRuntime {
    ctx: Context,
    providers: Arc<Mutex<HashMap<String, Arc<dyn SubagentProvider>>>>,
    manager: SubagentManager,
    max_depth: Arc<dyn Fn() -> u64 + Send + Sync>,
}

hoh_kernel::service!(pub Subagents: SubagentRuntime = "subagents");

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_max_active_subagents")]
    pub max_active_subagents: usize,
    #[serde(default = "default_max_depth")]
    pub max_depth: u64,
}

const fn default_max_active_subagents() -> usize {
    8
}
const fn default_max_depth() -> u64 {
    1
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_active_subagents: default_max_active_subagents(),
            max_depth: default_max_depth(),
        }
    }
}

#[derive(Inject)]
pub struct Deps {
    projections: Service<hoh_session_projection::SessionProjections>,
    _working_directory: Service<WorkingDirectoryKey>,
}

pub struct SubagentPlugin;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfiguredMaxDepth {
    ProviderManaged,
    Value(u64),
}

impl Plugin for SubagentPlugin {
    const NAME: &'static str = "SubagentRuntime";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let initial = config.get();
        if initial.max_active_subagents == 0 {
            return Err("maxActiveSubagents must be a positive integer".into());
        }
        crate::depth::assert_subagent_max_depth(Some(initial.max_depth as f64))?;
        deps.projections
            .register(
                &ctx,
                crate::catalog::subagent_catalog_projection_definition(),
            )
            .await?;
        deps.projections
            .register(
                &ctx,
                crate::projection::subagent_timing_projection_definition(),
            )
            .await?;
        deps.projections
            .register(
                &ctx,
                crate::projection::subagent_identity_projection_definition(),
            )
            .await?;
        let capacity_config = config.clone();
        let depth_config = config;
        let runtime = SubagentRuntime::with_config(
            ctx.clone(),
            Arc::new(move || capacity_config.get().max_active_subagents),
            Arc::new(move || depth_config.get().max_depth),
        );
        let cleanup = runtime.clone();
        ctx.effect("subagents.manager", move || {
            Undo::future(move || async move { cleanup.manager.drain().await })
        })?;
        ctx.provide::<Subagents>(Arc::new(runtime)).await?;
        Ok(())
    }
}

pub struct ProviderRegistration {
    name: String,
    provider: Arc<dyn SubagentProvider>,
    providers: Arc<Mutex<HashMap<String, Arc<dyn SubagentProvider>>>>,
    ctx: Context,
    removed: bool,
}

impl ProviderRegistration {
    pub fn unregister(mut self) {
        self.remove();
    }

    fn remove(&mut self) {
        if self.removed {
            return;
        }
        self.removed = true;
        let mut providers = lock(&self.providers);
        if providers
            .get(&self.name)
            .is_some_and(|current| Arc::ptr_eq(current, &self.provider))
        {
            providers.remove(&self.name);
            drop(providers);
            emit_provider_removed(&self.ctx, self.name.clone());
        }
    }
}

impl Drop for ProviderRegistration {
    fn drop(&mut self) {
        self.remove();
    }
}

impl SubagentRuntime {
    pub fn new(ctx: Context) -> Self {
        Self::with_config(ctx, Arc::new(|| 8), Arc::new(|| 1))
    }

    fn with_config(
        ctx: Context,
        max_active: Arc<dyn Fn() -> usize + Send + Sync>,
        max_depth: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        let manager_capacity = max_active.clone();
        Self {
            manager: SubagentManager::new(ctx.clone(), move || manager_capacity()),
            ctx,
            providers: Arc::default(),
            max_depth,
        }
    }

    pub fn resolve_max_depth(
        &self,
        configured: Option<ConfiguredMaxDepth>,
    ) -> Result<Option<u64>, BoxError> {
        if configured == Some(ConfiguredMaxDepth::ProviderManaged) {
            return Ok(None);
        }
        let value = match configured {
            Some(ConfiguredMaxDepth::Value(value)) => value,
            Some(ConfiguredMaxDepth::ProviderManaged) => unreachable!("handled above"),
            None => (self.max_depth)(),
        };
        crate::depth::assert_subagent_max_depth(Some(value as f64))?;
        Ok(Some(value))
    }

    pub fn register_provider(
        &self,
        provider: Arc<dyn SubagentProvider>,
    ) -> Result<ProviderRegistration, BoxError> {
        let name = provider.name().to_owned();
        if !provider.supports_start() && !provider.supports_continuable() {
            return Err(Box::new(SubagentError::error(
                format!("subagent provider \"{name}\" must implement start or prepareContinuable"),
                SubagentErrorCode::UnsupportedCapability,
            )));
        }
        let mut providers = lock(&self.providers);
        if providers.contains_key(&name) {
            return Err(Box::new(SubagentError::error(
                format!("a subagent provider named \"{name}\" is already registered"),
                SubagentErrorCode::DuplicateProvider,
            )));
        }
        providers.insert(name.clone(), provider.clone());
        drop(providers);
        if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.ctx.emit(&SubagentProviderAdded(provider.clone()));
        })) {
            let mut providers = lock(&self.providers);
            if providers
                .get(&name)
                .is_some_and(|current| Arc::ptr_eq(current, &provider))
            {
                providers.remove(&name);
            }
            drop(providers);
            emit_provider_removed(&self.ctx, name);
            std::panic::resume_unwind(panic);
        }
        Ok(ProviderRegistration {
            name,
            provider,
            providers: self.providers.clone(),
            ctx: self.ctx.clone(),
            removed: false,
        })
    }

    pub fn get_provider(&self, name: &str) -> Option<Arc<dyn SubagentProvider>> {
        lock(&self.providers).get(name).cloned()
    }

    pub async fn start_activation(
        &self,
        spec: SubagentActivationSpec,
    ) -> Result<SubagentActivation, BoxError> {
        let provider = self.expect_provider(&spec.provider)?;
        if spec.child_id.is_some() && !provider.supports_continuable() {
            return Err(Box::new(SubagentError::error(
                "reserved child ids require a local backend",
                SubagentErrorCode::UnsupportedCapability,
            )));
        }
        crate::depth::assert_subagent_max_depth(spec.request.max_depth.map(|value| value as f64))?;
        self.assert_capabilities(provider.as_ref(), &spec)?;
        if let Some(schema) = &spec.request.output_schema {
            assert_object_json_schema(schema)?;
        }
        if provider.supports_continuable() {
            let cwd = self.resolve_cwd(&spec).await?;
            let runtime = self.clone();
            let checked = provider.clone();
            return self
                .manager
                .start_local(
                    spec,
                    provider,
                    cwd,
                    Arc::new(move || runtime.assert_provider_current(&checked)),
                )
                .await;
        }
        if spec.delivery == SubagentDeliveryTarget::Caller && spec.name.is_some() {
            return Err(Box::new(SubagentError::error(
                "agent names require a cataloged child; caller-delivered external runs are not cataloged",
                SubagentErrorCode::UnsupportedCapability,
            )));
        }
        let root = self.root_of(&spec.request.parent)?;
        let cwd = self.resolve_cwd(&spec).await?;
        let mut reservation = if spec.delivery == SubagentDeliveryTarget::Parent {
            Some(
                self.manager
                    .reserve_child_name(&spec.request.parent, spec.name.as_deref())?,
            )
        } else {
            None
        };
        let root_key = match self.manager.reserve_external_capacity(&root) {
            Ok(root_key) => root_key,
            Err(error) => {
                if let Some(reservation) = &mut reservation {
                    reservation.release();
                }
                return Err(error);
            }
        };
        let request = ResolvedSubagentStartRequest {
            cwd,
            agent_preset: spec.request.agent_preset.clone(),
            label: Some(spec.label.clone()),
            prompt: spec.request.prompt.clone(),
            parent: spec.request.parent.clone(),
            signal: spec.request.signal.clone(),
            agent_options: spec.request.agent_options.clone(),
            output_schema: spec.request.output_schema.clone(),
            max_depth: spec.request.max_depth,
        };
        if let Err(error) = self.assert_provider_current(&provider) {
            if let Some(reservation) = &mut reservation {
                reservation.release();
            }
            self.manager.release_external_capacity(root_key);
            return Err(error);
        }
        let Some(start) = provider.start(request) else {
            if let Some(reservation) = &mut reservation {
                reservation.release();
            }
            self.manager.release_external_capacity(root_key);
            return Err(Box::new(SubagentError::error(
                format!(
                    "subagent provider \"{}\" must implement start or prepareContinuable",
                    provider.name()
                ),
                SubagentErrorCode::UnsupportedCapability,
            )));
        };
        let run = match start.await {
            Ok(run) => run,
            Err(error) => {
                if let Some(reservation) = &mut reservation {
                    reservation.release();
                }
                self.manager.release_external_capacity(root_key);
                return Err(error);
            }
        };
        let child_id = run.id().clone();
        if let Err(error) =
            self.manager
                .register_external(&spec.request.parent, &root, &child_id, run.clone())
        {
            if let Some(reservation) = &mut reservation {
                reservation.release();
            }
            let _ = run.dispose().await;
            self.manager.release_external_capacity(root_key);
            return Err(error);
        }
        let catalog_name = reservation.as_ref().map_or_else(
            || child_id.to_string(),
            |reservation| reservation.name().to_owned(),
        );
        let path = crate::names::join_agent_path(
            self.manager
                .path_for_member(&spec.request.parent, &root)
                .as_deref(),
            reservation.as_ref().map(|reservation| reservation.name()),
        );
        let name = crate::names::agent_address(path.as_deref(), child_id.as_str());
        let run_info = SubagentRunInfo {
            run_id: SubagentRunId::from(hoh_util_crypto::random_uuid().as_str()),
            provider: spec.provider.clone(),
            parent_session_id: spec.request.parent.id().clone(),
            id: child_id.clone(),
            local: false,
        };
        emit_subagent_start(root.ctx(), run_info.clone());
        let admission: Result<(), BoxError> = spec
            .request
            .signal
            .throw_if_aborted()
            .map_err(|error| Box::new(error) as BoxError)
            .and_then(|()| {
                let agents = self.ctx.get::<hoh_agent::Agents>().ok_or_else(|| {
                    Box::new(SubagentError::error(
                        "subagent creation requires the agents service",
                        SubagentErrorCode::ContinuationUnavailable,
                    )) as BoxError
                })?;
                if agents
                    .get(spec.request.parent.id())
                    .is_some_and(|live| Arc::ptr_eq(&live, &spec.request.parent))
                {
                    Ok(())
                } else {
                    Err(Box::new(SubagentError::error(
                        "subagent creation requires the exact live parent agent",
                        SubagentErrorCode::Unauthorized,
                    )) as BoxError)
                }
            })
            .and_then(|()| {
                if reservation.is_some() {
                    establish_external_catalog_child(
                        spec.request.parent.session(),
                        &child_id,
                        &spec.label,
                        &catalog_name,
                    )
                } else {
                    Ok(())
                }
            });
        if let Err(error) = admission {
            if let Some(reservation) = &mut reservation {
                reservation.release();
            }
            let _ = run.dispose().await;
            emit_subagent_end(
                root.ctx(),
                SubagentRunEndInfo::new(run_info, SubagentStopReason::Error, None),
            );
            self.manager.unregister_external(&child_id, &run);
            self.manager.release_external_capacity(root_key);
            return Err(error);
        }
        if let Some(reservation) = &mut reservation {
            reservation.release();
        }

        let (send, receive) = oneshot::channel();
        let result_run = run.clone();
        let lifecycle_ctx = root.ctx().clone();
        let lifecycle = run_info;
        let parent = spec.request.parent.clone();
        let delivery = spec.delivery;
        let result_name = name.clone();
        let result_child_id = child_id.clone();
        let manager = self.manager.clone();
        let registered_run = run.clone();
        let settling = self.manager.begin_settling(&root);
        tokio::spawn(async move {
            let _settling = settling;
            let result = result_run.result().await.map_err(SharedError::new);
            let teardown_failed = if let Err(error) = result_run.dispose().await {
                lifecycle_ctx.logger().warn(format_args!(
                    "subagent \"{result_child_id}\" activation teardown failed: {error}"
                ));
                true
            } else {
                false
            };
            manager.unregister_external(&result_child_id, &registered_run);
            manager.release_external_capacity(root_key);
            match &result {
                Ok(result) => {
                    if delivery == SubagentDeliveryTarget::Parent {
                        let _ = parent.followup(
                            crate::continuation_messages::create_settlement_message(
                                &result_child_id,
                                &result_name,
                                result,
                                false,
                                false,
                            ),
                        );
                    }
                    emit_subagent_end(
                        &lifecycle_ctx,
                        SubagentRunEndInfo::new(
                            lifecycle,
                            if teardown_failed {
                                SubagentStopReason::Error
                            } else {
                                result.stop_reason.clone()
                            },
                            (!teardown_failed && !result.output.is_empty())
                                .then(|| result.output.clone()),
                        ),
                    );
                }
                Err(_) => emit_subagent_end(
                    &lifecycle_ctx,
                    SubagentRunEndInfo::new(lifecycle, SubagentStopReason::Error, None),
                ),
            }
            let _ = send.send(result);
        });
        let dispose_run = run.clone();
        let dispose = Arc::new(move || dispose_run.dispose());
        let result = Box::pin(async move {
            match receive.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(error)) => Err(Box::new(error) as BoxError),
                Err(_) => Err("subagent result task ended before settlement".into()),
            }
        });
        Ok(SubagentActivation {
            child_id,
            name,
            message_id: None,
            result,
            dispose,
        })
    }

    pub fn root_of(
        &self,
        caller: &Arc<dyn hoh_agent::Agent>,
    ) -> Result<Arc<dyn hoh_agent::Agent>, BoxError> {
        self.manager.root_of(caller)
    }

    pub fn names_of(
        &self,
        agent: &Arc<dyn hoh_agent::Agent>,
    ) -> Result<SubagentMemberNames, BoxError> {
        self.manager.names_of(agent)
    }

    pub async fn resolve_name(
        &self,
        caller: &Arc<dyn hoh_agent::Agent>,
        name: &str,
        signal: hoh_timeout::AbortSignal,
    ) -> Result<SessionId, BoxError> {
        let root = self.root_of(caller)?;
        if name == crate::names::ROOT_AGENT_NAME {
            return Ok(root.id().clone());
        }
        if let Some(id) = resolve_agent_path(&self.ctx, root.id(), name, signal).await? {
            return Ok(id);
        }
        if name.contains('/') {
            return Err(Box::new(SubagentError::error(
                format!(
                    "agent name {} was not found in this subagent tree; use list_agents to see the names",
                    hoh_util_values::js_json_stringify(&serde_json::Value::String(name.into()))
                ),
                SubagentErrorCode::AgentNameNotFound,
            )));
        }
        Ok(name.into())
    }

    pub async fn send_message(
        &self,
        sender: &Arc<dyn hoh_agent::Agent>,
        target_id: &SessionId,
        content: Vec<hoh_llm::ContentBlock>,
        options: SubagentSendMessageOptions,
    ) -> Result<hoh_llm::MessageId, BoxError> {
        self.manager
            .send_message(sender, target_id, content, options)
            .await
    }

    pub fn interrupt(
        &self,
        target_id: &SessionId,
        authority: SubagentInterruptAuthority,
    ) -> Result<(), BoxError> {
        self.manager.interrupt(target_id, authority)
    }

    pub async fn drain_descendants(
        &self,
        roots: &[Arc<dyn hoh_agent::Agent>],
    ) -> Result<(), BoxError> {
        self.manager.drain_descendants(roots).await
    }

    pub async fn wait_for_tree_work(
        &self,
        root: &Arc<dyn hoh_agent::Agent>,
    ) -> Result<bool, BoxError> {
        self.manager.wait_for_tree_work(root).await
    }

    pub async fn list_descendants(
        &self,
        root_session_id: &SessionId,
        signal: Option<hoh_timeout::AbortSignal>,
    ) -> Result<Vec<crate::control_types::SubagentDescendantListEntry>, BoxError> {
        list_descendants(&self.ctx, root_session_id, signal).await
    }

    pub async fn prompt(
        &self,
        request: crate::control_types::SubagentPromptRequest,
        signal: hoh_timeout::AbortSignal,
    ) -> Result<crate::control_types::SubagentPromptReceipt, HarnessError> {
        let payload = serde_json::to_value(&request).map_err(|error| {
            HarnessError::named("RemoteError", error.to_string(), "gateway/bad-request")
        })?;
        crate::control::validate_control_request("subagent.prompt", &payload)?;
        let canonical_time_zone = request
            .client_time_zone
            .as_deref()
            .and_then(hoh_util_time::canonical_client_time_zone);
        if let Some(value) = request.client_time_zone.as_deref()
            && canonical_time_zone.is_none()
        {
            return Err(HarnessError::named(
                "RemoteError",
                "clientTimeZone must be UTC or a valid IANA Area/Location name",
                "subagent/invalid-time-zone",
            )
            .with_detail(serde_json::json!({ "value": value })));
        }
        let admitted = if request
            .content
            .iter()
            .all(|part| matches!(part, PromptContentPart::Text { .. }))
        {
            request
                .content
                .iter()
                .map(|part| match part {
                    PromptContentPart::Text { text } => ContentBlock::text(text.clone()),
                    PromptContentPart::Image { .. } => unreachable!("text-only branch"),
                })
                .collect()
        } else {
            let store = self.ctx.get::<Attachments>().ok_or_else(|| {
                HarnessError::named("RemoteError", "subagent prompt failed", "gateway/internal")
            })?;
            let parts = request
                .content
                .iter()
                .map(|part| match part {
                    PromptContentPart::Text { text } => {
                        AttachmentAdmissionPart::Text { text: text.clone() }
                    }
                    PromptContentPart::Image {
                        media_type,
                        data,
                        name,
                    } => AttachmentAdmissionPart::EncodedImage {
                        media_type: *media_type,
                        data: data.clone(),
                        name: name.clone(),
                    },
                })
                .collect::<Vec<_>>();
            store
                .admit_prompt_content(&parts)
                .await
                .map_err(|error| {
                    crate::control::reject_prompt(error, &request.child_session_id, &signal)
                })?
                .into_iter()
                .map(|part| match part {
                    AdmittedPromptContentPart::Text { text } => ContentBlock::text(text),
                    AdmittedPromptContentPart::Image { attachment } => hoh_llm::ImageBlock {
                        attachment,
                        offloaded: None,
                    }
                    .into(),
                    AdmittedPromptContentPart::File { attachment } => {
                        hoh_llm::FileBlock { attachment }.into()
                    }
                })
                .collect()
        };
        let mut source = hoh_llm::MessageSource::new("user");
        source
            .data
            .insert("rpcId".into(), request.request_id.as_str().into());
        if let Some(zone) = canonical_time_zone {
            source.data.insert("clientTimeZone".into(), zone.into());
        }
        let message_id = self
            .manager
            .deliver_prompt(
                &request.child_session_id,
                admitted,
                source,
                &signal,
                request.delivery,
            )
            .await
            .map_err(|error| {
                crate::control::reject_prompt(error, &request.child_session_id, &signal)
            })?;
        Ok(crate::control_types::SubagentPromptReceipt { message_id })
    }

    pub fn interrupt_by_user(
        &self,
        child_session_id: &SessionId,
    ) -> Result<crate::control_types::SubagentInterruptReceipt, HarnessError> {
        crate::control::validate_control_request(
            "subagent.interrupt",
            &serde_json::json!({ "childSessionId": child_session_id }),
        )?;
        self.interrupt(child_session_id, SubagentInterruptAuthority::User)
            .map_err(|error| {
                HarnessError::named(
                    "RemoteError",
                    "subagent interrupt failed",
                    "gateway/internal",
                )
                .with_cause(error)
            })?;
        Ok(crate::control_types::SubagentInterruptReceipt { accepted: true })
    }

    fn expect_provider(&self, name: &str) -> Result<Arc<dyn SubagentProvider>, BoxError> {
        self.get_provider(name).ok_or_else(|| {
            Box::new(SubagentError::error(
                format!("no subagent provider registered for \"{name}\""),
                SubagentErrorCode::NoProvider,
            )) as BoxError
        })
    }

    fn assert_provider_current(
        &self,
        provider: &Arc<dyn SubagentProvider>,
    ) -> Result<(), BoxError> {
        if !self
            .get_provider(provider.name())
            .is_some_and(|current| Arc::ptr_eq(&current, provider))
        {
            return Err(Box::new(SubagentError::error(
                format!(
                    "subagent provider \"{}\" changed during startup; retry the delegation",
                    provider.name()
                ),
                SubagentErrorCode::ProviderChanged,
            )));
        }
        Ok(())
    }

    fn assert_capabilities(
        &self,
        provider: &dyn SubagentProvider,
        spec: &SubagentActivationSpec,
    ) -> Result<(), BoxError> {
        let capabilities = provider.capabilities();
        if spec.request.agent_preset.is_some()
            && (!capabilities.agent_preset || provider.inherits_parent_context())
        {
            return Err(unsupported(
                provider.name(),
                "independent agent preset selection",
            ));
        }
        for (needed, supported, name) in [
            (
                spec.request.agent_options.is_some(),
                capabilities.agent_options,
                "agentOptions",
            ),
            (
                spec.request.output_schema.is_some(),
                capabilities.output_schema,
                "outputSchema",
            ),
            (
                spec.request.max_depth.is_some(),
                capabilities.depth_limit,
                "depthLimit",
            ),
        ] {
            if needed && !supported {
                return Err(unsupported(
                    provider.name(),
                    &format!("\"{name}\" capability"),
                ));
            }
        }
        Ok(())
    }

    async fn resolve_cwd(&self, spec: &SubagentActivationSpec) -> Result<String, BoxError> {
        let service = self
            .ctx
            .get::<WorkingDirectoryKey>()
            .ok_or_else(|| -> BoxError {
                Box::new(SubagentError::error(
                    "subagent creation requires the workingDirectory service",
                    SubagentErrorCode::WorkingDirectoryUnavailable,
                ))
            })?;
        let base = service
            .ensure(&spec.request.parent, Some(&spec.request.signal))
            .await?;
        let Some(requested) = &spec.request.cwd else {
            return Ok(base);
        };
        if is_absolute(requested) {
            Ok(requested.clone())
        } else {
            Ok(resolve(&base, &[requested]))
        }
    }
}

fn unsupported(provider: &str, capability: &str) -> BoxError {
    Box::new(SubagentError::error(
        if capability == "independent agent preset selection" {
            format!(
                "subagent provider \"{provider}\" does not support independent agent preset selection"
            )
        } else {
            format!("subagent provider \"{provider}\" does not support the {capability}")
        },
        SubagentErrorCode::UnsupportedCapability,
    ))
}

#[allow(dead_code)]
fn _capability_shape(_: SubagentCapabilities, _: SessionId) {}
