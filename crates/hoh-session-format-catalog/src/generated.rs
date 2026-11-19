use hoh_session::KNOWN_SESSION_EVENT_TYPES;
use hoh_session_format::*;
use hoh_session_format_v0_to_v1::{
    RELEASED_V0_SESSION_FORMAT_CODEC, RELEASED_V1_SESSION_FORMAT_CODEC, SESSION_FORMAT_V0_TO_V1,
};
use hoh_session_format_v1_to_v2::{RELEASED_V2_SESSION_FORMAT_CODEC, SESSION_FORMAT_V1_TO_V2};
use hoh_session_format_v2_to_v3::{RELEASED_V3_SESSION_FORMAT_CODEC, SESSION_FORMAT_V2_TO_V3};
use hoh_session_format_v3_to_v4::{
    RELEASED_V4_SESSION_FORMAT_CODEC, SESSION_FORMAT_V3_TO_V4, assert_released_v4_header,
    restore_released_v4_artifact,
};
use std::{collections::HashSet, sync::Arc};
fn known() -> HashSet<String> {
    KNOWN_SESSION_EVENT_TYPES
        .iter()
        .map(|t| (*t).into())
        .collect()
}
/// Fixed generation ordering shared by native and parent-specific historical readers.
pub fn session_format_catalog_options() -> SessionFormatCatalogOptions {
    SessionFormatCatalogOptions {
        current_version: 4,
        codecs: vec![
            Arc::new(RELEASED_V0_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V1_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V2_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V3_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V4_SESSION_FORMAT_CODEC),
        ],
        current_encoder: Arc::new(RELEASED_V4_SESSION_FORMAT_CODEC),
        migrations: vec![
            Arc::new(SESSION_FORMAT_V0_TO_V1),
            Arc::new(SESSION_FORMAT_V1_TO_V2),
            Arc::new(SESSION_FORMAT_V2_TO_V3),
            Arc::new(SESSION_FORMAT_V3_TO_V4),
        ],
        restore_current: Arc::new(|artifact| {
            let restored = restore_released_v4_artifact(artifact, &known())?;
            crate::current::validate_installed_current_session_artifact(&restored)
                .map_err(SessionFormatError::other)?;
            Ok(restored)
        }),
        restore_transformed_current: Arc::new(|artifact| {
            restore_released_v4_artifact(artifact, &known())
        }),
        restore_current_header: Arc::new(|header| {
            assert_released_v4_header(&header)?;
            crate::current::validate_installed_current_session_header(&header)
                .map_err(SessionFormatError::other)?;
            Ok(header)
        }),
    }
}
pub fn session_format_catalog() -> Result<SessionFormatCatalog, SessionFormatError> {
    create_session_format_catalog(session_format_catalog_options())
}
