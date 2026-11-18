use crate::{RELEASED_V4_SESSION_FORMAT_CODEC, helpers::*};
use hoh_session_format::*;
pub fn assert_released_v4_artifact(artifact: &SessionFormatArtifact) -> Result {
    let codec = RELEASED_V4_SESSION_FORMAT_CODEC;
    let header = codec.encode_header(&artifact.header, artifact.inherited_event_count)?;
    let mut decoder = codec.create_decoder(&header.into(), SessionFormatRecovery::Strict)?;
    let mut output = SessionFormatEventCollector::new();
    for event in &artifact.events {
        decoder.decode_row(codec.encode_event(event)?.into(), &mut output)?;
    }
    decoder.finish(&mut output)?;
    Ok(())
}
