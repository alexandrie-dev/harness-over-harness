pub mod generation;
pub use crate::io::{IoHook, IoOperation, IoRequest, NodeFileError};
pub async fn read_zstd_prefix(
    backend: &crate::JsonlSessionPersistence,
    bytes: &[u8],
    signal: Option<&hoh_timeout::AbortSignal>,
) -> crate::Result<crate::format::SessionLogScan> {
    Ok(backend.decode_bytes(bytes, signal).await?.0)
}
pub async fn create_jsonl_test_backend(
    ctx: &hoh_kernel::Context,
    config: crate::Config,
    hook: Option<IoHook>,
) -> crate::Result<crate::JsonlSessionPersistence> {
    let backend = crate::JsonlSessionPersistence::with_io_hook(ctx, config, hook).await?;
    backend.install(ctx)?;
    Ok(backend)
}
pub fn writer(
    backend: &crate::JsonlSessionPersistence,
    id: &hoh_session::SessionId,
) -> Option<std::sync::Arc<crate::storage::JsonlSessionHandle>> {
    backend.writer_for_testing(id)
}
