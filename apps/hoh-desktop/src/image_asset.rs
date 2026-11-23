//! A single policy for images the conversation asks the native window to read.
//!
//! The WebView supplies a path, but never an authorization root. The Tauri adapter obtains the
//! requesting session's cwd from the engine and passes it here. Canonical containment is checked
//! before and after opening, the opened inode must still be the canonical path, and only bounded
//! regular raster files are returned.

use serde::Serialize;
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::{Read, Take},
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt},
};

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_IMAGE_FRAMES: u32 = 300;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImageProfile {
    width: u32,
    height: u32,
    frames: u32,
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

fn invalid_header() -> String {
    unavailable("the image header is invalid")
}

fn validate_profile(width: u32, height: u32, frames: u32) -> Result<ImageProfile, String> {
    if width == 0 || height == 0 || frames == 0 {
        return Err(invalid_header());
    }
    if width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(unavailable(
            "the image dimensions exceed 16384 pixels per side or 40 million pixels",
        ));
    }
    if frames > MAX_IMAGE_FRAMES {
        return Err(unavailable("the image has more than 300 frames"));
    }
    Ok(ImageProfile {
        width,
        height,
        frames,
    })
}

fn be_u16(bytes: &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(..2)?.try_into().ok()?))
}

fn be_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?))
}

fn le_u16(bytes: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(..2)?.try_into().ok()?))
}

fn le_u24(bytes: &[u8]) -> Option<u32> {
    let bytes = bytes.get(..3)?;
    Some(u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16))
}

fn le_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

fn png_profile(bytes: &[u8]) -> Result<ImageProfile, String> {
    if bytes.len() < 33
        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || be_u32(&bytes[8..12]) != Some(13)
        || bytes.get(12..16) != Some(b"IHDR")
    {
        return Err(invalid_header());
    }
    let width = be_u32(&bytes[16..20]).ok_or_else(invalid_header)?;
    let height = be_u32(&bytes[20..24]).ok_or_else(invalid_header)?;
    let bit_depth = bytes[24];
    let color_type = bytes[25];
    let legal_depth = matches!(
        (color_type, bit_depth),
        (0, 1 | 2 | 4 | 8 | 16) | (2, 8 | 16) | (3, 1 | 2 | 4 | 8) | (4, 8 | 16) | (6, 8 | 16)
    );
    if !legal_depth || bytes[26] != 0 || bytes[27] != 0 || bytes[28] > 1 {
        return Err(invalid_header());
    }
    validate_profile(width, height, 1)
}

fn jpeg_profile(bytes: &[u8]) -> Result<ImageProfile, String> {
    if !bytes.starts_with(b"\xff\xd8") {
        return Err(invalid_header());
    }
    let mut offset = 2usize;
    while offset < bytes.len() {
        if bytes[offset] != 0xff {
            return Err(invalid_header());
        }
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *bytes.get(offset).ok_or_else(invalid_header)?;
        offset += 1;
        if marker == 0x00 {
            return Err(invalid_header());
        }
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if marker == 0xd9 || marker == 0xda {
            return Err(invalid_header());
        }
        let length = usize::from(
            be_u16(bytes.get(offset..).ok_or_else(invalid_header)?).ok_or_else(invalid_header)?,
        );
        if length < 2 {
            return Err(invalid_header());
        }
        let end = offset.checked_add(length).ok_or_else(invalid_header)?;
        let segment = bytes.get(offset..end).ok_or_else(invalid_header)?;
        let is_sof = matches!(
            marker,
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf
        );
        if is_sof {
            if segment.len() < 8 {
                return Err(invalid_header());
            }
            let components = usize::from(segment[7]);
            if components == 0 || length != 8 + 3 * components {
                return Err(invalid_header());
            }
            let height = u32::from(be_u16(&segment[3..5]).ok_or_else(invalid_header)?);
            let width = u32::from(be_u16(&segment[5..7]).ok_or_else(invalid_header)?);
            return validate_profile(width, height, 1);
        }
        offset = end;
    }
    Err(invalid_header())
}

fn skip_gif_sub_blocks(bytes: &[u8], offset: &mut usize) -> Result<(), String> {
    loop {
        let length = usize::from(*bytes.get(*offset).ok_or_else(invalid_header)?);
        *offset += 1;
        if length == 0 {
            return Ok(());
        }
        *offset = (*offset).checked_add(length).ok_or_else(invalid_header)?;
        if *offset > bytes.len() {
            return Err(invalid_header());
        }
    }
}

fn gif_profile(bytes: &[u8]) -> Result<ImageProfile, String> {
    if bytes.len() < 13 || !(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return Err(invalid_header());
    }
    let width = u32::from(le_u16(&bytes[6..8]).ok_or_else(invalid_header)?);
    let height = u32::from(le_u16(&bytes[8..10]).ok_or_else(invalid_header)?);
    let packed = bytes[10];
    let mut offset = 13usize;
    if packed & 0x80 != 0 {
        let colors = 2usize << usize::from(packed & 0x07);
        offset = offset.checked_add(colors * 3).ok_or_else(invalid_header)?;
        if offset > bytes.len() {
            return Err(invalid_header());
        }
    }
    let mut frames = 0u32;
    let mut trailer = false;
    while offset < bytes.len() {
        let marker = bytes[offset];
        offset += 1;
        match marker {
            0x3b => {
                trailer = true;
                break;
            }
            0x21 => {
                offset += 1;
                if offset > bytes.len() {
                    return Err(invalid_header());
                }
                skip_gif_sub_blocks(bytes, &mut offset)?;
            }
            0x2c => {
                let descriptor = bytes.get(offset..offset + 9).ok_or_else(invalid_header)?;
                let frame_width = u32::from(le_u16(&descriptor[4..6]).ok_or_else(invalid_header)?);
                let frame_height = u32::from(le_u16(&descriptor[6..8]).ok_or_else(invalid_header)?);
                validate_profile(frame_width, frame_height, 1)?;
                offset += 9;
                if descriptor[8] & 0x80 != 0 {
                    let colors = 2usize << usize::from(descriptor[8] & 0x07);
                    offset = offset.checked_add(colors * 3).ok_or_else(invalid_header)?;
                }
                offset += 1;
                if offset > bytes.len() {
                    return Err(invalid_header());
                }
                skip_gif_sub_blocks(bytes, &mut offset)?;
                frames = frames.checked_add(1).ok_or_else(invalid_header)?;
                if frames > MAX_IMAGE_FRAMES {
                    return Err(unavailable("the image has more than 300 frames"));
                }
            }
            _ => return Err(invalid_header()),
        }
    }
    if !trailer {
        return Err(invalid_header());
    }
    validate_profile(width, height, frames)
}

fn vp8_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 10 || data.get(3..6) != Some(b"\x9d\x01\x2a") {
        return None;
    }
    let width = u32::from(le_u16(&data[6..8])? & 0x3fff);
    let height = u32::from(le_u16(&data[8..10])? & 0x3fff);
    Some((width, height))
}

fn vp8l_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 5 || data[0] != 0x2f {
        return None;
    }
    let bits = le_u32(&data[1..5])?;
    Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
}

fn webp_frame_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut offset = 0usize;
    let mut dimensions = None;
    while offset < data.len() {
        let header = data.get(offset..offset + 8)?;
        let size = usize::try_from(le_u32(&header[4..8])?).ok()?;
        let start = offset.checked_add(8)?;
        let end = start.checked_add(size)?;
        let payload = data.get(start..end)?;
        let parsed = match &header[..4] {
            b"ALPH" if dimensions.is_none() => None,
            b"VP8 " if dimensions.is_none() => Some(vp8_dimensions(payload)?),
            b"VP8L" if dimensions.is_none() => Some(vp8l_dimensions(payload)?),
            _ => return None,
        };
        if let Some(parsed) = parsed {
            dimensions = Some(parsed);
        }
        offset = end.checked_add(size & 1)?;
    }
    (offset == data.len()).then_some(dimensions).flatten()
}

fn webp_profile(bytes: &[u8]) -> Result<ImageProfile, String> {
    if bytes.len() < 12 || !bytes.starts_with(b"RIFF") || bytes.get(8..12) != Some(b"WEBP") {
        return Err(invalid_header());
    }
    let declared = usize::try_from(le_u32(&bytes[4..8]).ok_or_else(invalid_header)?)
        .map_err(|_| invalid_header())?;
    let end = declared.checked_add(8).ok_or_else(invalid_header)?;
    if end < 12 || end != bytes.len() {
        return Err(invalid_header());
    }
    let mut offset = 12usize;
    let mut dimensions = None;
    let mut has_vp8x = false;
    let mut animation_flag = false;
    let mut has_anim = false;
    let mut has_top_image = false;
    let mut frames = 0u32;
    while offset < end {
        let header = bytes.get(offset..offset + 8).ok_or_else(invalid_header)?;
        let tag = &header[..4];
        let size = usize::try_from(le_u32(&header[4..8]).ok_or_else(invalid_header)?)
            .map_err(|_| invalid_header())?;
        let data_start = offset + 8;
        let data_end = data_start.checked_add(size).ok_or_else(invalid_header)?;
        let data = bytes.get(data_start..data_end).ok_or_else(invalid_header)?;
        match tag {
            b"VP8X" => {
                if offset != 12
                    || has_vp8x
                    || size != 10
                    || data[0] & 0xc1 != 0
                    || data[1..4] != [0, 0, 0]
                {
                    return Err(invalid_header());
                }
                has_vp8x = true;
                animation_flag = data[0] & 0x02 != 0;
                let parsed = (
                    le_u24(&data[4..7]).ok_or_else(invalid_header)? + 1,
                    le_u24(&data[7..10]).ok_or_else(invalid_header)? + 1,
                );
                validate_profile(parsed.0, parsed.1, 1)?;
                dimensions = Some(parsed);
            }
            b"VP8 " => {
                let parsed = vp8_dimensions(data).ok_or_else(invalid_header)?;
                if has_top_image || animation_flag || (!has_vp8x && offset != 12) {
                    return Err(invalid_header());
                }
                validate_profile(parsed.0, parsed.1, 1)?;
                if has_vp8x && dimensions != Some(parsed) {
                    return Err(invalid_header());
                } else if !has_vp8x {
                    dimensions = Some(parsed);
                }
                has_top_image = true;
            }
            b"VP8L" => {
                let parsed = vp8l_dimensions(data).ok_or_else(invalid_header)?;
                if has_top_image || animation_flag || (!has_vp8x && offset != 12) {
                    return Err(invalid_header());
                }
                validate_profile(parsed.0, parsed.1, 1)?;
                if has_vp8x && dimensions != Some(parsed) {
                    return Err(invalid_header());
                } else if !has_vp8x {
                    dimensions = Some(parsed);
                }
                has_top_image = true;
            }
            b"ANIM" => {
                if !has_vp8x || !animation_flag || has_anim || has_top_image || size != 6 {
                    return Err(invalid_header());
                }
                has_anim = true;
            }
            b"ANMF" => {
                if !has_anim || !animation_flag || has_top_image || size < 16 {
                    return Err(invalid_header());
                }
                let (canvas_width, canvas_height) = dimensions.ok_or_else(invalid_header)?;
                let x = le_u24(&data[0..3]).ok_or_else(invalid_header)? * 2;
                let y = le_u24(&data[3..6]).ok_or_else(invalid_header)? * 2;
                let frame_width = le_u24(&data[6..9]).ok_or_else(invalid_header)? + 1;
                let frame_height = le_u24(&data[9..12]).ok_or_else(invalid_header)? + 1;
                let encoded = webp_frame_dimensions(&data[16..]).ok_or_else(invalid_header)?;
                validate_profile(encoded.0, encoded.1, 1)?;
                if encoded != (frame_width, frame_height) {
                    return Err(invalid_header());
                }
                if x.checked_add(frame_width)
                    .is_none_or(|right| right > canvas_width)
                    || y.checked_add(frame_height)
                        .is_none_or(|bottom| bottom > canvas_height)
                {
                    return Err(invalid_header());
                }
                frames = frames.checked_add(1).ok_or_else(invalid_header)?;
                if frames > MAX_IMAGE_FRAMES {
                    return Err(unavailable("the image has more than 300 frames"));
                }
            }
            _ if has_vp8x => {}
            _ => return Err(invalid_header()),
        }
        offset = data_end.checked_add(size & 1).ok_or_else(invalid_header)?;
        if offset > end {
            return Err(invalid_header());
        }
    }
    let valid_payload = if animation_flag {
        has_anim && frames > 0 && !has_top_image
    } else {
        !has_anim && frames == 0 && has_top_image
    };
    if offset != end || !valid_payload {
        return Err(invalid_header());
    }
    let (width, height) = dimensions.ok_or_else(invalid_header)?;
    validate_profile(width, height, if animation_flag { frames } else { 1 })
}

fn inspect_image(kind: ImageKind, bytes: &[u8]) -> Result<ImageProfile, String> {
    match kind {
        ImageKind::Png => png_profile(bytes),
        ImageKind::Jpeg => jpeg_profile(bytes),
        ImageKind::Gif => gif_profile(bytes),
        ImageKind::Webp => webp_profile(bytes),
    }
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
struct RootDirectory {
    file: File,
    metadata: std::fs::Metadata,
}

#[cfg(unix)]
fn open_root_directory(path: &Path) -> Result<RootDirectory, String> {
    let encoded = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| unavailable("the conversation project folder is invalid"))?;
    #[cfg(target_os = "linux")]
    let flags = libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
    // SAFETY: encoded is NUL-terminated, and a successful descriptor is immediately owned by File.
    let descriptor = unsafe { libc::open(encoded.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(unavailable(
            "the conversation project folder cannot be opened safely",
        ));
    }
    // SAFETY: descriptor is fresh, valid, and has no other Rust owner.
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file
        .metadata()
        .map_err(|_| unavailable("the conversation project folder cannot be inspected"))?;
    if !metadata.is_dir() {
        return Err(unavailable(
            "the conversation project folder is not available",
        ));
    }
    Ok(RootDirectory { file, metadata })
}

#[cfg(target_os = "linux")]
fn open_beneath(root: &File, relative: &Path) -> Result<Option<File>, String> {
    let encoded = CString::new(relative.as_os_str().as_bytes())
        .map_err(|_| unavailable("the image path is invalid"))?;
    // libc marks open_how non-exhaustive, so initialize its known v0 fields through zeroed memory.
    // SAFETY: all-zero is a valid open_how v0 value; fields are then set before the syscall.
    let mut how = unsafe { std::mem::zeroed::<libc::open_how>() };
    how.flags = (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW) as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_MAGICLINKS | libc::RESOLVE_NO_XDEV;
    // SAFETY: root is a live directory FD, encoded/how point to initialized memory, and size is exact.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root.as_raw_fd(),
            encoded.as_ptr(),
            &how,
            std::mem::size_of::<libc::open_how>(),
        ) as libc::c_int
    };
    if descriptor >= 0 {
        // SAFETY: descriptor is fresh, valid, and has no other Rust owner.
        return Ok(Some(unsafe { File::from_raw_fd(descriptor) }));
    }
    match std::io::Error::last_os_error().raw_os_error() {
        // Old kernels and seccomp profiles can make openat2 unavailable. The caller retains the
        // canonical/O_NOFOLLOW checks and adds same-device plus single-hardlink enforcement.
        Some(code) if matches!(code, libc::ENOSYS | libc::EINVAL | libc::EPERM) => Ok(None),
        _ => Err(unavailable(
            "the file cannot be resolved beneath the conversation project",
        )),
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_beneath(_root: &File, _relative: &Path) -> Result<Option<File>, String> {
    Ok(None)
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
        && before.nlink() == after.nlink()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
}

#[cfg(unix)]
fn root_is_unchanged(root: &Path, opened: &std::fs::Metadata) -> bool {
    root.metadata()
        .map(|named| named.dev() == opened.dev() && named.ino() == opened.ino())
        .unwrap_or(false)
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
    if system_pseudo_path(&candidate) {
        return Err(unavailable("system and device files cannot be shown"));
    }
    let relative = if requested_path.is_absolute() {
        requested_path
            .strip_prefix(&root)
            .map(Path::to_path_buf)
            .map_err(|_| unavailable("the file is outside this conversation's project"))?
    } else {
        requested_path.to_path_buf()
    };
    if relative.as_os_str().is_empty() {
        return Err(unavailable("the path is not a regular file"));
    }
    let expected = expected_kind(&candidate)?;

    #[cfg(unix)]
    let root_directory = open_root_directory(&root)?;
    #[cfg(unix)]
    {
        if !root_is_unchanged(&root, &root_directory.metadata) {
            return Err(unavailable("the conversation project folder changed"));
        }
    }

    #[cfg(unix)]
    let mut file = match open_beneath(&root_directory.file, &relative)? {
        Some(file) => file,
        None => {
            let fallback = candidate
                .canonicalize()
                .map_err(|_| unavailable("the file was not found"))?;
            if !fallback.starts_with(&root) {
                return Err(unavailable(
                    "the file is outside this conversation's project",
                ));
            }
            open_regular(&fallback)?
        }
    };
    #[cfg(not(unix))]
    let mut file = {
        let fallback = candidate
            .canonicalize()
            .map_err(|_| unavailable("the file was not found"))?;
        if !fallback.starts_with(&root) {
            return Err(unavailable(
                "the file is outside this conversation's project",
            ));
        }
        open_regular(&fallback)?
    };

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
    let opened = file
        .metadata()
        .map_err(|_| unavailable("the file cannot be inspected"))?;
    if !opened.is_file() {
        return Err(unavailable("the path is not a regular file"));
    }
    if opened.len() > MAX_IMAGE_BYTES {
        return Err(unavailable("the image is larger than 20 MiB"));
    }
    #[cfg(unix)]
    {
        if opened.nlink() != 1 {
            return Err(unavailable("files with multiple hardlinks cannot be shown"));
        }
        if opened.dev() != root_directory.metadata.dev() {
            return Err(unavailable(
                "the file crosses the conversation project's device boundary",
            ));
        }
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
    inspect_image(expected, &bytes)?;

    // An in-place writer can keep the inode while changing the bytes. Require stable metadata
    // across the whole read, then verify once more that the name still resolves to this handle.
    let after_read = file
        .metadata()
        .map_err(|_| unavailable("the file changed while it was being read"))?;
    if !unchanged_file(&opened, &after_read) || after_read.len() != bytes.len() as u64 {
        return Err(unavailable("the file changed while it was being read"));
    }
    #[cfg(unix)]
    if after_read.nlink() != 1 || !root_is_unchanged(&root, &root_directory.metadata) {
        return Err(unavailable(
            "the file or conversation project folder changed",
        ));
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
    use std::fmt::Debug;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

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

    fn assert_refused<T: Debug>(result: Result<T, String>, words: &str) {
        let error = result.unwrap_err();
        assert!(error.contains(words), "{error:?} did not contain {words:?}");
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = b"\xff\xd8\xff\xc0\0\x0b\x08".to_vec();
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[1, 1, 0x11, 0]);
        bytes
    }

    fn gif(width: u16, height: u16, frames: usize) -> Vec<u8> {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0]);
        for _ in 0..frames {
            bytes.push(0x2c);
            bytes.extend_from_slice(&[0, 0, 0, 0]);
            bytes.extend_from_slice(&width.to_le_bytes());
            bytes.extend_from_slice(&height.to_le_bytes());
            bytes.extend_from_slice(&[0, 2, 1, 0, 0]);
        }
        bytes.push(0x3b);
        bytes
    }

    fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        for (tag, data) in chunks {
            body.extend_from_slice(*tag);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() & 1 != 0 {
                body.push(0);
            }
        }
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);
        bytes
    }

    fn webp_lossless(width: u32, height: u32) -> Vec<u8> {
        let bits = (width - 1) | ((height - 1) << 14);
        let mut payload = vec![0x2f];
        payload.extend_from_slice(&bits.to_le_bytes());
        riff(&[(b"VP8L", payload)])
    }

    fn webp_lossy(width: u16, height: u16) -> Vec<u8> {
        let mut payload = vec![0, 0, 0, 0x9d, 0x01, 0x2a];
        payload.extend_from_slice(&width.to_le_bytes());
        payload.extend_from_slice(&height.to_le_bytes());
        riff(&[(b"VP8 ", payload)])
    }

    fn vp8x_payload(width: u32, height: u32, flags: u8) -> Vec<u8> {
        let mut payload = vec![flags, 0, 0, 0];
        for dimension in [width - 1, height - 1] {
            payload.extend_from_slice(&dimension.to_le_bytes()[..3]);
        }
        payload
    }

    fn webp_extended_static(width: u32, height: u32) -> Vec<u8> {
        let lossy = webp_lossy(width as u16, height as u16);
        riff(&[
            (b"VP8X", vp8x_payload(width, height, 0)),
            (b"VP8 ", lossy[20..].to_vec()),
        ])
    }

    fn webp_animation_with_bitstream(
        width: u32,
        height: u32,
        frames: usize,
        bitstream: Option<(u32, u32)>,
    ) -> Vec<u8> {
        let vp8x = vp8x_payload(width, height, 0x02);
        let mut chunks = vec![(b"VP8X", vp8x), (b"ANIM", vec![0; 6])];
        chunks.extend((0..frames).map(|_| {
            let mut frame = vec![0; 6];
            for dimension in [width - 1, height - 1] {
                let bytes = dimension.to_le_bytes();
                frame.extend_from_slice(&bytes[..3]);
            }
            frame.extend_from_slice(&[0; 4]);
            if let Some((encoded_width, encoded_height)) = bitstream {
                let bits = (encoded_width - 1) | ((encoded_height - 1) << 14);
                frame.extend_from_slice(b"VP8L");
                frame.extend_from_slice(&5u32.to_le_bytes());
                frame.push(0x2f);
                frame.extend_from_slice(&bits.to_le_bytes());
                frame.push(0);
            }
            (b"ANMF", frame)
        }));
        riff(&chunks)
    }

    fn webp_animation(width: u32, height: u32, frames: usize) -> Vec<u8> {
        webp_animation_with_bitstream(width, height, frames, Some((width, height)))
    }

    #[test]
    fn reads_a_raster_inside_the_conversation_project() {
        let scratch = Scratch::new("inside");
        let project = scratch.0.join("project");
        std::fs::create_dir_all(project.join("shots")).unwrap();
        let png = png(2, 2);
        std::fs::write(project.join("shots/main.png"), &png).unwrap();
        let image = read_project_image(&project, "shots/main.png", &scratch.0).unwrap();
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.size_bytes, png.len() as u64);
        assert_eq!(image.data_base64, encode_base64(&png));
        assert!(Path::new(&image.source_path).starts_with(project.canonicalize().unwrap()));
        let mut changed = png;
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
        std::fs::write(other.join("secret.png"), png(2, 2)).unwrap();
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
        std::fs::write(other.join("secret.png"), png(2, 2)).unwrap();
        std::os::unix::fs::symlink(other.join("secret.png"), project.join("alias.png")).unwrap();
        assert_refused(
            read_project_image(&project, "alias.png", &scratch.0),
            "conversation project",
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_hardlink_to_an_image_outside_the_project() {
        let scratch = Scratch::new("hardlink");
        let project = scratch.0.join("project");
        let protected = scratch.0.join("home/.claude");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&protected).unwrap();
        let outside = protected.join("private.png");
        std::fs::write(&outside, png(2, 2)).unwrap();
        std::fs::hard_link(&outside, project.join("shown.png")).unwrap();
        assert_refused(
            read_project_image(&project, "shown.png", &scratch.0.join("home")),
            "multiple hardlinks",
        );
    }

    #[test]
    fn rejects_protected_home_and_system_files_even_under_a_broad_root() {
        let scratch = Scratch::new("protected");
        let home = scratch.0.join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".claude/screenshot.png"), png(2, 2)).unwrap();
        std::fs::write(home.join(".codex/screenshot.png"), png(2, 2)).unwrap();
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
            std::fs::write(vault.join("screenshot.png"), png(2, 2)).unwrap();
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

    #[test]
    fn validates_bounded_headers_for_every_supported_format() {
        assert_eq!(inspect_image(ImageKind::Png, &png(2, 3)).unwrap().width, 2);
        assert_eq!(
            inspect_image(ImageKind::Jpeg, &jpeg(4, 5)).unwrap().height,
            5
        );
        assert_eq!(
            inspect_image(ImageKind::Gif, &gif(6, 7, 2)).unwrap().frames,
            2
        );
        assert_eq!(
            inspect_image(ImageKind::Webp, &webp_lossless(8, 9))
                .unwrap()
                .height,
            9
        );
        assert_eq!(
            inspect_image(ImageKind::Webp, &webp_lossy(12, 13))
                .unwrap()
                .width,
            12
        );
        assert_eq!(
            inspect_image(ImageKind::Webp, &webp_extended_static(14, 15))
                .unwrap()
                .height,
            15
        );
        assert_eq!(
            inspect_image(ImageKind::Webp, &webp_animation(10, 11, 2))
                .unwrap()
                .frames,
            2
        );

        for (kind, bytes) in [
            (ImageKind::Png, b"\x89PNG\r\n\x1a\n".as_slice()),
            (ImageKind::Jpeg, b"\xff\xd8\xff\xd9".as_slice()),
            (ImageKind::Gif, b"GIF89a".as_slice()),
            (ImageKind::Webp, b"RIFF\0\0\0\0WEBP".as_slice()),
        ] {
            assert_refused(
                inspect_image(kind, bytes).map(|_| unreachable!()),
                "header is invalid",
            );
        }
        assert_refused(
            inspect_image(
                ImageKind::Webp,
                &webp_animation_with_bitstream(10, 11, 1, None),
            )
            .map(|_| unreachable!()),
            "header is invalid",
        );
        let oversized = webp_lossless(10_000, 5_000);
        let small = webp_lossless(1, 1);
        assert_refused(
            inspect_image(
                ImageKind::Webp,
                &riff(&[
                    (b"VP8L", oversized[20..].to_vec()),
                    (b"VP8L", small[20..].to_vec()),
                ]),
            )
            .map(|_| unreachable!()),
            "dimensions exceed",
        );
        assert_refused(
            inspect_image(
                ImageKind::Webp,
                &riff(&[
                    (b"VP8L", oversized[20..].to_vec()),
                    (b"VP8X", vp8x_payload(1, 1, 0)),
                ]),
            )
            .map(|_| unreachable!()),
            "dimensions exceed",
        );
        assert_refused(
            inspect_image(
                ImageKind::Webp,
                &webp_animation_with_bitstream(10, 11, 1, Some((20, 22))),
            )
            .map(|_| unreachable!()),
            "header is invalid",
        );
    }

    #[test]
    fn rejects_excessive_dimensions_pixels_and_frame_counts() {
        for (kind, bytes) in [
            (ImageKind::Png, png(16_385, 1)),
            (ImageKind::Jpeg, jpeg(20_000, 1)),
            (ImageKind::Gif, gif(20_000, 1, 1)),
            (ImageKind::Webp, webp_lossless(10_000, 5_000)),
        ] {
            assert_refused(
                inspect_image(kind, &bytes).map(|_| unreachable!()),
                "dimensions exceed",
            );
        }
        assert_refused(
            inspect_image(ImageKind::Gif, &gif(1, 1, 301)).map(|_| unreachable!()),
            "more than 300 frames",
        );
        assert_refused(
            inspect_image(ImageKind::Webp, &webp_animation(1, 1, 301)).map(|_| unreachable!()),
            "more than 300 frames",
        );
    }
}
