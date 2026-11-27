//! A single policy for images the conversation asks the native window to read.
//!
//! The WebView supplies a path, but never an authorization root. The Tauri adapter obtains the
//! requesting session's cwd from the engine and passes it here. Canonical containment is checked
//! before and after opening, the opened inode must still be the canonical path, and only bounded
//! regular raster files are returned.

use serde::Serialize;
use std::{
    ffi::CString,
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{ffi::OsStrExt, fs::MetadataExt},
};

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_IMAGE_FRAMES: u32 = 300;
const MAX_DECODED_PIXELS: u64 = 100_000_000;
const MAX_ANIMATION_PIXELS_PER_SECOND: u64 = 200_000_000;
const MIN_FRAME_DELAY_NS: u64 = 20_000_000;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAsset {
    pub data_base64: String,
    pub mime_type: &'static str,
    pub size_bytes: u64,
    pub source_path: String,
    pub file_revision: String,
    pub decode_profile: ImageDecodeProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectRootIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDecodeProfile {
    pub canvas_pixels: u64,
    pub decoded_frame_pixels: u64,
    pub animation_pixels_per_second: u64,
    pub frame_count: u32,
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

fn unavailable(reason: impl AsRef<str>) -> String {
    format!("Image unavailable: {}.", reason.as_ref())
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

fn validate_profile(width: u32, height: u32, frames: u32) -> Result<ImageDecodeProfile, String> {
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
    let canvas_pixels = u64::from(width) * u64::from(height);
    Ok(ImageDecodeProfile {
        canvas_pixels,
        decoded_frame_pixels: canvas_pixels,
        animation_pixels_per_second: 0,
        frame_count: frames,
    })
}

fn add_frame_pixels(total: &mut u64, width: u32, height: u32) -> Result<(), String> {
    validate_profile(width, height, 1)?;
    *total = total
        .checked_add(u64::from(width) * u64::from(height))
        .ok_or_else(invalid_header)?;
    if *total > MAX_DECODED_PIXELS {
        return Err(unavailable(
            "the image exceeds the 100 million decoded-pixel budget",
        ));
    }
    Ok(())
}

fn add_frame_delay(total_ns: &mut u64, delay_ns: u64) -> Result<(), String> {
    *total_ns = total_ns
        .checked_add(delay_ns.max(MIN_FRAME_DELAY_NS))
        .ok_or_else(invalid_header)?;
    Ok(())
}

fn fraction_delay_ns(numerator: u16, denominator: u16) -> u64 {
    let denominator = u64::from(if denominator == 0 { 100 } else { denominator });
    (u64::from(numerator) * NANOS_PER_SECOND / denominator).max(MIN_FRAME_DELAY_NS)
}

fn milliseconds_delay_ns(milliseconds: u64) -> u64 {
    milliseconds
        .saturating_mul(1_000_000)
        .max(MIN_FRAME_DELAY_NS)
}

fn animation_rate(decoded_pixels: u64, duration_ns: u64) -> Result<u64, String> {
    if u128::from(decoded_pixels) * u128::from(NANOS_PER_SECOND)
        > u128::from(MAX_ANIMATION_PIXELS_PER_SECOND) * u128::from(duration_ns)
    {
        return Err(unavailable(
            "the animation exceeds 200 million frame-pixels per second",
        ));
    }
    let rate = (u128::from(decoded_pixels) * u128::from(NANOS_PER_SECOND))
        .div_ceil(u128::from(duration_ns));
    u64::try_from(rate).map_err(|_| invalid_header())
}

fn finalize_profile(
    width: u32,
    height: u32,
    frames: u32,
    decoded_frame_pixels: u64,
    animation_duration_ns: u64,
    animated: bool,
) -> Result<ImageDecodeProfile, String> {
    let mut profile = validate_profile(width, height, frames)?;
    if decoded_frame_pixels == 0 || decoded_frame_pixels > MAX_DECODED_PIXELS {
        return Err(invalid_header());
    }
    profile.decoded_frame_pixels = decoded_frame_pixels;
    if animated {
        profile.animation_pixels_per_second =
            animation_rate(decoded_frame_pixels, animation_duration_ns)?;
    }
    Ok(profile)
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

fn png_profile(bytes: &[u8]) -> Result<ImageDecodeProfile, String> {
    if bytes.len() < 45 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(invalid_header());
    }
    let mut offset = 8usize;
    let mut canvas = None;
    let mut has_idat = false;
    let mut has_iend = false;
    let mut animation_frames = None;
    let mut frame_count = 0u32;
    let mut decoded_pixels = 0u64;
    let mut animation_duration_ns = 0u64;
    let mut expected_sequence = 0u32;
    let mut current_fdat_frame = false;
    let mut current_frame_has_data = false;

    while offset < bytes.len() {
        let header = bytes.get(offset..offset + 8).ok_or_else(invalid_header)?;
        let length = usize::try_from(be_u32(&header[..4]).ok_or_else(invalid_header)?)
            .map_err(|_| invalid_header())?;
        let data_start = offset.checked_add(8).ok_or_else(invalid_header)?;
        let data_end = data_start.checked_add(length).ok_or_else(invalid_header)?;
        let chunk_end = data_end.checked_add(4).ok_or_else(invalid_header)?;
        let data = bytes.get(data_start..data_end).ok_or_else(invalid_header)?;
        if chunk_end > bytes.len() {
            return Err(invalid_header());
        }
        match &header[4..8] {
            b"IHDR" => {
                if offset != 8 || length != 13 || canvas.is_some() {
                    return Err(invalid_header());
                }
                let width = be_u32(&data[0..4]).ok_or_else(invalid_header)?;
                let height = be_u32(&data[4..8]).ok_or_else(invalid_header)?;
                let bit_depth = data[8];
                let color_type = data[9];
                let legal_depth = matches!(
                    (color_type, bit_depth),
                    (0, 1 | 2 | 4 | 8 | 16)
                        | (2, 8 | 16)
                        | (3, 1 | 2 | 4 | 8)
                        | (4, 8 | 16)
                        | (6, 8 | 16)
                );
                if !legal_depth || data[10] != 0 || data[11] != 0 || data[12] > 1 {
                    return Err(invalid_header());
                }
                validate_profile(width, height, 1)?;
                canvas = Some((width, height));
            }
            b"acTL" => {
                if canvas.is_none() || has_idat || animation_frames.is_some() || length != 8 {
                    return Err(invalid_header());
                }
                let frames = be_u32(&data[..4]).ok_or_else(invalid_header)?;
                if frames == 0 {
                    return Err(invalid_header());
                }
                if frames > MAX_IMAGE_FRAMES {
                    return Err(unavailable("the image has more than 300 frames"));
                }
                animation_frames = Some(frames);
            }
            b"fcTL" => {
                let (canvas_width, canvas_height) = canvas.ok_or_else(invalid_header)?;
                if animation_frames.is_none()
                    || length != 26
                    || (current_fdat_frame && !current_frame_has_data)
                {
                    return Err(invalid_header());
                }
                let sequence = be_u32(&data[..4]).ok_or_else(invalid_header)?;
                if sequence != expected_sequence {
                    return Err(invalid_header());
                }
                expected_sequence = expected_sequence
                    .checked_add(1)
                    .ok_or_else(invalid_header)?;
                let width = be_u32(&data[4..8]).ok_or_else(invalid_header)?;
                let height = be_u32(&data[8..12]).ok_or_else(invalid_header)?;
                let x = be_u32(&data[12..16]).ok_or_else(invalid_header)?;
                let y = be_u32(&data[16..20]).ok_or_else(invalid_header)?;
                if x.checked_add(width)
                    .is_none_or(|right| right > canvas_width)
                    || y.checked_add(height)
                        .is_none_or(|bottom| bottom > canvas_height)
                    || data[24] > 2
                    || data[25] > 1
                {
                    return Err(invalid_header());
                }
                add_frame_pixels(&mut decoded_pixels, width, height)?;
                let delay_num = be_u16(&data[20..22]).ok_or_else(invalid_header)?;
                let delay_den = be_u16(&data[22..24]).ok_or_else(invalid_header)?;
                add_frame_delay(
                    &mut animation_duration_ns,
                    fraction_delay_ns(delay_num, delay_den),
                )?;
                frame_count = frame_count.checked_add(1).ok_or_else(invalid_header)?;
                if !has_idat {
                    if frame_count != 1
                        || width != canvas_width
                        || height != canvas_height
                        || x != 0
                        || y != 0
                    {
                        return Err(invalid_header());
                    }
                    current_fdat_frame = false;
                } else {
                    current_fdat_frame = true;
                    current_frame_has_data = false;
                }
            }
            b"IDAT" => {
                if canvas.is_none() || has_iend {
                    return Err(invalid_header());
                }
                has_idat = true;
            }
            b"fdAT" => {
                if animation_frames.is_none() || !has_idat || !current_fdat_frame || length < 5 {
                    return Err(invalid_header());
                }
                let sequence = be_u32(&data[..4]).ok_or_else(invalid_header)?;
                if sequence != expected_sequence {
                    return Err(invalid_header());
                }
                expected_sequence = expected_sequence
                    .checked_add(1)
                    .ok_or_else(invalid_header)?;
                current_frame_has_data = true;
            }
            b"IEND" => {
                if length != 0
                    || !has_idat
                    || has_iend
                    || (current_fdat_frame && !current_frame_has_data)
                {
                    return Err(invalid_header());
                }
                has_iend = true;
            }
            _ => {
                if canvas.is_none() || has_iend {
                    return Err(invalid_header());
                }
            }
        }
        offset = chunk_end;
        if has_iend && offset != bytes.len() {
            return Err(invalid_header());
        }
    }

    let (width, height) = canvas.ok_or_else(invalid_header)?;
    if !has_iend {
        return Err(invalid_header());
    }
    let animated = animation_frames.is_some();
    let frames = if let Some(declared) = animation_frames {
        if frame_count != declared {
            return Err(invalid_header());
        }
        declared
    } else {
        add_frame_pixels(&mut decoded_pixels, width, height)?;
        1
    };
    finalize_profile(
        width,
        height,
        frames,
        decoded_pixels,
        animation_duration_ns,
        animated,
    )
}

fn jpeg_profile(bytes: &[u8]) -> Result<ImageDecodeProfile, String> {
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

fn gif_profile(bytes: &[u8]) -> Result<ImageDecodeProfile, String> {
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
    let mut decoded_pixels = 0u64;
    let mut animation_duration_ns = 0u64;
    let mut pending_delay_ns = None;
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
                let label = *bytes.get(offset).ok_or_else(invalid_header)?;
                offset += 1;
                if label == 0xf9 {
                    if pending_delay_ns.is_some() {
                        return Err(invalid_header());
                    }
                    let control = bytes.get(offset..offset + 6).ok_or_else(invalid_header)?;
                    if control[0] != 4 || control[5] != 0 {
                        return Err(invalid_header());
                    }
                    let delay_cs = le_u16(&control[2..4]).ok_or_else(invalid_header)?;
                    pending_delay_ns = Some(milliseconds_delay_ns(u64::from(delay_cs) * 10));
                    offset += 6;
                } else {
                    skip_gif_sub_blocks(bytes, &mut offset)?;
                    // A Graphic Control Extension applies to the next rendering block. A plain
                    // text extension is such a block, so its delay must not leak to a later image.
                    if label == 0x01 {
                        pending_delay_ns = None;
                    }
                }
            }
            0x2c => {
                let descriptor = bytes.get(offset..offset + 9).ok_or_else(invalid_header)?;
                let frame_width = u32::from(le_u16(&descriptor[4..6]).ok_or_else(invalid_header)?);
                let frame_height = u32::from(le_u16(&descriptor[6..8]).ok_or_else(invalid_header)?);
                add_frame_pixels(&mut decoded_pixels, frame_width, frame_height)?;
                add_frame_delay(
                    &mut animation_duration_ns,
                    pending_delay_ns.take().unwrap_or(MIN_FRAME_DELAY_NS),
                )?;
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
    finalize_profile(
        width,
        height,
        frames,
        decoded_pixels,
        animation_duration_ns,
        frames > 1,
    )
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

fn webp_profile(bytes: &[u8]) -> Result<ImageDecodeProfile, String> {
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
    let mut decoded_pixels = 0u64;
    let mut animation_duration_ns = 0u64;
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
                add_frame_pixels(&mut decoded_pixels, parsed.0, parsed.1)?;
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
                add_frame_pixels(&mut decoded_pixels, parsed.0, parsed.1)?;
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
                let duration_ms = le_u24(&data[12..15]).ok_or_else(invalid_header)?;
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
                add_frame_pixels(&mut decoded_pixels, frame_width, frame_height)?;
                add_frame_delay(
                    &mut animation_duration_ns,
                    milliseconds_delay_ns(u64::from(duration_ms)),
                )?;
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
    finalize_profile(
        width,
        height,
        if animation_flag { frames } else { 1 },
        decoded_pixels,
        animation_duration_ns,
        animation_flag,
    )
}

fn inspect_image(kind: ImageKind, bytes: &[u8]) -> Result<ImageDecodeProfile, String> {
    match kind {
        ImageKind::Png => png_profile(bytes),
        ImageKind::Jpeg => jpeg_profile(bytes),
        ImageKind::Gif => gif_profile(bytes),
        ImageKind::Webp => webp_profile(bytes),
    }
}

fn protected_home_path(path: &Path, home: &Path) -> bool {
    if path.components().any(|component| {
        matches!(component, Component::Normal(name) if name == ".claude" || name == ".codex")
    }) {
        return true;
    }
    let canonical_home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    path.starts_with(canonical_home.join(".claude"))
        || path.starts_with(canonical_home.join(".codex"))
}

fn system_pseudo_path(path: &Path) -> bool {
    ["/proc", "/sys", "/dev"]
        .iter()
        .any(|root| path.starts_with(root))
}

#[cfg(target_os = "linux")]
struct RootDirectory {
    file: File,
    metadata: std::fs::Metadata,
}

#[cfg(target_os = "linux")]
fn open_root_directory(
    path: &Path,
    expected: ProjectRootIdentity,
) -> Result<RootDirectory, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(unavailable(
            "the conversation project folder path must be absolute and normalized",
        ));
    }
    let slash = CString::new("/").expect("slash contains no NUL");
    // SAFETY: slash is NUL-terminated, and a successful descriptor is immediately owned by File.
    let slash_descriptor = unsafe {
        libc::open(
            slash.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if slash_descriptor < 0 {
        return Err(unavailable("the filesystem root cannot be opened safely"));
    }
    // SAFETY: descriptor is fresh, valid, and has no other Rust owner.
    let slash_file = unsafe { File::from_raw_fd(slash_descriptor) };
    let relative = path
        .strip_prefix(Path::new("/"))
        .map_err(|_| unavailable("the conversation project folder path is invalid"))?;
    let relative = if relative.as_os_str().is_empty() {
        Path::new(".")
    } else {
        relative
    };
    let encoded = CString::new(relative.as_os_str().as_bytes())
        .map_err(|_| unavailable("the conversation project folder is invalid"))?;
    // libc marks open_how non-exhaustive, so initialize its known v0 fields through zeroed memory.
    // SAFETY: all-zero is a valid open_how v0 value; fields are then set before the syscall.
    let mut how = unsafe { std::mem::zeroed::<libc::open_how>() };
    how.flags = (libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
    // SAFETY: slash_file is a live directory FD and encoded/how point to initialized memory.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            slash_file.as_raw_fd(),
            encoded.as_ptr(),
            &how,
            std::mem::size_of::<libc::open_how>(),
        ) as libc::c_int
    };
    if descriptor < 0 {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ELOOP) => Err(unavailable(
                "the conversation project folder path contains a symbolic link",
            )),
            Some(code) if matches!(code, libc::ENOSYS | libc::EINVAL | libc::EPERM) => Err(
                unavailable("safe project-folder resolution is unavailable on this system"),
            ),
            _ => Err(unavailable(
                "the conversation project folder cannot be opened safely",
            )),
        };
    }
    // SAFETY: descriptor is fresh, valid, and has no other Rust owner.
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file
        .metadata()
        .map_err(|_| unavailable("the conversation project folder cannot be inspected"))?;
    if !metadata.is_dir() || metadata.nlink() == 0 {
        return Err(unavailable(
            "the conversation project folder is not available",
        ));
    }
    if metadata.dev() != expected.device || metadata.ino() != expected.inode {
        return Err(unavailable(
            "the conversation project folder was replaced before it could be opened",
        ));
    }
    Ok(RootDirectory { file, metadata })
}

#[cfg(target_os = "linux")]
pub(crate) fn open_project_directory(
    root: &Path,
    identity: ProjectRootIdentity,
) -> Result<File, String> {
    Ok(open_root_directory(root, identity)?.file)
}

#[cfg(target_os = "linux")]
pub(crate) fn descriptor_path(file: &File, kind: &str) -> Result<PathBuf, String> {
    let path = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|_| unavailable(format!("the {kind} descriptor cannot be resolved")))?;
    if !path.is_absolute() {
        return Err(unavailable(format!(
            "the {kind} descriptor no longer names an available path"
        )));
    }
    Ok(path)
}

#[cfg(target_os = "linux")]
fn open_beneath(root: &File, relative: &Path) -> Result<File, String> {
    open_relative(
        root,
        relative,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW,
        0,
    )
    .map_err(|error| match error.raw_os_error() {
        Some(libc::ENOSYS | libc::EINVAL | libc::EPERM) => {
            unavailable("safe descriptor-relative image resolution is unavailable on this system")
        }
        _ if error.kind() == std::io::ErrorKind::InvalidInput => {
            unavailable("the image path is invalid")
        }
        _ => unavailable("the file cannot be resolved beneath the conversation project"),
    })
}

/// Open a name relative to the checked project directory. Metadata-only handles let the caller
/// distinguish regular files from devices and FIFOs before opening a stream for reading.
#[cfg(target_os = "linux")]
pub(crate) fn open_project_entry(root: &File, relative: &Path) -> std::io::Result<File> {
    open_relative(
        root,
        relative,
        libc::O_PATH | libc::O_CLOEXEC,
        libc::RESOLVE_NO_SYMLINKS,
    )
}

#[cfg(target_os = "linux")]
fn open_relative(
    root: &File,
    relative: &Path,
    flags: libc::c_int,
    extra_resolve: u64,
) -> std::io::Result<File> {
    let encoded = CString::new(relative.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    // libc marks open_how non-exhaustive, so initialize its known v0 fields through zeroed memory.
    // SAFETY: all-zero is a valid open_how v0 value; fields are then set before the syscall.
    let mut how = unsafe { std::mem::zeroed::<libc::open_how>() };
    how.flags = flags as u64;
    how.resolve =
        libc::RESOLVE_BENEATH | libc::RESOLVE_NO_MAGICLINKS | libc::RESOLVE_NO_XDEV | extra_resolve;
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
        return Ok(unsafe { File::from_raw_fd(descriptor) });
    }
    Err(std::io::Error::last_os_error())
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

fn content_fingerprint(bytes: &[u8]) -> u64 {
    // The revision is a cache validator, not an authenticity primitive. Hashing every returned
    // byte prevents an in-place rewrite with restored timestamps from reusing a stale Blob URL.
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(target_os = "linux")]
fn read_project_file<T>(
    root_directory: &RootDirectory,
    requested: &str,
    home: &Path,
    private: &crate::project_files::PrivateDirectory,
    max_bytes: u64,
    check_type: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<(Vec<u8>, PathBuf, std::fs::Metadata, T), String> {
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

    let root = descriptor_path(&root_directory.file, "conversation project folder")?;
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    };
    // Check the lexical spelling as well as the resolved target. Otherwise a protected settings
    // directory that is itself a symlink could lose its `.claude` / `.codex` identity on realpath.
    if protected_home_path(&candidate, home) || private.contains(&candidate) {
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
    let expected = check_type(&candidate)?;

    let mut file = open_beneath(&root_directory.file, &relative)?;
    let canonical = descriptor_path(&file, "image file")?;
    if !canonical.starts_with(&root) {
        return Err(unavailable(
            "the file is outside this conversation's project",
        ));
    }
    if protected_home_path(&canonical, home) || private.contains(&canonical) {
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
    if opened.len() > max_bytes {
        return Err(unavailable(format!(
            "the file is larger than {} MiB",
            max_bytes / (1024 * 1024)
        )));
    }
    if opened.nlink() != 1 {
        return Err(unavailable("files with multiple hardlinks cannot be shown"));
    }
    if opened.dev() != root_directory.metadata.dev() {
        return Err(unavailable(
            "the file crosses the conversation project's device boundary",
        ));
    }

    let mut bytes = Vec::new();
    (&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable("the file could not be read"))?;
    if bytes.is_empty() || bytes.len() as u64 > max_bytes {
        return Err(unavailable(
            "the file is empty or larger than the preview limit",
        ));
    }

    // An in-place writer can keep the inode while changing the bytes. Require stable metadata
    // across the whole read, then verify once more that the name still resolves to this handle.
    let after_read = file
        .metadata()
        .map_err(|_| unavailable("the file changed while it was being read"))?;
    if !unchanged_file(&opened, &after_read) || after_read.len() != bytes.len() as u64 {
        return Err(unavailable("the file changed while it was being read"));
    }
    let root_after = root_directory
        .file
        .metadata()
        .map_err(|_| unavailable("the conversation project folder changed"))?;
    if after_read.nlink() != 1
        || root_after.nlink() == 0
        || root_after.dev() != root_directory.metadata.dev()
        || root_after.ino() != root_directory.metadata.ino()
        || descriptor_path(&root_directory.file, "conversation project folder")? != root
    {
        return Err(unavailable(
            "the file or conversation project folder changed",
        ));
    }
    let checked_after = descriptor_path(&file, "image file")?;
    if checked_after != canonical || !checked_after.starts_with(&root) {
        return Err(unavailable("the file changed while it was being read"));
    }
    Ok((bytes, canonical, after_read, expected))
}

#[cfg(target_os = "linux")]
fn read_project_image_from_root(
    root_directory: &RootDirectory,
    requested: &str,
    home: &Path,
) -> Result<ImageAsset, String> {
    let (bytes, canonical, after_read, expected) = read_project_file(
        root_directory,
        requested,
        home,
        &crate::project_files::PrivateDirectory::default(),
        MAX_IMAGE_BYTES,
        expected_kind,
    )?;
    if sniff_kind(&bytes) != Some(expected) {
        return Err(unavailable("the file contents do not match its image type"));
    }
    let decode_profile = inspect_image(expected, &bytes)?;
    let fingerprint = content_fingerprint(&bytes);

    let revision = format!(
        "{}:{}:{}:{}:{}:{fingerprint:016x}",
        after_read.dev(),
        after_read.ino(),
        bytes.len(),
        after_read.mtime(),
        after_read.mtime_nsec()
    );
    Ok(ImageAsset {
        data_base64: encode_base64(&bytes),
        mime_type: expected.mime_type(),
        size_bytes: bytes.len() as u64,
        source_path: canonical.to_string_lossy().into_owned(),
        file_revision: revision,
        decode_profile,
    })
}

/// Validate an absolute raster outside the project before using its parent as the read root.
#[cfg(target_os = "linux")]
pub(crate) fn external_image_path(path: &str) -> Result<&Path, String> {
    let candidate = Path::new(path);
    if path.len() > 4096
        || !candidate.is_absolute()
        || candidate.file_name().is_none()
        || path.chars().any(|ch| {
            ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        || candidate
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(unavailable("the external image path is invalid"));
    }
    expected_kind(candidate)?;
    Ok(candidate)
}

#[cfg(target_os = "linux")]
pub(crate) fn read_external_image(path: &str, home: &Path) -> Result<ImageAsset, String> {
    let candidate = external_image_path(path)?;
    // Reject protected paths before inspecting the containing folder.
    if protected_home_path(candidate, home) || system_pseudo_path(candidate) {
        return Err(unavailable("protected files cannot be shown"));
    }
    let parent = candidate
        .parent()
        .ok_or_else(|| unavailable("the image has no containing folder"))?;
    let identity = std::fs::symlink_metadata(parent)
        .map_err(|_| unavailable("the image folder cannot be opened"))?;
    read_project_image(
        parent,
        ProjectRootIdentity {
            device: identity.dev(),
            inode: identity.ino(),
        },
        path,
        home,
    )
}

/// Read a raster for a known conversation, including absolute paths to artifacts outside it.
#[cfg(target_os = "linux")]
pub fn read_chat_image(
    project_root: &Path,
    expected_root: ProjectRootIdentity,
    requested: &str,
    home: &Path,
) -> Result<ImageAsset, String> {
    let root = open_root_directory(project_root, expected_root)?;
    let path = Path::new(requested);
    if path.is_absolute()
        && !path.starts_with(descriptor_path(&root.file, "conversation project folder")?)
        && !path.starts_with(project_root)
    {
        return read_external_image(requested, home);
    }
    read_project_image_from_root(&root, requested, home)
}

#[cfg(not(target_os = "linux"))]
pub use read_project_image as read_chat_image;

/// Read an explicit HTML artifact through the same root and file checks as a project image.
#[cfg(target_os = "linux")]
pub(crate) fn read_project_html(
    project_root: &Path,
    expected_root: ProjectRootIdentity,
    requested: &str,
    home: &Path,
    private: &crate::project_files::PrivateDirectory,
) -> Result<String, String> {
    let root = open_root_directory(project_root, expected_root)?;
    let html = |path: &Path| {
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
            })
        {
            Ok(())
        } else {
            Err(unavailable("the file must have an .html or .htm extension"))
        }
    };
    let (bytes, _, _, ()) =
        read_project_file(&root, requested, home, private, 2 * 1024 * 1024, html)?;
    String::from_utf8(bytes).map_err(|_| unavailable("the document must be UTF-8"))
}

/// Read one image after binding it to the requesting conversation's authoritative project root.
#[cfg(target_os = "linux")]
pub fn read_project_image(
    project_root: &Path,
    expected_root: ProjectRootIdentity,
    requested: &str,
    home: &Path,
) -> Result<ImageAsset, String> {
    let root_directory = open_root_directory(project_root, expected_root)?;
    read_project_image_from_root(&root_directory, requested, home)
}

/// Image reads are fail-closed where descriptor-relative resolution is not implemented.
#[cfg(not(target_os = "linux"))]
pub fn read_project_image(
    _project_root: &Path,
    _expected_root: ProjectRootIdentity,
    _requested: &str,
    _home: &Path,
) -> Result<ImageAsset, String> {
    Err(unavailable(
        "safe descriptor-relative image reads are unsupported on this platform",
    ))
}

#[cfg(all(feature = "desktop", target_os = "linux"))]
pub(crate) fn encode_clipboard_image(bytes: &[u8], mime: &str) -> Result<String, String> {
    let kind = sniff_kind(bytes)
        .filter(|kind| kind.mime_type() == mime)
        .ok_or_else(|| unavailable("the clipboard contents do not match the image type"))?;
    inspect_image(kind, bytes)?;
    Ok(encode_base64(bytes))
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
            // Keep security-path tests off TMPDIR: CI may point it at /dev/shm (correctly
            // rejected as a device tree) or through a symlink. The running Cargo test binary
            // already lives under the selected target directory; canonicalizing it gives the
            // same ordinary, symlink-free filesystem the build uses.
            let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
            let target_profile = executable.parent().unwrap().parent().unwrap();
            let base = target_profile.join("hoh-image-test-scratch");
            std::fs::create_dir_all(&base).unwrap();
            let base = base.canonicalize().unwrap();
            let path = base.join(format!(
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

    fn project_identity(path: &Path) -> ProjectRootIdentity {
        #[cfg(unix)]
        {
            let metadata = std::fs::metadata(path).unwrap();
            ProjectRootIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            ProjectRootIdentity {
                device: 0,
                inode: 0,
            }
        }
    }

    fn read_project_image(
        project_root: &Path,
        requested: &str,
        home: &Path,
    ) -> Result<ImageAsset, String> {
        super::read_project_image(
            project_root,
            project_identity(project_root),
            requested,
            home,
        )
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn html_uses_the_project_boundary_and_requires_a_utf8_document() {
        let scratch = Scratch::new("html");
        let root = scratch.0.join("project");
        std::fs::create_dir(&root).unwrap();
        let identity = project_identity(&root);
        std::fs::write(
            root.join("report.html"),
            b"<details><summary>Overview</summary>Body</details>",
        )
        .unwrap();
        std::fs::write(root.join("invalid.html"), [0xff]).unwrap();
        std::fs::write(root.join("notes.txt"), "<p>plain text file</p>").unwrap();
        std::fs::write(scratch.0.join("other.html"), "<p>outside</p>").unwrap();
        let secret_dir = root.join("private");
        std::fs::create_dir(&secret_dir).unwrap();
        std::fs::write(secret_dir.join("secret"), "<p>synthetic credential</p>").unwrap();
        std::os::unix::fs::symlink(secret_dir.join("secret"), root.join("alias.html")).unwrap();
        let private = crate::project_files::PrivateDirectory::new(&secret_dir);
        assert!(
            read_project_html(&root, identity, "report.html", &scratch.0, &private)
                .unwrap()
                .contains("Overview")
        );
        for file in ["../other.html", "invalid.html", "notes.txt", "alias.html"] {
            assert!(
                read_project_html(&root, identity, file, &scratch.0, &private).is_err(),
                "{file}"
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn descriptor_relative_reads_fail_closed_on_unsupported_platforms() {
        assert_refused(
            read_project_image(Path::new("."), "image.png", Path::new(".")),
            "unsupported on this platform",
        );
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        png_chunk(&mut bytes, b"IHDR", &ihdr);
        png_chunk(&mut bytes, b"IDAT", &[0]);
        png_chunk(&mut bytes, b"IEND", &[]);
        bytes
    }

    fn png_chunk(bytes: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(tag);
        bytes.extend_from_slice(data);
        // The bounded parser deliberately does not perform decompression or CRC validation.
        bytes.extend_from_slice(&[0; 4]);
    }

    fn apng(width: u32, height: u32, frames: usize) -> Vec<u8> {
        apng_with_delay(width, height, frames, 1, 10)
    }

    fn apng_with_delay(
        width: u32,
        height: u32,
        frames: usize,
        delay_num: u16,
        delay_den: u16,
    ) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        png_chunk(&mut bytes, b"IHDR", &ihdr);
        let mut actl = Vec::new();
        actl.extend_from_slice(&(frames as u32).to_be_bytes());
        actl.extend_from_slice(&0u32.to_be_bytes());
        png_chunk(&mut bytes, b"acTL", &actl);
        let mut sequence = 0u32;
        for frame in 0..frames {
            let mut fctl = Vec::new();
            fctl.extend_from_slice(&sequence.to_be_bytes());
            sequence += 1;
            fctl.extend_from_slice(&width.to_be_bytes());
            fctl.extend_from_slice(&height.to_be_bytes());
            fctl.extend_from_slice(&[0; 8]);
            fctl.extend_from_slice(&delay_num.to_be_bytes());
            fctl.extend_from_slice(&delay_den.to_be_bytes());
            fctl.extend_from_slice(&[0, 0]);
            png_chunk(&mut bytes, b"fcTL", &fctl);
            if frame == 0 {
                png_chunk(&mut bytes, b"IDAT", &[0]);
            } else {
                let mut fdat = sequence.to_be_bytes().to_vec();
                sequence += 1;
                fdat.push(0);
                png_chunk(&mut bytes, b"fdAT", &fdat);
            }
        }
        png_chunk(&mut bytes, b"IEND", &[]);
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
        gif_with_delay(width, height, frames, 10)
    }

    fn gif_with_delay(width: u16, height: u16, frames: usize, delay_cs: u16) -> Vec<u8> {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0]);
        for _ in 0..frames {
            bytes.extend_from_slice(&[0x21, 0xf9, 4, 0]);
            bytes.extend_from_slice(&delay_cs.to_le_bytes());
            bytes.extend_from_slice(&[0, 0]);
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
        webp_animation_with_bitstream_and_duration(width, height, frames, bitstream, 100)
    }

    fn webp_animation_with_bitstream_and_duration(
        width: u32,
        height: u32,
        frames: usize,
        bitstream: Option<(u32, u32)>,
        duration_ms: u32,
    ) -> Vec<u8> {
        let vp8x = vp8x_payload(width, height, 0x02);
        let mut chunks = vec![(b"VP8X", vp8x), (b"ANIM", vec![0; 6])];
        chunks.extend((0..frames).map(|_| {
            let mut frame = vec![0; 6];
            for dimension in [width - 1, height - 1] {
                let bytes = dimension.to_le_bytes();
                frame.extend_from_slice(&bytes[..3]);
            }
            frame.extend_from_slice(&duration_ms.to_le_bytes()[..3]);
            frame.push(0);
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

    #[cfg(target_os = "linux")]
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
        assert_eq!(
            image.decode_profile,
            ImageDecodeProfile {
                canvas_pixels: 4,
                decoded_frame_pixels: 4,
                animation_pixels_per_second: 0,
                frame_count: 1,
            }
        );
        assert!(Path::new(&image.source_path).starts_with(project.canonicalize().unwrap()));
        let mut changed = png;
        *changed.last_mut().unwrap() ^= 1;
        std::fs::write(project.join("shots/main.png"), &changed).unwrap();
        let changed_image = read_project_image(&project, "shots/main.png", &scratch.0).unwrap();
        assert_ne!(changed_image.file_revision, image.file_revision);
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn chat_previews_absolute_rasters_outside_the_project_and_keeps_file_checks() {
        let scratch = Scratch::new("external-artifact");
        let project = scratch.0.join("project");
        let artifacts = scratch.0.join("artifacts");
        let protected = scratch.0.join(".codex");
        for folder in [&project, &artifacts, &protected] {
            std::fs::create_dir(folder).unwrap();
        }
        let raster = artifacts.join("shot.png");
        let private = protected.join("private.png");
        for path in [&raster, &private] {
            std::fs::write(path, png(2, 2)).unwrap();
        }
        let preview = |path: &Path| {
            read_chat_image(
                &project,
                project_identity(&project),
                path.to_str().unwrap(),
                &scratch.0,
            )
        };
        let image = preview(&raster).unwrap();
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(Path::new(&image.source_path), raster);
        assert_refused(preview(&private), "protected");
        let disguised = artifacts.join("disguised.png");
        std::fs::write(&disguised, "not a raster").unwrap();
        assert_refused(preview(&disguised), "do not match its image type");
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn an_open_root_descriptor_cannot_be_substituted_by_name() {
        let scratch = Scratch::new("root-substitution");
        let project = scratch.0.join("project");
        let held = scratch.0.join("held-project");
        std::fs::create_dir_all(&project).unwrap();
        let original = png(2, 2);
        std::fs::write(project.join("shown.png"), &original).unwrap();
        let root = open_root_directory(&project, project_identity(&project)).unwrap();

        std::fs::rename(&project, &held).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("shown.png"), png(3, 3)).unwrap();

        let image = read_project_image_from_root(&root, "shown.png", &scratch.0).unwrap();
        assert_eq!(image.data_base64, encode_base64(&original));
        assert!(Path::new(&image.source_path).starts_with(&held));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_a_project_directory_replaced_before_native_open() {
        let scratch = Scratch::new("root-pre-open-substitution");
        let project = scratch.0.join("project");
        let held = scratch.0.join("held-project");
        let replacement = scratch.0.join("replacement");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&replacement).unwrap();
        std::fs::write(project.join("shown.png"), png(2, 2)).unwrap();
        std::fs::write(replacement.join("private.png"), png(3, 3)).unwrap();
        let selected_identity = project_identity(&project);

        // Equivalent to an exchange for the checked name: the path now names a different ordinary
        // directory before native open, without a symlink for RESOLVE_NO_SYMLINKS to reject.
        std::fs::rename(&project, &held).unwrap();
        std::fs::rename(&replacement, &project).unwrap();

        assert_refused(
            super::read_project_image(&project, selected_identity, "private.png", &scratch.0),
            "replaced before it could be opened",
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_symlinks_in_any_project_root_component() {
        let scratch = Scratch::new("root-symlinks");
        let project = scratch.0.join("project");
        let held = scratch.0.join("held-project");
        let outside = scratch.0.join("outside");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("private.png"), png(2, 2)).unwrap();
        std::fs::rename(&project, &held).unwrap();
        std::os::unix::fs::symlink(&outside, &project).unwrap();
        assert_refused(
            read_project_image(&project, "private.png", &scratch.0),
            "symbolic link",
        );

        let real_parent = scratch.0.join("real-parent");
        std::fs::create_dir_all(real_parent.join("nested-project")).unwrap();
        std::fs::write(real_parent.join("nested-project/image.png"), png(2, 2)).unwrap();
        let linked_parent = scratch.0.join("linked-parent");
        std::os::unix::fs::symlink(&real_parent, &linked_parent).unwrap();
        assert_refused(
            read_project_image(
                &linked_parent.join("nested-project"),
                "image.png",
                &scratch.0,
            ),
            "symbolic link",
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn accepts_a_live_project_with_a_literal_deleted_suffix() {
        let scratch = Scratch::new("literal-deleted");
        let project = scratch.0.join("project (deleted)");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("image.png"), png(2, 2)).unwrap();
        let image = read_project_image(&project, "image.png", &scratch.0).unwrap();
        assert!(image.source_path.contains("project (deleted)"));
    }

    #[cfg(target_os = "linux")]
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
        assert_refused(
            read_project_image(Path::new("/"), "/proc/version", &home),
            "system and device files",
        );
        assert_refused(
            read_project_image(Path::new("/"), "/etc/passwd", &home),
            "not a supported",
        );
    }

    #[cfg(target_os = "linux")]
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
        assert_eq!(
            inspect_image(ImageKind::Png, &png(2, 3))
                .unwrap()
                .canvas_pixels,
            6
        );
        assert_eq!(
            inspect_image(ImageKind::Jpeg, &jpeg(4, 5))
                .unwrap()
                .decoded_frame_pixels,
            20
        );
        assert_eq!(
            inspect_image(ImageKind::Gif, &gif(6, 7, 2))
                .unwrap()
                .frame_count,
            2
        );
        assert_eq!(
            inspect_image(ImageKind::Webp, &webp_lossless(8, 9))
                .unwrap()
                .canvas_pixels,
            72
        );
        assert_eq!(
            inspect_image(ImageKind::Webp, &webp_lossy(12, 13))
                .unwrap()
                .canvas_pixels,
            156
        );
        assert_eq!(
            inspect_image(ImageKind::Webp, &webp_extended_static(14, 15))
                .unwrap()
                .canvas_pixels,
            210
        );
        let animated = inspect_image(ImageKind::Webp, &webp_animation(10, 11, 2)).unwrap();
        assert_eq!(animated.frame_count, 2);
        assert_eq!(animated.decoded_frame_pixels, 220);
        assert_eq!(animated.animation_pixels_per_second, 1_100);

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
        assert_refused(
            inspect_image(ImageKind::Png, &apng(1, 1, 301)).map(|_| unreachable!()),
            "more than 300 frames",
        );
    }

    #[test]
    fn rejects_aggregate_animated_decode_budgets() {
        for (kind, bytes) in [
            (ImageKind::Png, apng(8_000, 5_000, 3)),
            (ImageKind::Gif, gif(8_000, 5_000, 3)),
            (ImageKind::Webp, webp_animation(8_000, 5_000, 3)),
        ] {
            assert_refused(
                inspect_image(kind, &bytes).map(|_| unreachable!()),
                "100 million decoded-pixel budget",
            );
        }

        assert_eq!(
            inspect_image(ImageKind::Png, &apng(1_000, 1_000, 10))
                .unwrap()
                .frame_count,
            10
        );
        assert_eq!(
            inspect_image(ImageKind::Gif, &gif_with_delay(500, 500, 10, 10))
                .unwrap()
                .frame_count,
            10
        );
        assert_eq!(
            inspect_image(ImageKind::Webp, &webp_animation(1_000, 1_000, 10))
                .unwrap()
                .frame_count,
            10
        );
    }

    #[test]
    fn rejects_excessive_animation_pixel_rates_after_delay_clamping() {
        for (kind, bytes) in [
            (ImageKind::Png, apng_with_delay(5_000, 5_000, 2, 0, 100)),
            (ImageKind::Gif, gif_with_delay(5_000, 5_000, 2, 0)),
            (
                ImageKind::Webp,
                webp_animation_with_bitstream_and_duration(
                    5_000,
                    5_000,
                    2,
                    Some((5_000, 5_000)),
                    0,
                ),
            ),
        ] {
            assert_refused(
                inspect_image(kind, &bytes).map(|_| unreachable!()),
                "200 million frame-pixels per second",
            );
        }

        for (kind, bytes) in [
            (ImageKind::Png, apng_with_delay(5_000, 5_000, 1, 0, 100)),
            (
                ImageKind::Webp,
                webp_animation_with_bitstream_and_duration(
                    5_000,
                    5_000,
                    1,
                    Some((5_000, 5_000)),
                    0,
                ),
            ),
        ] {
            assert_refused(
                inspect_image(kind, &bytes).map(|_| unreachable!()),
                "200 million frame-pixels per second",
            );
        }

        assert_eq!(
            inspect_image(ImageKind::Gif, &gif_with_delay(500, 500, 10, 10))
                .unwrap()
                .frame_count,
            10
        );
    }
}
