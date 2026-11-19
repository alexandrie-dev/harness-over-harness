use hoh_agent::{Model, ModelSelection};
use hoh_error::BoxError;
use std::sync::Arc;

struct UnsupportedModel;

impl Model for UnsupportedModel {
    fn current(&self) -> Option<ModelSelection> {
        panic!("this test Agent does not support model selection")
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("this test Agent does not support model selection".into())
    }
}

/// Create a fail-fast Model placeholder for structural Agent stubs.
pub fn unsupported_model() -> Arc<dyn Model> {
    Arc::new(UnsupportedModel)
}
