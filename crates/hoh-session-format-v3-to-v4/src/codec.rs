use crate::{
    developer::assert_v4_developer_data, fork_result::assert_v4_fork_result, helpers::*,
    message_sources::assert_v4_source_row_admission, retired_syntax::assert_v4_retired_syntax,
    system_message::assert_v4_system_message_fields, tool_role::assert_v4_tool_result_message,
    validation::assert_released_v4_header,
};
use hoh_session_format::*;
use hoh_session_format_v2_to_v3::RELEASED_V2_SESSION_FORMAT_CODEC;
use serde_json::Value;
use std::collections::HashSet;
#[derive(Debug, Clone, Copy)]
pub struct ReleasedV4SessionFormatCodec;
pub const RELEASED_V4_SESSION_FORMAT_CODEC: ReleasedV4SessionFormatCodec =
    ReleasedV4SessionFormatCodec;
fn physical_v2(value: &Value) -> Result<Value> {
    let mut value = value
        .as_object()
        .filter(|h| number_eq(h.get("version"), 4))
        .cloned()
        .ok_or_else(|| error("expected format v4 physical header"))?;
    value.insert("version".into(), 2.into());
    Ok(value.into())
}
impl SessionFormatCodec for ReleasedV4SessionFormatCodec {
    fn version(&self) -> u64 {
        4
    }
    fn decode_header(&self, value: &Value) -> Result<SessionFormatHeader> {
        let mut h = RELEASED_V2_SESSION_FORMAT_CODEC.decode_header(&physical_v2(value)?)?;
        h.insert("version".into(), 4.into());
        Ok(h)
    }
    fn create_decoder(
        &self,
        value: &Value,
        recovery: SessionFormatRecovery,
    ) -> Result<Box<dyn SessionFormatArtifactDecoder>> {
        let decoder =
            RELEASED_V2_SESSION_FORMAT_CODEC.create_decoder(&physical_v2(value)?, recovery)?;
        let mut header = decoder.header().clone();
        header.insert("version".into(), 4.into());
        Ok(Box::new(Decoder { decoder, header }))
    }
}
impl SessionFormatCurrentEncoder for ReleasedV4SessionFormatCodec {
    fn encode_header(
        &self,
        header: &SessionFormatHeader,
        cut: u64,
    ) -> Result<SessionFormatJsonObject> {
        assert_released_v4_header(header)?;
        let mut h = header.clone();
        h.insert("version".into(), 2.into());
        let mut h = RELEASED_V2_SESSION_FORMAT_CODEC.encode_header(&h, cut)?;
        h.insert("version".into(), 4.into());
        Ok(h)
    }
    fn encode_event(&self, event: &SessionFormatEvent) -> Result<SessionFormatJsonObject> {
        let row = Value::Object(event.clone());
        if kind(event) == "developer/message" && field(event, "ignorable") == true {
            assert_v4_developer_data(event)?;
            assert_v4_retired_syntax(&row)?;
        }
        assert_v4_row_admission(&row, None)?;
        RELEASED_V2_SESSION_FORMAT_CODEC.encode_event(event)
    }
}
struct Decoder {
    decoder: Box<dyn SessionFormatArtifactDecoder>,
    header: SessionFormatHeader,
}
impl SessionFormatArtifactDecoder for Decoder {
    fn header(&self) -> &SessionFormatHeader {
        &self.header
    }
    fn header_inherited_event_count(&self) -> Option<u64> {
        self.decoder.header_inherited_event_count()
    }
    fn decode_row(
        &mut self,
        row: Value,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        assert_v4_row_admission(&row, None)?;
        self.decoder.decode_row(row, context)
    }
    fn finish(&mut self, context: &mut dyn SessionFormatMigrationContext) -> Result<u64> {
        self.decoder.finish(context)
    }
}
pub fn assert_v4_row_admission(row: &Value, known_event_types: Option<&HashSet<String>>) -> Result {
    if let Some(event) = row.as_object() {
        if kind(event) == "developer/message"
            && field(event, "ignorable") == true
            && !known_event_types.is_some_and(|k| k.contains("developer/message"))
        {
            return Ok(());
        }
        assert_v4_developer_data(event)?;
    }
    assert_v4_source_row_admission(row)?;
    assert_v4_retired_syntax(row)?;
    assert_v4_system_message_fields(row)?;
    if let Some(event) = row.as_object().filter(|e| kind(e) == "tool/result") {
        assert_v4_tool_result_message(event)?;
        assert_v4_fork_result(row)?;
    }
    Ok(())
}
