//! Seams for packages that are being written at the same time as this one. Nothing here belongs to
//! the source package: each item stands in for the public API of another package of dsh and is
//! replaced when that crate is accepted.

use std::sync::Arc;

use hoh_agent::Agent;
use hoh_kernel::Context;

/// SHIM(hoh-sandbox-policy): the `sandboxPolicy` service as the accepted `hoh-fs-sandbox` declares it
/// (one key type for the sandboxing filesystem and for this crate: the kernel tells service types
/// apart, so two declarations under the one name cannot both be mounted). Replace both when
/// `hoh-sandbox-policy` is wired into the composition.
pub use hoh_fs_sandbox::shim::{SandboxPolicyApi, SandboxPolicyKey};

/// SHIM(hoh-tools): `ToolExecution.agent` is still the small `hoh_tools::Agent` seam (scope key and
/// session). The tools that need the whole Agent (the working directory's context and inbox, the
/// approval route, the agent options) find it in the `agents` registry by the Session id. `None`
/// when no registry is mounted or the Agent is not registered. Replace with `exec.agent` itself when
/// `hoh-tools` adopts `hoh_agent::Agent`.
pub fn live_agent(ctx: &Context, agent: &Arc<dyn hoh_tools::Agent>) -> Option<Arc<dyn Agent>> {
    hoh_working_directory::live_agent(ctx, &agent.session())
}
