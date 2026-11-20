//! One-shot headless DeepSeek Harness bundle.

pub mod json_stream;
mod runner;
pub mod startup;

pub use runner::{Config, HeadlessRunner, summarize};
pub use startup::{
    HEADLESS_STARTUP_SERVICE, HeadlessRuntime, HeadlessRuntimeKey, HeadlessStartup,
    HeadlessStartupKey, HeadlessStartupPlugin, headless_command, json_requested,
};

/// The unchanged profile patch exported by the source package.
pub const CORDIS_PATCH: &str = include_str!("../cordis.patch.yml");
