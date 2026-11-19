use crate::{
    helpers::*,
    payload::{assert_v3_event, assert_v3_structural_row},
    validation::{assert_released_v3_header, assert_v3_event_admission},
};
use hoh_session_format::{
    SessionFormatArtifactDecoder, SessionFormatCodec, SessionFormatCurrentEncoder,
    SessionFormatEvent, SessionFormatEventRun, SessionFormatHeader, SessionFormatJsonObject,
    SessionFormatMigrationContext, SessionFormatRecovery, snapshot_session_format_json_labeled,
};
use hoh_session_format_v1_to_v2::RELEASED_V2_SESSION_FORMAT_CODEC;
use serde_json::Value;
#[derive(Debug, Clone, Copy)]
pub struct ReleasedV3SessionFormatCodec;
pub const RELEASED_V3_SESSION_FORMAT_CODEC: ReleasedV3SessionFormatCodec =
    ReleasedV3SessionFormatCodec;
impl SessionFormatCodec for ReleasedV3SessionFormatCodec {
    fn version(&self) -> u64 {
        3
    }
    fn decode_header(&self, value: &Value) -> Result<SessionFormatHeader> {
        let mut header =
            RELEASED_V2_SESSION_FORMAT_CODEC.decode_header(&v2_physical_header(value)?)?;
        header.insert("version".into(), 3.into());
        Ok(header)
    }
    fn create_decoder(
        &self,
        value: &Value,
        recovery: SessionFormatRecovery,
    ) -> Result<Box<dyn SessionFormatArtifactDecoder>> {
        let decoder = RELEASED_V2_SESSION_FORMAT_CODEC
            .create_decoder(&v2_physical_header(value)?, recovery)?;
        let mut header = decoder.header().clone();
        header.insert("version".into(), 3.into());
        Ok(Box::new(Decoder {
            decoder,
            header,
            recovery,
            issue: None,
            accepted_inherited_cut: None,
        }))
    }
}
impl SessionFormatCurrentEncoder for ReleasedV3SessionFormatCodec {
    fn encode_header(
        &self,
        header: &SessionFormatHeader,
        cut: u64,
    ) -> Result<SessionFormatJsonObject> {
        assert_released_v3_header(header)?;
        let mut header = header.clone();
        header.insert("version".into(), 2.into());
        let mut physical = RELEASED_V2_SESSION_FORMAT_CODEC.encode_header(&header, cut)?;
        physical.insert("version".into(), 3.into());
        Ok(physical)
    }
    fn encode_event(&self, event: &SessionFormatEvent) -> Result<SessionFormatJsonObject> {
        assert_v3_event_admission(event)?;
        assert_v3_event(event, None)?;
        RELEASED_V2_SESSION_FORMAT_CODEC.encode_event(event)
    }
}
pub fn assert_v3_row_admission(row: &Value) -> Result {
    assert_v3_structural_row(row)?;
    if let Some(event) = row.as_object() {
        assert_v3_event_admission(event)?;
    }
    Ok(())
}
fn v2_physical_header(value: &Value) -> Result<Value> {
    let snapshot = snapshot_session_format_json_labeled(value, "format v3 physical header")?;
    let Some(mut header) = snapshot
        .as_object()
        .cloned()
        .filter(|h| number_eq(field(h, "version"), 3))
    else {
        return Err(error("expected format v3 physical Session header"));
    };
    header.insert("version".into(), 2.into());
    Ok(Value::Object(header))
}
struct Decoder {
    decoder: Box<dyn SessionFormatArtifactDecoder>,
    header: SessionFormatHeader,
    recovery: SessionFormatRecovery,
    issue: Option<String>,
    accepted_inherited_cut: Option<u64>,
}
struct AdmissionContext<'a> {
    downstream: &'a mut dyn SessionFormatMigrationContext,
    recovery: SessionFormatRecovery,
    issue: &'a mut Option<String>,
    accepted_cut: &'a mut Option<u64>,
}
impl SessionFormatMigrationContext for AdmissionContext<'_> {
    fn emit_run(&mut self, run: Box<dyn SessionFormatEventRun>) -> Result {
        self.downstream.emit_run(run)
    }
    fn emit_event(&mut self, event: SessionFormatEvent) -> Result {
        assert_v3_event_admission(&event)?;
        if self.issue.is_none()
            && let Err(invalid) = assert_v3_event(&event, None)
        {
            if self.recovery == SessionFormatRecovery::Strict {
                return Err(invalid);
            }
            *self.issue = Some(invalid.to_string());
        }
        if let Some(issue) = self.issue {
            if kind(&event) == "turn/end" {
                return Err(error(issue.clone()));
            }
            return Ok(());
        }
        if kind(&event) == "session/end-seed"
            && field(&event, "data")
                .as_object()
                .is_some_and(|d| field(d, "inherited") == true)
        {
            *self.accepted_cut = Some(hoh_session_format::session_format_count(
                field(&event, "seq"),
                "event seq",
            )?);
        }
        self.downstream.emit_event(event)
    }
}
impl SessionFormatArtifactDecoder for Decoder {
    fn header(&self) -> &SessionFormatHeader {
        &self.header
    }
    fn decode_row(
        &mut self,
        row: Value,
        context: &mut dyn SessionFormatMigrationContext,
    ) -> Result {
        assert_v3_row_admission(&row)?;
        self.decoder.decode_row(
            row,
            &mut AdmissionContext {
                downstream: context,
                recovery: self.recovery,
                issue: &mut self.issue,
                accepted_cut: &mut self.accepted_inherited_cut,
            },
        )
    }
    fn finish(&mut self, context: &mut dyn SessionFormatMigrationContext) -> Result<u64> {
        if self.issue.is_none() {
            return self.decoder.finish(context);
        }
        let seeded = field(self.decoder.header(), "isSeeded") == true;
        if seeded && self.accepted_inherited_cut.is_none() {
            return Err(error(
                "format v3 seeded Session lacks an accepted inherited end-seed marker",
            ));
        }
        if !seeded && self.accepted_inherited_cut.is_some() {
            return Err(error(
                "format v3 unseeded Session contains an inherited end-seed marker",
            ));
        }
        Ok(self.accepted_inherited_cut.unwrap_or(0))
    }
}
