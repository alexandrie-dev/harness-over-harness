//! Interface-first seam owned by the not-yet-ported PTC tool executor.

use std::sync::Arc;

use hoh_llm::{ContentBlock, ToolCallId};
use hoh_tools::ToolExecution;

/// One durable nested-dispatch projection before it is written to the session log.
pub struct PtcDispatchLog {
    pub exec: Arc<ToolExecution>,
    pub name: String,
    pub sub_call_id: ToolCallId,
}

hoh_kernel::event!(
    PtcDispatchLog = "tools/ptc-dispatch-log",
    hoh_kernel::Waterfall<Vec<ContentBlock>>
);
