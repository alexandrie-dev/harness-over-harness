//! Private owner interfaces awaiting accepted dependencies.
use crate::{ReplayError, Result, error};
use hoh_session::{
    SESSION_FORMAT_VERSION, Session, SessionEvent, SessionId, SessionLogOffset,
    SessionSeedEventState,
};
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
// SHIM(hoh-session-format-catalog): createRestore/decodeRow/finish.
// Header validation uses Session restore; body facts use public content validators, not runtime
// message projection plugins. Full schema/relationships and migrations require restore integration.
// Comparison preparation already uses the real catalog's encodeCurrentHeader/Event.
// Replace this restore seam during the separately deferred historical-reader integration.
pub(crate) struct Restore {
    header: Value,
    events: Vec<SessionEvent>,
}
pub(crate) struct Artifact {
    pub header: Value,
    pub events: Vec<SessionEvent>,
    pub inherited_event_count: SessionLogOffset,
}
impl Restore {
    pub fn new(header: Value) -> Result<Self> {
        if header["type"] != "session" {
            return Err(error("first record must be a Session header"));
        }
        let version = header["version"]
            .as_u64()
            .filter(|n| *n <= hoh_session::MAX_SAFE_INTEGER)
            .ok_or_else(|| error("Session header version must be a non-negative safe integer"))?;
        if version != SESSION_FORMAT_VERSION {
            return Err(Box::new(ReplayError::UnsupportedMigration {
                message: format!(
                    "llm-replay: migration v{version}->v{SESSION_FORMAT_VERSION} requires hoh-session-format-catalog"
                ),
                cause: None,
            }));
        }
        let id = header["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| error("Session header lacks id"))?;
        // Seed-cut validation requires body events; header facts are checked first.
        Session::from_restore(
            SessionId::from(id),
            &[],
            header.clone(),
            SessionLogOffset::default(),
            SessionSeedEventState::Detached,
            None,
        )?;
        Ok(Self {
            header,
            events: vec![],
        })
    }
    pub fn decode_row(&mut self, mut row: Value) -> Result<()> {
        if ["text-chunks", "reasoning-chunks", "tool-call-chunks"]
            .contains(&row["type"].as_str().unwrap_or(""))
        {
            return Err(error(
                "current Session format does not admit packed chunk rows",
            ));
        }
        if let Some(ranges) = row.get_mut("sourceEventSeqs") {
            *ranges = serde_json::to_value(hoh_session::decode_seq_ranges(
                ranges,
                Some(self.events.len() as u64),
            )?)?;
        }
        let index = self.events.len();
        if !row["type"].as_str().is_some_and(|t| !t.is_empty())
            || row.get("data").is_none()
            || row.get("ignorable").is_some_and(|v| v != true)
        {
            return Err(error(format!(
                "seed event at index {index} has an invalid envelope"
            )));
        }
        if row["seq"].as_u64() != Some(index as u64) {
            return Err(error(format!(
                "seed event at index {index} must have contiguous seq {index}"
            )));
        }
        if row["time"]
            .as_i64()
            .is_none_or(|n| n.unsigned_abs() > hoh_session::MAX_SAFE_INTEGER)
        {
            return Err(error(format!(
                "seed event at index {index} time must be a safe integer"
            )));
        }
        self.events
            .push(hoh_session::adopt_session_event(SessionEvent::from_value(
                row,
            ))?);
        Ok(())
    }
    pub fn finish(self) -> Result<Artifact> {
        let marker = self
            .events
            .iter()
            .rfind(|e| e.event_type() == "session/end-seed" && e.data()["inherited"] == true)
            .and_then(|e| e.as_value()["seq"].as_u64());
        let inherited = if self.header["isSeeded"] == true {
            marker.ok_or_else(|| error("seeded Session lacks inherited session/end-seed"))?
        } else if marker.is_some() {
            return Err(error(
                "unseeded Session contains an inherited end-seed marker",
            ));
        } else {
            0
        };
        let inherited_event_count = SessionLogOffset::new(inherited)?;
        Ok(Artifact {
            header: self.header,
            events: self.events,
            inherited_event_count,
        })
    }
}
