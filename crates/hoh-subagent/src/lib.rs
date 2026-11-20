//! Abstract subagent seam: named-provider registry for delegating to child agents.
//!
//! Port of `@deepseek-ai/dsh-subagent`.

pub mod activation;
pub mod archive_admission;
pub mod assistant_output;
pub mod catalog;
pub mod child_agent;
pub mod continuation_messages;
pub mod control;
pub mod control_types;
pub mod depth;
pub mod descriptor;
pub mod error;
pub mod lifecycle;
pub mod list_children;
pub mod manager;
pub mod names;
pub mod out_of_process;
pub mod projection;
pub mod projection_types;
pub mod runtime;
pub mod structured;
pub mod types;

pub use archive_admission::install_subagent_archive_admission;
pub use assistant_output::{AssistantOutputFold, final_assistant_output};
pub use child_agent::*;
pub use control::{reject_prompt, validate_control_request};
pub use control_types::*;
pub use depth::{assert_subagent_max_depth, delegation_depth_of};
pub use descriptor::{
    ContinuableSubagentDescriptorData, ContinuableSubagentDescriptorInput,
    OneShotSubagentDescriptorData, SUBAGENT_DESCRIPTOR_VERSION, SubagentDescriptorData,
    fold_subagent_descriptor, snapshot_subagent_descriptor,
};
pub use error::{SubagentError, SubagentErrorCode};
pub use lifecycle::{
    SubagentProviderAdded, SubagentProviderRemoved, emit_provider_removed, emit_subagent_end,
    emit_subagent_start, epoch_stop_reason,
};
pub use list_children::{
    SubagentDescendantListEntry, list_children, list_descendants, resolve_agent_path,
};
pub use manager::SubagentManager;
pub use names::ROOT_AGENT_NAME;
pub use out_of_process::*;
pub use projection::{
    subagent_identity_projection_definition, subagent_timing_projection_definition,
};
pub use projection_types::{
    SubagentCatalogEntry, SubagentIdentityProjection, SubagentTimingProjection,
};
pub use runtime::{
    Config, ConfiguredMaxDepth, Deps, ProviderRegistration, SubagentPlugin, SubagentRuntime,
    Subagents,
};
pub use structured::{
    STRUCTURED_OUTPUT_INSTRUCTION, STRUCTURED_OUTPUT_TOOL, StructuredAttachment,
    attach_structured_runtime,
};
pub use types::*;
