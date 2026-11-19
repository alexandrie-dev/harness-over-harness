//! Port of @deepseek-ai/dsh-session-format-v2-to-v3 (packages/session/session-format-v2-to-v3).
//! Frozen released-v2 to v3 Session migration and native v3 admission.
//!
//! JSON text uses `hoh_util_values::js_json_*`. Input values must already have
//! JavaScript property order and obey `JS_JSON_MAX_DEPTH` (4096); application
//! threads provide the 64 MiB stack required by PORTING section 4.
pub mod codec;
mod helpers;
pub mod migration;
pub mod payload;
pub mod references;
pub mod validation;
pub use codec::*;
pub use hoh_session_format_v1_to_v2::RELEASED_V2_SESSION_FORMAT_CODEC;
pub use migration::*;
pub use validation::*;
