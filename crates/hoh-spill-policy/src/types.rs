//! Minimal spill ownership view of a tool execution.

use hoh_session::SessionId;
use hoh_tools::ToolExecution;

/// The owning session identity needed when a policy saves one artifact.
pub trait SpillPolicyExec {
    fn spill_owner(&self) -> Option<SessionId>;
}

impl SpillPolicyExec for ToolExecution {
    fn spill_owner(&self) -> Option<SessionId> {
        self.agent
            .as_ref()
            .map(|agent| agent.session().id().clone())
    }
}
