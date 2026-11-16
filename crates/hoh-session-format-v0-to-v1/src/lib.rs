//! Frozen released-v0 physical codec and migration into shared-layout v1.
//!
//! JSON text enters through `hoh_util_values::js_json_parse` and leaves through
//! `js_json_stringify` / `js_json_stringify_pretty`. Recursive payload validators
//! require values no deeper than `hoh_util_values::JS_JSON_MAX_DEPTH`, including
//! values assembled in code (PORTING section 4).
pub mod codec;
pub mod dispositions;
pub mod migration;
pub mod payload_validation;
pub mod relationships;
pub mod testing;
pub mod validation;
pub mod validation_helpers;

pub use codec::*;
pub use dispositions::*;
pub use migration::*;
pub use payload_validation::assert_released_payload_semantics;
pub use relationships::{ReleasedRelationshipExtensions, assert_released_artifact_relationships};
pub use validation::{
    assert_released_event_payload, assert_released_surface_metadata, assert_released_v1_header,
    restore_released_v1_artifact,
};
