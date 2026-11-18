//! Port of @deepseek-ai/dsh-session-format-v1-to-v2 (packages/session/session-format-v1-to-v2).
//!
//! Values enter through `hoh_util_values::js_json_parse`; the text boundary uses
//! `js_json_stringify`. Programmatic JSON must obey JS property order and
//! `JS_JSON_MAX_DEPTH`, as required by PORTING section 4.
pub mod codec;
pub mod dispositions;
mod helpers;
pub mod migration;
pub mod testing;
pub mod validation;

pub use codec::*;
pub use dispositions::*;
pub use hoh_session_format_v0_to_v1::RELEASED_V1_SESSION_FORMAT_CODEC;
pub use migration::*;
pub use validation::{assert_released_v2_header, restore_released_v2_artifact};
