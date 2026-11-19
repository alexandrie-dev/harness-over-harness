//! Port of `@deepseek-ai/dsh-sdk-protocol` (`packages/sdk/protocol`).

mod shim;
pub mod transport;
pub mod types;

pub use transport::*;
pub use types::*;
