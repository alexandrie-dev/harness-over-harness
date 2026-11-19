//! Minimal CLI surfaces over hoh-loader; runtime packages are explicitly registered.
pub mod args;
pub mod bin;
pub mod dump_config;
pub mod dump_config_schema;
pub mod process_shutdown;
pub mod profile_boot;
pub mod profile_initialization;
pub mod registry;
pub mod startup_diagnostics;
mod window_glue;
