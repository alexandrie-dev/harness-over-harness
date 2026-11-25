//! Compatibility entry point for creating the basic in-repository Rust scaffold.
use hoh_error::BoxError;
use hoh_plugin_creator::{PluginCreator, Template};
use std::path::{Path, PathBuf};

pub fn create(root: &Path, id: &str) -> Result<PathBuf, BoxError> {
    Ok(PluginCreator.create(root, id, Template::Basic)?.path)
}
