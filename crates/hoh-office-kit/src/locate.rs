//! Compatibility helpers for Kit package locations.
//!
//! Production conversion uses [`crate::ConverterFactory`]. These helpers retain the source package's
//! installation-anchor and ASAR path rules for embedders which still package the JavaScript Kit.

use hoh_error::BoxError;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub type EnginePackageResolver =
    Arc<dyn Fn(&str) -> std::result::Result<PathBuf, BoxError> + Send + Sync + 'static>;

pub enum KitModuleResolution {
    Available {
        entry: PathBuf,
        resolve_engine_package: EnginePackageResolver,
    },
    Missing {
        error: BoxError,
    },
}

pub fn resolve_kit_module(
    lookup: impl FnOnce() -> std::result::Result<PathBuf, BoxError>,
) -> KitModuleResolution {
    match lookup() {
        Ok(entry) => KitModuleResolution::Available {
            resolve_engine_package: create_engine_package_resolver(&entry),
            entry,
        },
        Err(error) => KitModuleResolution::Missing { error },
    }
}

/// Anchor engine lookup at the selected Kit installation.
pub fn create_engine_package_resolver(kit_entry: &Path) -> EnginePackageResolver {
    let kit_directory = kit_entry
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf();
    Arc::new(move |name| {
        let manifest = kit_directory
            .join("node_modules")
            .join(name)
            .join("package.json");
        match std::fs::symlink_metadata(&manifest) {
            Ok(metadata) if metadata.is_file() => {
                Ok(if name == "@deepseek-ai/libreoffice-kit-wasm" {
                    manifest
                } else {
                    PathBuf::from(unpacked_path(&manifest.to_string_lossy()))
                })
            }
            Ok(_) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Installed LibreOfficeKit package is incomplete: {name}. Reinstall the package."
                ),
            )
            .into()),
            Err(error) => Err(error.into()),
        }
    })
}

/// Replace only the first complete `.asar` path segment.
pub fn unpacked_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut start = 0;
    while let Some(relative) = path[start..].find(".asar") {
        let index = start + relative;
        let after = index + 5;
        if bytes
            .get(after)
            .is_none_or(|byte| matches!(byte, b'/' | b'\\'))
        {
            let mut output = String::with_capacity(path.len() + ".unpacked".len());
            output.push_str(&path[..after]);
            output.push_str(".unpacked");
            output.push_str(&path[after..]);
            return output;
        }
        start = after;
    }
    path.to_owned()
}
