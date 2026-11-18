//! Header inspection and bounded admission, with all raster work on blocking workers.
use crate::util::error;
use ::image::{ColorType, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use hoh_attachment::{AttachmentErrorCode as Code, ImageMediaType};
use hoh_error::{BoxError, HarnessError};
use std::io::Cursor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedImage {
    pub media_type: ImageMediaType,
    pub width: u64,
    pub height: u64,
    pub animated: bool,
    pub carries_metadata: bool,
    pub depth: String,
    pub space: String,
    pub has_alpha: bool,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct DecodedImageLimits {
    pub max_pixels: Option<u64>,
    pub max_dimension: Option<u64>,
}
pub fn encoded_alpha_is_compatible(source_has_alpha: Option<bool>, output: &DetectedImage) -> bool {
    source_has_alpha.is_none_or(|alpha| {
        alpha == output.has_alpha
            || alpha && !output.has_alpha && output.media_type == ImageMediaType::Webp
    })
}
pub async fn probe_image(data: &[u8]) -> Result<DetectedImage, BoxError> {
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || inspect(&data, None, false)).await?
}
pub async fn detect_image(
    data: &[u8],
    limits: Option<DecodedImageLimits>,
) -> Result<DetectedImage, BoxError> {
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || inspect(&data, limits, true)).await?
}
fn invalid(cause: impl Into<BoxError>) -> BoxError {
    error("Unsupported or malformed image data.", Code::InvalidImage)
        .with_cause(cause)
        .into()
}
fn reader(data: &[u8]) -> Result<ImageReader<Cursor<&[u8]>>, BoxError> {
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(invalid)?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif)
    ) {
        return Err(error("Unsupported or malformed image data.", Code::InvalidImage).into());
    }
    reader.no_limits();
    Ok(reader)
}
fn inspect(
    data: &[u8],
    limits: Option<DecodedImageLimits>,
    full: bool,
) -> Result<DetectedImage, BoxError> {
    let reader = reader(data)?;
    let format = reader.format();
    let mut decoder = reader.into_decoder().map_err(invalid)?;
    let (width, height) = decoder.dimensions();
    let color = decoder.color_type();
    let exif = decoder.exif_metadata().map_err(invalid)?;
    let orientation = exif
        .as_ref()
        .and_then(|bytes| exif::Reader::new().read_raw(bytes.clone()).ok())
        .and_then(|metadata| {
            metadata
                .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                .and_then(|field| field.value.get_uint(0))
        });
    let profile = decoder.icc_profile().map_err(invalid)?;
    let xmp = decoder.xmp_metadata().map_err(invalid)?;
    let facts = container_facts(data, format);
    let transposed = orientation.is_some_and(|value| value >= 5);
    let media_type = match format {
        Some(ImageFormat::Png) => ImageMediaType::Png,
        Some(ImageFormat::Jpeg) => ImageMediaType::Jpeg,
        Some(ImageFormat::WebP) => ImageMediaType::Webp,
        Some(ImageFormat::Gif) => ImageMediaType::Gif,
        _ => return Err(error("Unsupported or malformed image data.", Code::InvalidImage).into()),
    };
    let (depth, space) = match color {
        ColorType::L16 | ColorType::La16 => ("ushort", "grey16"),
        ColorType::Rgb16 | ColorType::Rgba16 => ("ushort", "rgb16"),
        ColorType::L8 | ColorType::La8 => ("uchar", "b-w"),
        ColorType::Rgb32F | ColorType::Rgba32F => ("float", "scrgb"),
        _ => ("uchar", if facts.cmyk { "cmyk" } else { "srgb" }),
    };
    let detected = DetectedImage {
        media_type,
        width: u64::from(if transposed { height } else { width }),
        height: u64::from(if transposed { width } else { height }),
        animated: facts.frames > 1,
        carries_metadata: exif.is_some()
            || profile.is_some()
            || xmp.is_some()
            || orientation.is_some()
            || facts.metadata,
        depth: depth.into(),
        space: space.into(),
        has_alpha: color.has_alpha() || media_type == ImageMediaType::Gif,
    };
    if let Some(limits) = limits {
        if limits
            .max_pixels
            .is_some_and(|max| detected.width.saturating_mul(detected.height) > max)
        {
            return Err(error(
                "Image exceeds the configured decoded-pixel limit.",
                Code::ImageTooManyPixels,
            )
            .into());
        }
        if limits
            .max_dimension
            .is_some_and(|max| detected.width.max(detected.height) > max)
        {
            return Err(error(
                "Image exceeds the configured per-side pixel limit.",
                Code::ImageDimensionTooLarge,
            )
            .into());
        }
    }
    if full {
        DynamicImage::from_decoder(decoder).map_err(invalid)?;
    }
    Ok(detected)
}
#[derive(Default)]
struct ContainerFacts {
    metadata: bool,
    frames: u32,
    cmyk: bool,
}
fn container_facts(data: &[u8], format: Option<ImageFormat>) -> ContainerFacts {
    let mut facts = ContainerFacts {
        frames: 1,
        ..Default::default()
    };
    match format {
        Some(ImageFormat::Png) => {
            let mut offset = 8;
            while let Some(header) = data.get(offset..offset + 8) {
                let length =
                    u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
                let kind = &header[4..8];
                facts.metadata |= matches!(kind, b"eXIf" | b"iCCP" | b"tEXt" | b"iTXt" | b"zTXt");
                if kind == b"acTL"
                    && let Some(bytes) = data.get(offset + 8..offset + 12)
                {
                    facts.frames = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                }
                let Some(next) = offset.checked_add(length).and_then(|n| n.checked_add(12)) else {
                    break;
                };
                offset = next;
            }
        }
        Some(ImageFormat::Jpeg) => {
            let mut offset = 2;
            while offset + 3 < data.len() {
                if data[offset] != 255 {
                    break;
                }
                let marker = data[offset + 1];
                offset += 2;
                if marker == 0xda || marker == 0xd9 {
                    break;
                }
                if marker == 0xff {
                    offset -= 1;
                    continue;
                }
                if matches!(marker, 0xd0..=0xd8 | 1) {
                    continue;
                }
                let length = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
                if length < 2 {
                    break;
                }
                // EXIF, ICC and XMP are exposed by the decoder. Other APPn
                // payloads (for example MPF) are not retained descriptive metadata.
                facts.metadata |= marker == 0xfe
                    || (marker == 0xed
                        && data
                            .get(offset + 2..offset + length)
                            .is_some_and(|payload| payload.starts_with(b"Photoshop 3.0\0")));
                if matches!(marker,0xc0..=0xc3|0xc5..=0xc7|0xc9..=0xcb|0xcd..=0xcf) {
                    facts.cmyk = data.get(offset + 7) == Some(&4);
                }
                offset += length;
            }
        }
        Some(ImageFormat::WebP) => {
            let mut offset = 12;
            let mut frames = 0;
            while let Some(header) = data.get(offset..offset + 8) {
                let length =
                    u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
                facts.metadata |= matches!(&header[..4], b"EXIF" | b"XMP " | b"ICCP");
                if &header[..4] == b"ANMF" {
                    frames += 1;
                }
                let Some(next) = offset
                    .checked_add(8)
                    .and_then(|n| n.checked_add(length))
                    .and_then(|n| n.checked_add(length % 2))
                else {
                    break;
                };
                offset = next;
            }
            facts.frames = frames.max(1);
        }
        Some(ImageFormat::Gif) => {
            // GIF sub-block framing lets header probes count frames without decoding pixels.
            let mut offset = 13;
            if data.get(10).is_some_and(|packed| packed & 128 != 0) {
                offset += 3 * (1usize << ((data[10] & 7) + 1));
            }
            let mut frames = 0;
            while offset < data.len() {
                match data[offset] {
                    0x2c => {
                        let Some(descriptor) = data.get(offset..offset + 10) else {
                            break;
                        };
                        let packed = descriptor[9];
                        offset += 10;
                        if packed & 128 != 0 {
                            offset += 3 * (1usize << ((packed & 7) + 1));
                        }
                        offset += 1;
                        frames += 1;
                    }
                    0x21 => {
                        let Some(label) = data.get(offset + 1) else {
                            break;
                        };
                        facts.metadata |= matches!(*label, 0xfe | 0x01);
                        offset += 2;
                    }
                    _ => break,
                }
                while let Some(&length) = data.get(offset) {
                    offset += 1;
                    if length == 0 {
                        break;
                    }
                    offset += usize::from(length);
                }
            }
            facts.frames = frames.max(1);
        }
        _ => {}
    }
    facts
}

pub(crate) fn decode_srgb(data: &[u8], orient: bool) -> Result<DynamicImage, BoxError> {
    let reader = reader(data)?;
    let format = reader.format();
    let mut decoder = reader.into_decoder().map_err(invalid)?;
    let orientation = decoder.orientation().map_err(invalid)?;
    let profile = decoder.icc_profile().map_err(invalid)?;
    if container_facts(data, format).cmyk {
        let mut image = decode_cmyk_srgb(data, profile.as_deref())?;
        if orient {
            image.apply_orientation(orientation);
        }
        return Ok(image);
    }
    let mut image = DynamicImage::from_decoder(decoder).map_err(invalid)?;
    if orient {
        image.apply_orientation(orientation);
    }
    if let Some(icc) = profile {
        let source = moxcms::ColorProfile::new_from_slice(&icc)?;
        let destination = moxcms::ColorProfile::new_srgb();
        let layout = if image.color().has_alpha() {
            moxcms::Layout::Rgba
        } else {
            moxcms::Layout::Rgb
        };
        let (input_layout, input) = if source.color_space == moxcms::DataColorSpace::Gray {
            if image.color().has_alpha() {
                (moxcms::Layout::GrayAlpha, image.to_luma_alpha8().into_raw())
            } else {
                (moxcms::Layout::Gray, image.to_luma8().into_raw())
            }
        } else if image.color().has_alpha() {
            (moxcms::Layout::Rgba, image.to_rgba8().into_raw())
        } else {
            (moxcms::Layout::Rgb, image.to_rgb8().into_raw())
        };
        let transform = source.create_transform_8bit(
            input_layout,
            &destination,
            layout,
            moxcms::TransformOptions::default(),
        )?;
        if image.color().has_alpha() {
            let mut out = image.to_rgba8();
            transform.transform(&input, out.as_mut())?;
            image = DynamicImage::ImageRgba8(out);
        } else {
            let mut out = image.to_rgb8();
            transform.transform(&input, out.as_mut())?;
            image = DynamicImage::ImageRgb8(out);
        }
    } else if image.color().has_alpha() {
        image = DynamicImage::ImageRgba8(image.to_rgba8());
    } else {
        image = DynamicImage::ImageRgb8(image.to_rgb8());
    }
    Ok(image)
}

fn decode_cmyk_srgb(data: &[u8], embedded: Option<&[u8]>) -> Result<DynamicImage, BoxError> {
    use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default()
        .set_strict_mode(false)
        .set_max_width(usize::MAX)
        .set_max_height(usize::MAX);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
    decoder.decode_headers()?;
    let input_space = decoder.input_colorspace().ok_or_else(|| {
        std::io::Error::other("JPEG decoder did not report its channel colour space.")
    })?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| std::io::Error::other("JPEG decoder did not report its dimensions."))?;
    // image's JPEG wrapper would reduce these channels to RGB before ICC.
    // Request the original planes so the profile sees C, M, Y and K separately.
    decoder.set_options(decoder.options().jpeg_set_out_colorspace(input_space));
    let mut ink = decoder.decode()?;
    match input_space {
        ColorSpace::CMYK => {
            // JPEG's inverted CMYK convention -> ICC ink coverage (0 = no ink).
            for channel in &mut ink {
                *channel = 255 - *channel;
            }
        }
        ColorSpace::YCCK => {
            // Adobe YCCK encodes the three ink channels as full-range YCbCr;
            // only its K plane is inverted. zune has no YCCK->CMYK output mode.
            for pixel in ink.chunks_exact_mut(4) {
                let y = f64::from(pixel[0]);
                let cb = f64::from(pixel[1]) - 128.0;
                let cr = f64::from(pixel[2]) - 128.0;
                let channel = |v: f64| v.round().clamp(0.0, 255.0) as u8;
                pixel[0] = channel(y + 1.402 * cr);
                pixel[1] = channel(y - 0.344136 * cb - 0.714136 * cr);
                pixel[2] = channel(y + 1.772 * cb);
                pixel[3] = 255 - pixel[3];
            }
        }
        _ => {
            return Err(
                std::io::Error::other("JPEG decoder did not return four ink channels.").into(),
            );
        }
    }
    let fallback;
    let profile_bytes = if let Some(embedded) = embedded {
        embedded
    } else {
        use base64::Engine;
        // Sharp 0.35.5 src/pipeline.cc:391-397 uses libvips' built-in "cmyk"
        // profile, not naive CMYK multiplication. Preserve its forward LUTs.
        let encoded: String = include_str!("../assets/cmyk-input.icc.base64")
            .split_ascii_whitespace()
            .collect();
        fallback = base64::engine::general_purpose::STANDARD.decode(encoded)?;
        &fallback
    };
    let source = moxcms::ColorProfile::new_from_slice(profile_bytes)?;
    if source.color_space != moxcms::DataColorSpace::Cmyk {
        return Err(
            std::io::Error::other("JPEG ICC profile does not describe CMYK channels.").into(),
        );
    }
    let transform = source.create_transform_8bit(
        // moxcms uses Rgba as the storage layout for four CMYK components;
        // the source profile, rather than this layout name, specifies their meaning.
        moxcms::Layout::Rgba,
        &moxcms::ColorProfile::new_srgb(),
        moxcms::Layout::Rgb,
        moxcms::TransformOptions {
            rendering_intent: moxcms::RenderingIntent::Perceptual,
            ..Default::default()
        },
    )?;
    let mut output = ::image::RgbImage::new(u32::try_from(width)?, u32::try_from(height)?);
    transform.transform(&ink, output.as_mut())?;
    Ok(DynamicImage::ImageRgb8(output))
}

pub(crate) fn resize_srgb(
    pixels: DynamicImage,
    width: u32,
    height: u32,
    inside: bool,
) -> DynamicImage {
    let resize = |image: &DynamicImage| {
        if inside {
            image.resize(width, height, ::image::imageops::FilterType::Lanczos3)
        } else {
            image.resize_exact(width, height, ::image::imageops::FilterType::Lanczos3)
        }
    };
    if (pixels.width(), pixels.height()) == (width, height) {
        return pixels;
    }
    if !pixels.color().has_alpha() {
        return resize(&pixels);
    }
    // Like Sharp, resample associated alpha; image's resizer expects its
    // caller to premultiply, otherwise hidden colours bleed into visible edges.
    let mut associated = pixels.to_rgba32f();
    for pixel in associated.pixels_mut() {
        let alpha = pixel[3];
        for channel in &mut pixel.0[..3] {
            *channel *= alpha;
        }
    }
    let mut resized = resize(&DynamicImage::ImageRgba32F(associated)).to_rgba32f();
    for pixel in resized.pixels_mut() {
        let alpha = pixel[3];
        for channel in &mut pixel.0[..3] {
            *channel = if alpha > 0.0 { *channel / alpha } else { 0.0 };
        }
    }
    DynamicImage::ImageRgba8(DynamicImage::ImageRgba32F(resized).to_rgba8())
}

pub(crate) fn attachment_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    HarnessError::of(error).is_some_and(|e| e.is_named(hoh_attachment::AttachmentError::NAME))
}
