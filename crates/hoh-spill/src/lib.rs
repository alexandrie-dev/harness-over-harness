//! Spill storage service contract.

pub mod types;

use hoh_error::BoxError;
use std::{future::Future, pin::Pin};
pub use types::{SaveTextSpill, SpillLocator, SpillOwner, SpillRef, SpillSource, spill_locator};

/// Abstract spill storage service implemented by a concrete backend.
pub trait SpillStoreApi: Send + Sync + 'static {
    /// Persist the full text and return its opaque locator and retrieval guidance.
    fn save_text(
        &self,
        input: SaveTextSpill,
    ) -> Pin<Box<dyn Future<Output = Result<SpillRef, BoxError>> + Send + '_>>;
}

hoh_kernel::service!(pub SpillStore: dyn SpillStoreApi = "spillStore");
