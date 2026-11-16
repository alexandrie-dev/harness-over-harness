use super::validation::{assert_released_v1_artifact, assert_released_v1_migration_source};
use crate::{
    RELEASED_V0_SESSION_FORMAT_CODEC, RELEASED_V1_SESSION_FORMAT_CODEC, SESSION_FORMAT_V0_TO_V1,
    assert_released_v1_header,
};
use hoh_session_format::*;
use serde_json::Value;
use std::sync::Arc;

struct UnusedEncoder;
impl SessionFormatCurrentEncoder for UnusedEncoder {
    fn encode_header(
        &self,
        h: &SessionFormatHeader,
        _cut: u64,
    ) -> Result<SessionFormatJsonObject, SessionFormatError> {
        Ok(h.clone())
    }
    fn encode_event(
        &self,
        e: &SessionFormatEvent,
    ) -> Result<SessionFormatJsonObject, SessionFormatError> {
        Ok(e.clone())
    }
}
fn restore(
    header: &Value,
    rows: &[Value],
    recovery: SessionFormatRecovery,
    validation: SessionFormatValidation,
) -> Result<SessionFormatArtifact, SessionFormatError> {
    let catalog = create_session_format_catalog(SessionFormatCatalogOptions {
        current_version: 1,
        codecs: vec![
            Arc::new(RELEASED_V0_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V1_SESSION_FORMAT_CODEC),
        ],
        current_encoder: Arc::new(UnusedEncoder),
        migrations: vec![Arc::new(SESSION_FORMAT_V0_TO_V1)],
        restore_current: Arc::new(|a| {
            assert_released_v1_artifact(&a)?;
            Ok(a)
        }),
        restore_transformed_current: Arc::new(|a| {
            assert_released_v1_migration_source(&a)?;
            Ok(a)
        }),
        restore_current_header: Arc::new(|h| {
            assert_released_v1_header(&h)?;
            Ok(h)
        }),
    })?;
    let mut current = catalog.create_restore(
        header,
        SessionFormatRestoreOptions {
            recovery,
            validation,
        },
    )?;
    for row in rows {
        current.decode_row(row.clone())?;
    }
    current.finish()
}
pub fn restore_v0_to_v1(
    header: &Value,
    rows: &[Value],
    recovery: SessionFormatRecovery,
) -> Result<SessionFormatArtifact, SessionFormatError> {
    restore(header, rows, recovery, SessionFormatValidation::Transformed)
}
pub fn restore_v1(
    header: &Value,
    rows: &[Value],
    recovery: SessionFormatRecovery,
) -> Result<SessionFormatArtifact, SessionFormatError> {
    restore(header, rows, recovery, SessionFormatValidation::Current)
}
