//! Temporary seam for the not-yet-unified tool execution Agent API.

use std::sync::Arc;

use hoh_agent::{Agent, Agents};
use hoh_kernel::Context;

/// SHIM(hoh-tools): recover the full live Agent by the Session carried by the tools seam.
pub fn live_agent(ctx: &Context, agent: &Arc<dyn hoh_tools::Agent>) -> Option<Arc<dyn Agent>> {
    ctx.get::<Agents>()?.get(agent.session().id())
}
