//! Port of `@deepseek-ai/dsh-compaction`.

mod brand;
pub mod checkpoint;
mod engine;
mod shim;
pub mod tool_pairing;
pub mod types;

pub use brand::CompactionId;
pub use checkpoint::{
    CompactionCheckpointSource, compact_checkpoint_source, is_compact_checkpoint_source,
};
pub use engine::*;
pub use tool_pairing::{tool_pairing_balanced_after, tool_pairing_balanced_before};
pub use types::*;
