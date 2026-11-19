//! One-shot user approval, fail-closed answerers, and durable session policy.
mod host;
mod method;
mod shim;
mod state;
pub mod types;
pub use host::HostApprovalPeer;
pub use method::method_human_text;
pub use types::{ApprovalOutcome, ApprovalRequestEvent, ApprovalRequestId, DisplayReason};
pub type ApprovalRequest = ApprovalRequestEvent;

use futures_util::future::BoxFuture;
use hoh_agent::{Agent, AssembleAgent, agent_carrier};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};
use hoh_session::Session;
use hoh_session_projection::{ProjectionHandle, SessionProjections};
use hoh_system_prompt::{
    PromptContext, PromptContextOrderName, PromptText, SystemPrompt, SystemPromptKey,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

const ASK_SENTENCE: &str = "Approval prompts are enabled in this session: operations that require approval may ask through the configured answerers; without an available answerer, the request fails closed.";
const NEVER_SENTENCE: &str = "Approval prompts are disabled in this session: actions that require approval are rejected automatically — do not request sandbox escalation (do not set `sandbox_permissions`).";
const OUTSIDE_TURN: &str = "approval.request() outside an open turn: the approval/asked + approval/decided audit pair must be turn-enclosed (a bare event between turns is crash-tail garbage on reload). Ask from inside the turn that needs the decision.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalPolicy {
    #[default]
    Ask,
    Never,
}
impl ApprovalPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Never => "never",
        }
    }
    fn notice_name(self) -> &'static str {
        match self {
            Self::Ask => "ask the user for approval",
            Self::Never => "never ask the user for approval",
        }
    }
}
pub const APPROVAL_POLICIES: [ApprovalPolicy; 2] = [ApprovalPolicy::Ask, ApprovalPolicy::Never];
impl TryFrom<&str> for ApprovalPolicy {
    type Error = ApprovalError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "ask" => Ok(Self::Ask),
            "never" => Ok(Self::Never),
            _ => Err(ApprovalError::InvalidPolicy),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    /// The deployment's default {@link ApprovalPolicy} for sessions without an
    /// `approval/policy` override — `'ask'` delegates to the composed answerers
    /// (fail-closed with none); `'never'` auto-rejects every ask without
    /// prompting (the deterministic CI/unattended stance).
    #[serde(deserialize_with = "shim::default_null_policy")]
    #[schemars(with = "Option<ApprovalPolicy>")]
    pub policy: ApprovalPolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("{OUTSIDE_TURN}")]
    OutsideTurn,
    #[error("approval policy must be one of \"ask\" or \"never\"")]
    InvalidPolicy,
    #[error("{0}")]
    InvalidState(String),
    #[error("host approval mode is already configured")]
    HostAlreadyConfigured,
}
impl ApprovalError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::OutsideTurn => "Error",
            Self::InvalidPolicy => "TypeError",
            Self::InvalidState(_) => "ZodError",
            Self::HostAlreadyConfigured => "Error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalPolicyEvent {
    pub policy: ApprovalPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ApprovalPolicySource>,
}
impl ApprovalPolicyEvent {
    pub const TYPE: &'static str = "approval/policy";
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalPolicySource {
    Delegation,
}

pub fn set_approval_policy(session: &Session, policy: ApprovalPolicy) -> Result<(), BoxError> {
    session.append(
        ApprovalPolicyEvent::TYPE,
        serde_json::to_value(ApprovalPolicyEvent {
            policy,
            source: None,
        })?,
        None,
    )?;
    Ok(())
}

#[cfg(test)]
type PolicyReadHook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct ApprovalService {
    ctx: Context,
    config: Arc<Mutex<Config>>,
    approval_projection_handle: ProjectionHandle<state::ApprovalState>,
    host: Arc<Mutex<Option<host::HostApprovalMode>>>,
    /// Sandbox escalations the user already allowed through another question (see
    /// [`ApprovalService::pre_approve_escalation`]). Shared by every incarnation of the service.
    escalation_grants: Arc<Mutex<HashMap<(String, String), EscalationGrant>>>,
    /// `set_policy` reads the effective policy, appends and injects as one step (the source's
    /// method has no `await` between them, so concurrent switches never interleave). One lock for
    /// every incarnation of the service: an old `Service<Approval>` that outlives a replacement
    /// still switches under the same lock as the new one.
    policy_switch: Arc<Mutex<()>>,
    /// Test seam: runs between reading the previous policy and writing the new one.
    #[cfg(test)]
    after_policy_read: Arc<Mutex<Option<PolicyReadHook>>>,
}
hoh_kernel::service!(pub Approval: ApprovalService = "approval");

/// The state of one kernel that must be the same for every incarnation of the service: host mode
/// (enabled once, in `initialize`), the policy-switch lock and the escalation grants. The service is
/// rebuilt whenever a dependency (the projection registry) leaves and returns, and an old
/// `Service<Approval>` may outlive its replacement, so the slot lives on the kernel root.
#[derive(Clone, Default)]
pub(crate) struct HostSlot {
    host: Arc<Mutex<Option<host::HostApprovalMode>>>,
    policy_switch: Arc<Mutex<()>>,
    escalation_grants: Arc<Mutex<HashMap<(String, String), EscalationGrant>>>,
}
hoh_kernel::service!(HostSlotKey: HostSlot = "approvalHostSlot");

/// How long a grant waits for the escalation request it was made for.
const ESCALATION_GRANT_LIFETIME: Duration = Duration::from_secs(300);

/// One allowed escalation: for one tool and for exactly the reason that tool will ask with.
struct EscalationGrant {
    tool_name: String,
    reason: String,
    made: Instant,
}

async fn host_slot(ctx: &Context) -> Result<HostSlot, BoxError> {
    if ctx.get::<HostSlotKey>().is_none() {
        // The root owns the provision, so it outlives any one fiber of this plugin.
        ctx.root()
            .provide::<HostSlotKey>(Arc::new(HostSlot::default()))
            .await?;
    }
    let slot = ctx
        .get::<HostSlotKey>()
        .ok_or("approval host slot is unavailable")?;
    let slot: &HostSlot = &slot;
    Ok(slot.clone())
}

impl ApprovalService {
    /// The source's mutable config property, shared by all service references.
    /// Release this synchronous borrow before awaiting or calling service methods.
    pub fn config(&self) -> MutexGuard<'_, Config> {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// GLUE-SHIM(2b): the JSON-RPC server calls this once from a successful host-mode
    /// `initialize`. The peer keeps this crate independent from the server transport.
    pub fn install_host_mode(
        &self,
        approval_key: &str,
        peer: Arc<dyn HostApprovalPeer>,
    ) -> Result<(), BoxError> {
        let key = hoh_protocol::approval::key_from_hex(approval_key)?;
        let mut slot = self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return Err(ApprovalError::HostAlreadyConfigured.into());
        }
        *slot = Some(host::HostApprovalMode::new(key, peer));
        Ok(())
    }
    /// Decide one privileged JSON-RPC method call (`hoh_protocol::PRIVILEGED_METHODS`) in host
    /// mode: the same `approval/request` with a verified mac as for tools. `Ok(())` only when the
    /// user allowed exactly this call; without host mode, or on any other outcome, an error.
    /// GLUE-SHIM(2b): the server's `HohPrivilegedApprover` service wraps this method.
    pub fn approve_privileged(
        &self,
        method: &str,
        params: &Value,
    ) -> futures_util::future::BoxFuture<'static, Result<(), BoxError>> {
        let host = self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let context = self.ctx.clone();
        let (method, params) = (method.to_owned(), params.clone());
        Box::pin(async move {
            let Some(host) = host else {
                return Err(format!("{method} requires host approval mode").into());
            };
            host.approve_method(&context, &method, &params, request_id()?.to_string())
                .await
        })
    }
    /// HoH window profile (no dsh counterpart): the user has just allowed (`allowed-once`) a
    /// question that covers the sandbox escalation `tool_name` is about to ask for in `call_id`
    /// (the destructive guard words it in Russian). The next request of that very call, from that
    /// very tool, with exactly `reason` (the tool's escalation text) resolves `allowed-once`
    /// without asking again; it is still audited (`approval/asked` + `approval/decided`).
    ///
    /// Call it only after the host's decision: a rejected, cancelled or expired question makes no
    /// grant. A grant is bound to (session, call id, tool name, reason), is used once, expires after
    /// five minutes, never overrides `policy: never` or an aborted call, and is not spent by a
    /// request of another tool or reason — a call id may be reused between steps.
    pub fn pre_approve_escalation(
        &self,
        session: &Session,
        call_id: &hoh_llm::ToolCallId,
        tool_name: &str,
        reason: &str,
    ) {
        let mut grants = self
            .escalation_grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        grants.retain(|_, grant| grant.made.elapsed() < ESCALATION_GRANT_LIFETIME);
        if grants.len() >= 256 {
            grants.clear();
        }
        grants.insert(
            (session.id().to_string(), call_id.to_string()),
            EscalationGrant {
                tool_name: tool_name.to_owned(),
                reason: reason.to_owned(),
                made: Instant::now(),
            },
        );
    }
    pub fn host_mode_enabled(&self) -> bool {
        self.host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
    async fn initialize(
        ctx: Context,
        config: Config,
        registration: impl Future<Output = Result<ProjectionHandle<state::ApprovalState>, BoxError>>,
    ) -> Result<Option<Self>, BoxError> {
        let handle = registration.await?;
        // Source: registration may finish after unload starts; add no effects then.
        if ctx.ensure_active().is_err() {
            return Ok(None);
        }
        let slot = host_slot(&ctx).await?;
        let service = Self {
            ctx: ctx.clone(),
            config: Arc::new(Mutex::new(config)),
            approval_projection_handle: handle,
            host: slot.host,
            escalation_grants: slot.escalation_grants,
            policy_switch: slot.policy_switch,
            #[cfg(test)]
            after_policy_read: Arc::default(),
        };
        let for_prompt = service.clone();
        let prompt = ctx.inject::<PromptDeps, _>(move |scope, deps| {
            let service = for_prompt.clone();
            async move {
                deps.prompt.context(
                    &scope,
                    PromptContext {
                        name: "approval:policy".into(),
                        order: SystemPrompt::get_context_order(
                            PromptContextOrderName::ApprovalPolicy,
                        ),
                        text: PromptText::provider(move |context| {
                            let Some(agent) = context.extension::<AssembleAgent>() else {
                                return Ok(String::new());
                            };
                            Ok(match service.effective_policy(agent.0.session())? {
                                ApprovalPolicy::Ask => ASK_SENTENCE,
                                ApprovalPolicy::Never => NEVER_SENTENCE,
                            }
                            .into())
                        }),
                        ..Default::default()
                    },
                )?;
                Ok(())
            }
        })?;
        // A missing optional prompt leaves its child pending; do not wait on it.
        if ctx.has("systemPrompt") {
            prompt.settled().await?;
        }
        Ok(Some(service))
    }
    pub fn set_policy(&self, agent: &dyn Agent, policy: ApprovalPolicy) -> Result<(), BoxError> {
        // Check and change under one lock: of two simultaneous switches to the same policy the
        // second sees the first one's result and does nothing (one event, one notice).
        let _switching = self
            .policy_switch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = self.effective_policy(agent.session())?;
        #[cfg(test)]
        {
            let hook = self
                .after_policy_read
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hook) = hook {
                hook();
            }
        }
        if previous == policy {
            return Ok(());
        }
        set_approval_policy(agent.session(), policy)?;
        agent.inject(create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(format!(
                "The user changed the approval policy from \"{}\" to \"{}\".",
                previous.notice_name(),
                policy.notice_name()
            ))],
            source: MessageSource::new("user-approval"),
        }))?;
        Ok(())
    }
    pub fn override_of(&self, session: &Session) -> Result<Option<ApprovalPolicy>, BoxError> {
        Ok(self.approval_projection_handle.state_of(session)?.policy)
    }
    fn effective_policy(&self, session: &Session) -> Result<ApprovalPolicy, BoxError> {
        if let Some(policy) = self.override_of(session)? {
            return Ok(policy);
        }
        Ok(self.config().policy)
    }
    /// Test seam: make every grant older by `by`.
    #[cfg(test)]
    fn age_escalation_grants(&self, by: Duration) {
        for grant in self
            .escalation_grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values_mut()
        {
            grant.made -= by;
        }
    }
    fn take_escalation_grant(&self, session: &Session, req: &ApprovalRequest) -> bool {
        let (Some(call_id), Some(reason)) = (&req.call_id, &req.reason) else {
            return false;
        };
        let key = (session.id().to_string(), call_id.to_string());
        let mut grants = self
            .escalation_grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(grant) = grants.get(&key) else {
            return false;
        };
        if grant.made.elapsed() >= ESCALATION_GRANT_LIFETIME {
            grants.remove(&key);
            return false;
        }
        if grant.tool_name != req.tool_name || grant.reason != *reason {
            return false; // not this request: the grant stays for the one it was made for
        }
        grants.remove(&key);
        true
    }
    /// Starts immediately and continues if its returned future is dropped, like
    /// the source Promise. Audit failures propagate without replacing their error.
    pub fn request(
        &self,
        req: impl Into<Arc<ApprovalRequest>>,
    ) -> BoxFuture<'static, Result<ApprovalOutcome, BoxError>> {
        let req = req.into();
        let session = req.agent.session().clone();
        let prepare = (|| -> Result<_, BoxError> {
            if !self
                .approval_projection_handle
                .state_of(&session)?
                .turn_open
            {
                return Err(ApprovalError::OutsideTurn.into());
            }
            let id = request_id()?;
            session.append(
                types::ApprovalAsked::TYPE,
                serde_json::to_value(types::ApprovalAsked {
                    id: id.clone(),
                    tool_name: req.tool_name.clone(),
                    call_id: req.call_id.clone(),
                    reason: req.reason.clone(),
                })?,
                None,
            )?;
            let decision = if req.signal.as_ref().is_some_and(|signal| signal.aborted()) {
                Some(ApprovalOutcome::Cancelled)
            } else if self.effective_policy(&session)? == ApprovalPolicy::Never {
                Some(ApprovalOutcome::Rejected)
            } else if self.take_escalation_grant(&session, &req) {
                Some(ApprovalOutcome::AllowedOnce)
            } else {
                None
            };
            Ok((id, decision))
        })();
        let (id, immediate) = match prepare {
            Ok(value) => value,
            Err(error) => return Box::pin(async { Err(error) }),
        };
        let host = self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let context = self.ctx.clone();
        // No context timer/task in source: this is continuation of an already
        // started public Promise, rather than a background plugin effect.
        let task = tokio::spawn(async move {
            let outcome = match immediate {
                Some(outcome) => outcome,
                None => {
                    if let Some(host) = host {
                        host.resolve(&context, req.clone(), id.as_str()).await
                    } else {
                        let (started, answer) = agent_carrier(&req.agent)
                            .context(&context)
                            .waterfall_with_start::<ApprovalRequestEvent, Value, _>(
                            req.clone(),
                            |_| async { Ok(ApprovalOutcome::Unavailable.into()) },
                        );
                        // TS schedules the answerer before the cancelled request's
                        // audit continuation. Await startup, never the pending answer.
                        started.await;
                        let answer = async move {
                            answer
                                .await
                                .map(ApprovalOutcome::normalize)
                                .unwrap_or(ApprovalOutcome::Unavailable)
                        };
                        match &req.signal {
                            None => answer.await,
                            Some(signal) => {
                                tokio::select! {
                                    biased;
                                    _ = signal.cancelled() => ApprovalOutcome::Cancelled,
                                    outcome = answer => outcome,
                                }
                            }
                        }
                    }
                }
            };
            session.append(
                types::ApprovalDecided::TYPE,
                serde_json::to_value(types::ApprovalDecided { id, outcome })?,
                None,
            )?;
            Ok(outcome)
        });
        Box::pin(async move { task.await.map_err(|error| Box::new(error) as BoxError)? })
    }
}
fn request_id() -> Result<ApprovalRequestId, BoxError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut id = String::with_capacity(36);
    use std::fmt::Write;
    for (index, byte) in bytes.into_iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        // Writing to String is infallible.
        write!(id, "{byte:02x}").expect("String formatting cannot fail");
    }
    Ok(id.into())
}

#[derive(Inject)]
pub struct ApprovalDeps {
    projections: Service<SessionProjections>,
}
#[derive(Inject)]
struct PromptDeps {
    prompt: Service<SystemPromptKey>,
}
impl Plugin for ApprovalService {
    const NAME: &'static str = "ApprovalService";
    type Config = Config;
    type Deps = ApprovalDeps;
    async fn apply(ctx: Context, deps: ApprovalDeps, config: Cfg<Config>) -> Result<(), BoxError> {
        let registration = deps.projections.register(&ctx, state::definition());
        if let Some(service) =
            Self::initialize(ctx.clone(), (*config.get()).clone(), registration).await?
        {
            ctx.provide::<Approval>(Arc::new(service)).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/approval.rs"]
mod approval_tests;
#[cfg(test)]
#[path = "../tests/host.rs"]
mod host_tests;
#[cfg(test)]
#[path = "../tests/projection.rs"]
mod projection_tests;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_support;
