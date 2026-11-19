//! Message feedback contracts ported from `@deepseek-ai/dsh-message-feedback`.

mod service;
mod shim;
mod types;

pub use service::*;
pub use shim::{FEEDBACK_CATEGORIES, FeedbackCategory};
pub use types::*;
