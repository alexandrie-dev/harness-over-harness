use crate::error::SessionFormatError;
use crate::types::{SessionFormatEvent, SessionFormatEventRun, SessionFormatMigrationContext};

/// Migration output context that expands compact runs into retained events.
#[derive(Debug, Default)]
pub struct SessionFormatEventCollector {
    /// Events retained by this collector in source order.
    pub values: Vec<SessionFormatEvent>,
}

impl SessionFormatEventCollector {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionFormatMigrationContext for SessionFormatEventCollector {
    /// Retain one settled event.
    fn emit_event(&mut self, event: SessionFormatEvent) -> Result<(), SessionFormatError> {
        self.values.push(event);
        Ok(())
    }

    /// Expand one compact run directly into retained events.
    fn emit_run(&mut self, run: Box<dyn SessionFormatEventRun>) -> Result<(), SessionFormatError> {
        self.values.extend(run.expand());
        Ok(())
    }
}
