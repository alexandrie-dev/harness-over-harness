//! Private owner interfaces: the compaction summary shim and the format-catalog restore.
use crate::Result;
use hoh_session::{SessionEvent, SessionLogOffset};
use hoh_session_format::{
    SessionFormatRecovery, SessionFormatRestore, SessionFormatRestoreOptions,
    SessionFormatValidation,
};
use hoh_session_format_catalog::create_session_format_catalog_with_children;
use serde::Deserialize;
use serde_json::Value;
// SHIM(hoh-compaction): persisted summary call facts; replace when its event data type is published.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompactionSummary {
    pub llm_stream_call: Option<bool>,
    pub raw_output: Option<Vec<hoh_llm::ContentBlock>>,
    pub usage: Option<hoh_llm::TokenUsage>,
}
/// The restore of one fixture: the source's `createSessionFormatCatalogWithChildren([])` with
/// `recovery: 'strict'` and `validation: 'current'`, so every released generation is migrated to
/// the current logical events and a refusal keeps its `SessionFormatError` class.
pub(crate) struct Restore {
    inner: SessionFormatRestore,
}
pub(crate) struct Artifact {
    pub header: Value,
    pub events: Vec<SessionEvent>,
    pub inherited_event_count: SessionLogOffset,
}
impl Restore {
    pub fn new(header: Value) -> Result<Self> {
        let catalog = create_session_format_catalog_with_children(vec![])?;
        let inner = catalog.create_restore(
            &header,
            SessionFormatRestoreOptions {
                recovery: SessionFormatRecovery::Strict,
                validation: SessionFormatValidation::Current,
            },
        )?;
        Ok(Self { inner })
    }
    pub fn decode_row(&mut self, row: Value) -> Result<()> {
        Ok(self.inner.decode_row(row)?)
    }
    pub fn finish(self) -> Result<Artifact> {
        let artifact = self.inner.finish()?;
        let events = artifact
            .events
            .into_iter()
            .map(|event| SessionEvent::from_value(event.into()))
            .collect();
        Ok(Artifact {
            header: artifact.header.into(),
            events,
            inherited_event_count: SessionLogOffset::new(artifact.inherited_event_count)?,
        })
    }
}
