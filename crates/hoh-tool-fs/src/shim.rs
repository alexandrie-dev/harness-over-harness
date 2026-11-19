//! Seams for packages that are being written at the same time as this one. Nothing here belongs to
//! the source package: each item stands in for the public API of another package of dsh and is
//! replaced when that crate is accepted.

use std::sync::Arc;

use hoh_agent::Agent;
use hoh_kernel::Context;

/// The complete live agent carried by `ToolExecution`.
pub fn live_agent(_: &Context, agent: &Arc<dyn hoh_tools::Agent>) -> Option<Arc<dyn Agent>> {
    Some(agent.clone())
}
