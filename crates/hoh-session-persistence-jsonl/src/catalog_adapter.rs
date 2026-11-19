use crate::{
    Result,
    generation::{JsonlGenerationFormatAdapter, JsonlGenerationRestore},
};
use hoh_session_format::*;
use serde_json::Value;
pub(crate) struct CatalogAdapter(pub SessionFormatCatalog);
impl JsonlGenerationFormatAdapter for CatalogAdapter {
    fn current_version(&self) -> u64 {
        self.0.current_version()
    }
    fn create_restore(&self, header: &Value) -> Result<Box<dyn JsonlGenerationRestore>> {
        Ok(Box::new(self.0.create_restore(
            header,
            SessionFormatRestoreOptions {
                recovery: SessionFormatRecovery::Recoverable,
                validation: SessionFormatValidation::Current,
            },
        )?))
    }
    fn encode_header(&self, header: &SessionFormatHeader, cut: u64) -> Result<Value> {
        Ok(self.0.encode_current_header(header, cut)?.into())
    }
    fn encode_event(&self, event: &SessionFormatEvent) -> Result<Value> {
        Ok(self.0.encode_current_event(event)?.into())
    }
}
