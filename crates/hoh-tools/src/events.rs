//! Tool contribution changes and shared execution policy events.

use std::sync::Arc;

use hoh_kernel::{Contained, Emit, Waterfall};

use crate::contracts::{PostToolDecision, PreToolDecision, ToolExecution, ToolExecutionResult};

/// `tools/pre-execute`: allow, deny, cancel, or ask before dispatch. Delegating (`next.run()`)
/// delegates to allow; `Cancel` selects the canonical pre-dispatch cancellation result, and missing
/// approval support turns `Ask` into denial. Async gates must observe `exec.signal()`; the registry
/// rechecks cancellation after they settle but never abandons their future. Scope-filtered dispatch:
/// agent-scoped listeners receive only that agent's calls.
pub struct ToolsPreExecute {
    /// The pending call (name, parsed arguments, caller agent).
    pub exec: Arc<ToolExecution>,
}
hoh_kernel::event!(
    ToolsPreExecute = "tools/pre-execute",
    Waterfall<PreToolDecision>
);

/// `tools/execute`: around-dispatch waterfall for timeout, retry, or metrics. `next.run()` returns a
/// normalized result; wrappers may change only the signal (`exec.set_signal`), while call identity
/// remains immutable. The registry re-fuses the original caller signal before the body, so replacement
/// cannot detach caller cancellation; wrappers must still restore their signal and reach quiescence.
pub struct ToolsExecute {
    /// The allowed call about to dispatch (name, parsed arguments, caller agent, signal).
    pub exec: Arc<ToolExecution>,
}
hoh_kernel::event!(
    ToolsExecute = "tools/execute",
    Waterfall<ToolExecutionResult>
);

/// `tools/post-execute`: accept, replace, enrich, or block a normalized dispatch result. `next.run()`
/// accepts it unchanged; thrown tools still reach this waterfall as errors.
pub struct ToolsPostExecute {
    /// The call that just ran.
    pub exec: Arc<ToolExecution>,
    /// The dispatch outcome a listener may accept, replace, or block.
    pub result: Arc<ToolExecutionResult>,
}
hoh_kernel::event!(
    ToolsPostExecute = "tools/post-execute",
    Waterfall<PostToolDecision>
);

/// `tools/result`: observe the final outcome. Listener failures are contained and logged. Scope-filtered
/// dispatch keyed by `exec.agent`.
pub struct ToolsResult {
    /// The execution object that traversed the pipeline.
    pub exec: Arc<ToolExecution>,
    /// A snapshot of the final returned result.
    pub result: Arc<ToolExecutionResult>,
}
hoh_kernel::event!(ToolsResult = "tools/result", Contained);

/// `tools/change`: a tool was registered or unregistered, or a scoped restriction changed.
/// Unfiltered registry notification: scoped listeners receive changes from every scope because a
/// global contribution can affect every agent's next assembly.
pub struct ToolsChange;
hoh_kernel::event!(ToolsChange = "tools/change", Emit);
