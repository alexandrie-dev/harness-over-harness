use hoh_session_format::*;
use hoh_session_format_v0_to_v1::{
    RELEASED_V0_SESSION_FORMAT_CODEC, RELEASED_V1_SESSION_FORMAT_CODEC, SESSION_FORMAT_V0_TO_V1,
};
use hoh_session_format_v1_to_v2::{RELEASED_V2_SESSION_FORMAT_CODEC, SESSION_FORMAT_V1_TO_V2};
use hoh_session_format_v2_to_v3::{
    RELEASED_V3_SESSION_FORMAT_CODEC, SESSION_FORMAT_V2_TO_V3, assert_released_v3_header,
    restore_released_v3_artifact,
};
use hoh_session_format_v3_to_v4::RELEASED_V3_EVENT_TYPES;
use std::{collections::HashSet, sync::Arc};
fn restore(artifact: SessionFormatArtifact) -> Result<SessionFormatArtifact, SessionFormatError> {
    let known: HashSet<_> = RELEASED_V3_EVENT_TYPES
        .iter()
        .map(|t| (*t).into())
        .collect();
    restore_released_v3_artifact(artifact, &known)
}
pub fn historical_session_format_catalog() -> Result<SessionFormatCatalog, SessionFormatError> {
    create_session_format_catalog(SessionFormatCatalogOptions {
        current_version: 3,
        codecs: vec![
            Arc::new(RELEASED_V0_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V1_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V2_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V3_SESSION_FORMAT_CODEC),
        ],
        current_encoder: Arc::new(RELEASED_V3_SESSION_FORMAT_CODEC),
        migrations: vec![
            Arc::new(SESSION_FORMAT_V0_TO_V1),
            Arc::new(SESSION_FORMAT_V1_TO_V2),
            Arc::new(SESSION_FORMAT_V2_TO_V3),
        ],
        restore_current: Arc::new(restore),
        restore_transformed_current: Arc::new(restore),
        restore_current_header: Arc::new(|header| {
            assert_released_v3_header(&header)?;
            Ok(header)
        }),
    })
}
