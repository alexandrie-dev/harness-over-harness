//! Temporary ownership seams for packages that do not have a Rust owner yet.

/// SHIM(hoh-subagent): the source type is a declaration-merge-extensible string
/// union. Keep it open until `packages/subagent/subagent` owns the Rust alias.
pub type SubagentStopReason = String;
