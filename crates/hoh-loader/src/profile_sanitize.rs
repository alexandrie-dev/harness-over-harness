//! Recovery is deliberately independent of plugin loading and patch parsing.
use crate::LoadError;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Caller owns shutdown and write exclusion. The timestamp is supplied for deterministic recovery.
pub fn sanitize_profile(
    bin_name: &str,
    dir: &Path,
    bundles: &[String],
    timestamp: i64,
) -> Result<Option<PathBuf>, LoadError> {
    sanitize(bin_name, dir, bundles, timestamp, |from, to| {
        std::fs::rename(from, to)
    })
}
fn sanitize(
    bin_name: &str,
    dir: &Path,
    bundles: &[String],
    timestamp: i64,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<Option<PathBuf>, LoadError> {
    let manifest_path = dir.join("package.json");
    let manifest = if manifest_path.exists() {
        Some(crate::read_profile_manifest(bin_name, dir)?)
    } else {
        None
    };
    let patch = dir.join("cordis.patch.yml");
    let base = format!("{}.bak-{timestamp}", patch.display());
    let mut destination = PathBuf::from(&base);
    let mut ordinal = 0;
    while destination.try_exists().map_err(|source| LoadError::Io {
        file: destination.clone(),
        source,
    })? {
        ordinal += 1;
        destination = PathBuf::from(format!("{base}-{ordinal}"));
    }
    let backup = match rename(&patch, &destination) {
        Ok(()) => Some(destination),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(LoadError::Io {
                file: patch,
                source,
            });
        }
    };
    if let Some(manifest) = manifest {
        let updated = crate::profile_files::with_bundles(
            &manifest,
            &bundles
                .iter()
                .cloned()
                .map(Value::String)
                .collect::<Vec<_>>(),
        )?;
        crate::write_profile_manifest(dir, &updated)?;
    }
    Ok(backup)
}

#[cfg(test)]
mod tests {
    #[test]
    fn propagates_backup_failures_before_changing_activation() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = "{\"dsh\":{\"profile\":{\"bundles\":[\"broken\"]}}}";
        std::fs::write(dir.path().join("package.json"), manifest).unwrap();
        std::fs::write(dir.path().join("cordis.patch.yml"), ": broken YAML").unwrap();
        let error = super::sanitize("test", dir.path(), &["base".into()], 1, |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "patch rename denied",
            ))
        })
        .unwrap_err();
        assert!(error.to_string().ends_with("patch rename denied"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
            manifest
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("cordis.patch.yml")).unwrap(),
            ": broken YAML"
        );
    }
}
