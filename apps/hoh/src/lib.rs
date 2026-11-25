//! Minimal CLI surfaces over hoh-loader; runtime packages are explicitly registered.
pub mod args;
pub mod bin;
pub mod dump_config;
pub mod dump_config_schema;
mod plugin_creator;
mod plugin_inventory;
pub mod plugin_new;
pub mod process_shutdown;
pub mod profile_boot;
pub mod profile_initialization;
pub mod registry;
pub mod startup_diagnostics;
mod v1_registry;
mod window_glue;
