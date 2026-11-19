//! Private seams for optional packages not available in the current workspace.
//! This module implements no command, goal, scheduling, or subagent runtime.
use futures_util::future::BoxFuture;
use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_kernel::{Contained, Context, Waterfall};
use hoh_session::SessionId;
use std::sync::Arc;

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
