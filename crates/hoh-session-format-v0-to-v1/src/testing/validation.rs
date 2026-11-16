use crate::dispositions::RELEASED_V0_EVENT_TYPES;
use crate::relationships::{
    ReleasedRelationshipExtensions, assert_released_artifact_relationships,
};
use crate::validation::{
    assert_released_artifact_coordinates, assert_released_event_payload,
    assert_released_session_format_header,
};
use crate::validation_helpers::event_type;
use hoh_session_format::{SessionFormatArtifact, SessionFormatError};
use std::collections::HashSet;

fn validate(
    a: &SessionFormatArtifact,
    version: u64,
    legacy: bool,
    payload: bool,
    restart: bool,
) -> Result<(), SessionFormatError> {
    let types: HashSet<_> = RELEASED_V0_EVENT_TYPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_released_session_format_header(&a.header, version)?;
    assert_released_artifact_coordinates(a, legacy, Some(&types), false, true)?;
    if payload {
        for e in &a.events {
            if RELEASED_V0_EVENT_TYPES.contains(&event_type(e)) {
                assert_released_event_payload(e, version)?;
            }
        }
        assert_released_artifact_relationships(
            a,
            &ReleasedRelationshipExtensions {
                legacy_interrupted_turn_restart: restart,
                ..Default::default()
            },
        )?;
    }
    Ok(())
}
pub fn assert_released_v0_source_artifact(
    a: &SessionFormatArtifact,
) -> Result<(), SessionFormatError> {
    validate(a, 0, true, false, false)
}
pub fn assert_normalized_released_v0_artifact(
    a: &SessionFormatArtifact,
) -> Result<(), SessionFormatError> {
    validate(a, 0, false, true, true)
}
pub fn assert_released_v1_migration_source(
    a: &SessionFormatArtifact,
) -> Result<(), SessionFormatError> {
    validate(a, 1, false, true, true)
}
pub fn assert_released_v1_artifact(a: &SessionFormatArtifact) -> Result<(), SessionFormatError> {
    validate(a, 1, false, true, false)
}
pub fn assert_released_v1_physical_artifact(
    a: &SessionFormatArtifact,
) -> Result<(), SessionFormatError> {
    assert_released_session_format_header(&a.header, 1)?;
    assert_released_artifact_coordinates(a, false, None, true, false)
}
