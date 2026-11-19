//! Node-private ABI probing has no Rust equivalent; libzstd supplies the reusable native context.
pub use crate::zstd::ZstdFrameDecoder as NodePrivateZstdFrameDecoder;
