//! A single policy for images the conversation asks the native window to read.
//!
//! The WebView supplies a path, but never an authorization root. The Tauri adapter obtains the
//! requesting session's cwd from the engine and passes it here. Canonical containment is checked
//! before and after opening, the opened inode must still be the canonical path, and only bounded
//! regular raster files are returned.

use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Take},
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::{fs::MetadataExt, fs::OpenOptionsExt};

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAsset {
    pub data_base64: String,
    pub mime_type: &'static str,
    pub size_bytes: u64,
    pub source_path: String,
    pub file_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageKind {
    Gif,
    Jpeg,
    Png,
    Webp,
}

impl ImageKind {
    fn mime_type(self) -> &'static str {
        match self {
            Self::Gif => "image/gif",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

fn unavailable(reason: &str) -> String {
    format!("Image unavailable: {reason}.")
}

fn expected_kind(path: &Path) -> Result<ImageKind, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "gif" => Ok(ImageKind::Gif),
        "jpg" | "jpeg" => Ok(ImageKind::Jpeg),
        "png" => Ok(ImageKind::Png),
        "webp" => Ok(ImageKind::Webp),
        _ => Err(unavailable(
            "the file is not a supported PNG, JPEG, GIF, or WebP image",
        )),
    }
}

fn sniff_kind(bytes: &[u8]) -> Option<ImageKind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageKind::Png);
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some(ImageKind::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(ImageKind::Gif);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(ImageKind::Webp);
    }
    None
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|_| unavailable("the conversation project folder cannot be resolved"))?;
    if !canonical.is_dir() {
        return Err(unavailable(
            "the conversation project folder is not available",
        ));
    }
    Ok(canonical)
}

fn protected_home_path(path: &Path, home: &Path) -> bool {
    let canonical_home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    path.starts_with(canonical_home.join(".claude"))
        || path.starts_with(canonical_home.join(".codex"))
}

fn system_pseudo_path(path: &Path) -> bool {
    ["/proc", "/sys", "/dev"]
        .iter()
        .any(|root| path.starts_with(root))
}

#[cfg(unix)]
fn open_regular(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        // A swapped-in FIFO must not hang the desktop, and a final symlink must not be followed.
        .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| unavailable("the file cannot be opened safely"))
}

#[cfg(not(unix))]
fn open_regular(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| unavailable("the file cannot be opened safely"))
}

fn read_bounded(mut file: Take<&mut File>) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| unavailable("the file could not be read"))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(unavailable("the image is larger than 20 MiB"));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn unchanged_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
}

#[cfg(not(unix))]
fn unchanged_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.len() == after.len() && before.modified().ok() == after.modified().ok()
}

fn content_fingerprint(bytes: &[u8]) -> u64 {
    // The revision is a cache validator, not an authenticity primitive. Hashing every returned
    // byte prevents an in-place rewrite with restored timestamps from reusing a stale Blob URL.
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// Read one image after binding it to the requesting conversation's authoritative project root.
pub fn read_project_image(
    project_root: &Path,
    requested: &str,
    home: &Path,
) -> Result<ImageAsset, String> {
    if requested.is_empty() || requested.contains('\0') {
        return Err(unavailable("the path is empty or invalid"));
    }
    let requested_path = Path::new(requested);
    if requested_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(unavailable("parent path segments are not allowed"));
    }

    let root = canonical_directory(project_root)?;
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    };
    // Check the lexical spelling as well as the resolved target. Otherwise a protected settings
    // directory that is itself a symlink could lose its `.claude` / `.codex` identity on realpath.
    if protected_home_path(&candidate, home) {
        return Err(unavailable("protected assistant settings cannot be shown"));
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|_| unavailable("the file was not found"))?;
    if !canonical.starts_with(&root) {
        return Err(unavailable(
            "the file is outside this conversation's project",
        ));
    }
    if protected_home_path(&canonical, home) {
        return Err(unavailable("protected assistant settings cannot be shown"));
    }
    if system_pseudo_path(&canonical) {
        return Err(unavailable("system and device files cannot be shown"));
    }
    let expected = expected_kind(&canonical)?;

    let mut file = open_regular(&canonical)?;
    let opened = file
        .metadata()
        .map_err(|_| unavailable("the file cannot be inspected"))?;
    if !opened.is_file() {
        return Err(unavailable("the path is not a regular file"));
    }
    if opened.len() > MAX_IMAGE_BYTES {
        return Err(unavailable("the image is larger than 20 MiB"));
    }

    // Resolve after opening. A directory or final entry replaced during the check is rejected,
    // and on Unix the opened inode must be the same object the canonical path now names.
    let checked = canonical
        .canonicalize()
        .map_err(|_| unavailable("the file changed while it was being opened"))?;
    if checked != canonical || !checked.starts_with(&root) || protected_home_path(&checked, home) {
        return Err(unavailable("the file changed while it was being opened"));
    }
    let named = checked
        .metadata()
        .map_err(|_| unavailable("the file changed while it was being opened"))?;
    #[cfg(unix)]
    if opened.dev() != named.dev() || opened.ino() != named.ino() {
        return Err(unavailable("the file changed while it was being opened"));
    }

    let bytes = read_bounded((&mut file).take(MAX_IMAGE_BYTES + 1))?;
    if bytes.is_empty() {
        return Err(unavailable("the image is empty"));
    }
    if sniff_kind(&bytes) != Some(expected) {
        return Err(unavailable("the file contents do not match its image type"));
    }

    // An in-place writer can keep the inode while changing the bytes. Require stable metadata
    // across the whole read, then verify once more that the name still resolves to this handle.
    let after_read = file
        .metadata()
        .map_err(|_| unavailable("the file changed while it was being read"))?;
    if !unchanged_file(&opened, &after_read) || after_read.len() != bytes.len() as u64 {
        return Err(unavailable("the file changed while it was being read"));
    }
    let checked_after = canonical
        .canonicalize()
        .map_err(|_| unavailable("the file changed while it was being read"))?;
    if checked_after != canonical
        || !checked_after.starts_with(&root)
        || protected_home_path(&checked_after, home)
    {
        return Err(unavailable("the file changed while it was being read"));
    }
    let named_after = checked_after
        .metadata()
        .map_err(|_| unavailable("the file changed while it was being read"))?;
    if !unchanged_file(&after_read, &named_after) {
        return Err(unavailable("the file changed while it was being read"));
    }
    let fingerprint = content_fingerprint(&bytes);

    #[cfg(unix)]
    let revision = format!(
        "{}:{}:{}:{}:{}:{fingerprint:016x}",
        after_read.dev(),
        after_read.ino(),
        bytes.len(),
        after_read.mtime(),
        after_read.mtime_nsec()
    );
    #[cfg(not(unix))]
    let revision = format!("{}:{fingerprint:016x}", bytes.len());

    Ok(ImageAsset {
        data_base64: encode_base64(&bytes),
        mime_type: expected.mime_type(),
        size_bytes: bytes.len() as u64,
        source_path: canonical.to_string_lossy().into_owned(),
        file_revision: revision,
    })
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        output.push(TABLE[((word >> 18) & 63) as usize] as char);
        output.push(TABLE[((word >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((word >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(word & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nvalid-test-payload";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "hoh-image-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn assert_refused(result: Result<ImageAsset, String>, words: &str) {
        let error = result.unwrap_err();
        assert!(error.contains(words), "{error:?} did not contain {words:?}");
    }

    #[test]
    fn reads_a_raster_inside_the_conversation_project() {
        let scratch = Scratch::new("inside");
        let project = scratch.0.join("project");
        std::fs::create_dir_all(project.join("shots")).unwrap();
        std::fs::write(project.join("shots/main.png"), PNG).unwrap();
        let image = read_project_image(&project, "shots/main.png", &scratch.0).unwrap();
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.size_bytes, PNG.len() as u64);
        assert_eq!(image.data_base64, encode_base64(PNG));
        assert!(Path::new(&image.source_path).starts_with(project.canonicalize().unwrap()));
        let mut changed = PNG.to_vec();
        *changed.last_mut().unwrap() ^= 1;
        std::fs::write(project.join("shots/main.png"), &changed).unwrap();
        let changed_image = read_project_image(&project, "shots/main.png", &scratch.0).unwrap();
        assert_ne!(changed_image.file_revision, image.file_revision);
    }

    #[test]
    fn rejects_parent_segments_and_another_projects_file() {
        let scratch = Scratch::new("scope");
        let (project, other) = (scratch.0.join("project"), scratch.0.join("other"));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("secret.png"), PNG).unwrap();
        assert_refused(
            read_project_image(&project, "../other/secret.png", &scratch.0),
            "parent path",
        );
        assert_refused(
            read_project_image(
                &project,
                other.join("secret.png").to_str().unwrap(),
                &scratch.0,
            ),
            "outside this conversation's project",
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_that_escapes_the_project() {
        let scratch = Scratch::new("symlink");
        let (project, other) = (scratch.0.join("project"), scratch.0.join("other"));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("secret.png"), PNG).unwrap();
        std::os::unix::fs::symlink(other.join("secret.png"), project.join("alias.png")).unwrap();
        assert_refused(
            read_project_image(&project, "alias.png", &scratch.0),
            "outside this conversation's project",
        );
    }

    #[test]
    fn rejects_protected_home_and_system_files_even_under_a_broad_root() {
        let scratch = Scratch::new("protected");
        let home = scratch.0.join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".claude/screenshot.png"), PNG).unwrap();
        std::fs::write(home.join(".codex/screenshot.png"), PNG).unwrap();
        assert_refused(
            read_project_image(&home, ".claude/screenshot.png", &home),
            "protected assistant settings",
        );
        assert_refused(
            read_project_image(&home, ".codex/screenshot.png", &home),
            "protected assistant settings",
        );
        #[cfg(unix)]
        {
            let linked_home = scratch.0.join("linked-home");
            let vault = scratch.0.join("vault");
            std::fs::create_dir_all(&linked_home).unwrap();
            std::fs::create_dir_all(&vault).unwrap();
            std::fs::write(vault.join("screenshot.png"), PNG).unwrap();
            std::os::unix::fs::symlink(&vault, linked_home.join(".claude")).unwrap();
            assert_refused(
                read_project_image(
                    &scratch.0,
                    "linked-home/.claude/screenshot.png",
                    &linked_home,
                ),
                "protected assistant settings",
            );
        }
        assert_refused(
            read_project_image(Path::new("/"), "/proc/version", &home),
            "system and device files",
        );
        assert_refused(
            read_project_image(Path::new("/"), "/etc/passwd", &home),
            "not a supported",
        );
    }

    #[test]
    fn rejects_non_images_mismatched_contents_and_oversized_images() {
        let scratch = Scratch::new("type-size");
        let project = scratch.0.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("notes.txt"), b"hello").unwrap();
        std::fs::write(project.join("fake.png"), b"not a png").unwrap();
        let large = File::create(project.join("large.png")).unwrap();
        large.set_len(MAX_IMAGE_BYTES + 1).unwrap();
        assert_refused(
            read_project_image(&project, "notes.txt", &scratch.0),
            "not a supported",
        );
        assert_refused(
            read_project_image(&project, "fake.png", &scratch.0),
            "contents do not match",
        );
        assert_refused(
            read_project_image(&project, "large.png", &scratch.0),
            "larger than 20 MiB",
        );
    }
}
