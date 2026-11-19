//! Build-static first-party physical codecs and complete adjacent migration inventory.
pub mod children;
pub mod current;
pub mod generated;
pub mod historical;
pub mod message_projections;
pub use children::create_session_format_catalog_with_children;
pub use generated::session_format_catalog;
pub use historical::historical_session_format_catalog;
pub use hoh_session_format::SessionFormatError;
/// The shared enum's `UnsupportedMigration` variant is the source subclass.
pub use hoh_session_format::SessionFormatError as SessionFormatUnsupportedMigrationError;
