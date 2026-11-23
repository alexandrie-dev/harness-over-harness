//! Pure, replayable conversation-statistics projection.
//!
//! This crate intentionally owns no I/O or persistence. Provider adapters feed raw JSON values
//! or normalized [`StatsEvent`]s into [`SessionStats`], and consumers publish the resulting
//! [`SessionStatsSnapshot`] through the engine/protocol boundary in HOH-43 Part B.

mod reducer;
mod types;

pub use reducer::{SessionStats, explicit_codex_exec, is_codex_mcp_tool};
pub use types::*;
