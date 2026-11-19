//! Private seams for optional packages not available in the current workspace.
//! This module implements no command, goal, scheduling, or subagent runtime.
use crate::command_helpers::CommandDescriptor;
use futures_util::future::BoxFuture;
use hoh_agent::Agent;
use hoh_attachment::EncodedImageAttachment;
use hoh_error::BoxError;
use hoh_kernel::{Contained, Context, Fiber, Waterfall};
use hoh_protocol::CommandExecution;
use hoh_session::SessionId;
use hoh_timeout::AbortSignal;
use std::sync::Arc;

/// The only `CommandSubmitAttachment` variant used by the SDK server.
///
/// SHIM(hoh-commands): the source command package accepts this native shape
/// (`mediaType`, not the SDK wire shape's `mimeType`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandImageAttachment {
    pub image: EncodedImageAttachment,
}

/// SHIM(hoh-commands): list/execute only; replace with the native commands service when ported.
pub trait CommandsApi: Send + Sync + 'static {
    fn list(&self, agent: &Arc<dyn Agent>) -> Result<Vec<CommandDescriptor>, BoxError>;
    fn execute(
        &self,
        agent: Arc<dyn Agent>,
        line: String,
        images: Vec<CommandImageAttachment>,
        signal: AbortSignal,
    ) -> BoxFuture<'static, Result<Option<CommandExecution>, BoxError>>;
}
hoh_kernel::service!(pub Commands: dyn CommandsApi = "commands");

/// SHIM(hoh-subagent): the three existing source service methods used by this server.
pub trait SubagentsApi: Send + Sync + 'static {
    fn root_of(&self, agent: &Arc<dyn Agent>) -> Result<Arc<dyn Agent>, BoxError>;
    fn wait_for_tree_work(
        &self,
        agent: Arc<dyn Agent>,
    ) -> BoxFuture<'static, Result<bool, BoxError>>;
    fn drain_descendants(
        &self,
        roots: Vec<Arc<dyn Agent>>,
    ) -> BoxFuture<'static, Result<(), BoxError>>;
}
hoh_kernel::service!(pub Subagents: dyn SubagentsApi = "subagents");
/// SHIM(hoh-subagent): the snapshotted lifecycle event, never infer locality from IDs/lineage.
pub struct SubagentRunEndInfo {
    pub provider: String,
    pub id: SessionId,
    pub parent_session_id: SessionId,
    pub local: bool,
    pub stop_reason: String,
    pub last_assistant_message: Option<Vec<hoh_llm::ContentBlock>>,
}
hoh_kernel::event!(SubagentRunEndInfo = "subagent/end", Contained);

/// SHIM(hoh-schedule): the SDK activation handler; replace with the source event's owned type.
pub struct ScheduleResolveAgent {
    pub session_id: SessionId,
}
hoh_kernel::event!(
    ScheduleResolveAgent = "schedule/resolve-agent",
    Waterfall<Arc<dyn Agent>>
);

/// SHIM(hoh-goal): only the source's optional get(agent)?.phase === 'active' observation.
pub trait GoalsApi: Send + Sync + 'static {
    fn is_active(&self, agent: &Arc<dyn Agent>) -> bool;
}
hoh_kernel::service!(pub Goals: dyn GoalsApi = "goals");
/// SHIM(hoh-goal): activation observation; goal data stays merge-extensible.
pub struct GoalActivationChanged {
    pub session_id: SessionId,
    pub goal: Option<serde_json::Value>,
}
hoh_kernel::event!(GoalActivationChanged = "goal/activation-changed", Contained);

/// SHIM(hoh-llm-deepseek-api-key): explicit host-provided mount hook instead of porting DeepSeek.
/// Replace with registration of hoh-llm-openai in the profile; pre-registered adapters always win.
pub trait DeepSeekFallbackApi: Send + Sync + 'static {
    fn mount(&self, ctx: Context) -> BoxFuture<'static, Result<Fiber, BoxError>>;
}
hoh_kernel::service!(pub DeepSeekFallback: dyn DeepSeekFallbackApi = "sdkDeepSeekFallback");

/// Adapt the current working-directory shim to the real Agent without editing either owner.
pub(crate) fn directory_agent(agent: Arc<dyn Agent>) -> hoh_working_directory::shim::Agent {
    struct DirectoryAgent(Arc<dyn Agent>);
    impl hoh_working_directory::shim::AgentApi for DirectoryAgent {
        fn session(&self) -> hoh_session::Session {
            self.0.session().clone()
        }
        fn ctx(&self) -> Context {
            self.0.ctx().clone()
        }
        fn inject(&self, message: hoh_llm::UserMessage) -> Result<(), BoxError> {
            self.0.inject(message)
        }
    }
    hoh_working_directory::shim::Agent::new(DirectoryAgent(agent))
}
