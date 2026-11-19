//! The file system questions the classifier asks, behind a seam: the plugin runs the whole
//! classification on a blocking thread (`assess_blocking`), and tests put a controllable
//! implementation in the place of the real one.

use std::path::Path;

pub trait Fs: Send + Sync + 'static {
    /// Something is at `path` (a dangling symlink counts).
    fn exists(&self, path: &Path) -> bool;
    /// `path` is a folder, or a link to one.
    fn is_dir(&self, path: &Path) -> bool;
    /// The names in the folder, `None` when it cannot be read.
    fn list(&self, dir: &Path) -> Option<Vec<String>>;
}

pub struct RealFs;

impl Fs for RealFs {
    fn exists(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }

    fn is_dir(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.is_dir())
    }

    fn list(&self, dir: &Path) -> Option<Vec<String>> {
        let entries = std::fs::read_dir(dir).ok()?;
        Some(
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect(),
        )
    }
}
