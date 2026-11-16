//! Port of `@deepseek-ai/dsh-session-format` (`packages/session/session-format`).
//!
//! Pure adjacent streaming Session format migration machinery. The library is synchronous: rows are
//! pushed one at a time and stateful stages own and return their values.

pub mod catalog;
pub mod chain;
pub mod context;
pub mod error;
pub mod filename;
pub mod json;
pub mod types;

pub use catalog::*;
pub use chain::*;
pub use context::*;
pub use error::*;
pub use filename::*;
pub use json::*;
pub use types::*;
