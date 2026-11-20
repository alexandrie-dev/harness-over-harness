//! Host-side Session archive and Markdown export primitives.

mod archive;
mod client;
mod markdown;
mod routes;
mod zip;

pub use archive::*;
pub use client::*;
pub use markdown::format_session_markdown;
pub use routes::*;
pub use zip::{ZipEntry, write_zip};

/// Cordis package name used by the source package.
pub const NAME: &str = "session-log-download";
