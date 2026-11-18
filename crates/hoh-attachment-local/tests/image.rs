use crate::{image::*, test_support::*};

#[tokio::test]
async fn resizing_transparency_does_not_mix_hidden_colour_into_visible_edges() {
    tokio::task::spawn_blocking(|| {
        let mut pixels = ::image::RgbaImage::new(2, 1);
        pixels.put_pixel(0, 0, ::image::Rgba([255, 0, 0, 0]));
        pixels.put_pixel(1, 0, ::image::Rgba([0, 0, 255, 255]));
        let output = resize_srgb(::image::DynamicImage::ImageRgba8(pixels), 1, 1).to_rgba8();
        let pixel = output.get_pixel(0, 0).0;
        assert_eq!(pixel[0], 0);
        assert_eq!(pixel[2], 255);
        assert!((127..=128).contains(&pixel[3]));
    })
    .await
    .unwrap();
}
use ::image::ImageFormat;

#[tokio::test]
async fn decodes_every_supported_format_and_its_intrinsic_dimensions() {
    for (format, media_type) in [
        (ImageFormat::Png, hoh_attachment::ImageMediaType::Png),
        (ImageFormat::Jpeg, hoh_attachment::ImageMediaType::Jpeg),
        (ImageFormat::WebP, hoh_attachment::ImageMediaType::Webp),
        (ImageFormat::Gif, hoh_attachment::ImageMediaType::Gif),
    ] {
        let detected = detect_image(&flat(3, 2, format, true).await, None)
            .await
            .unwrap();
        assert_eq!(detected.media_type, media_type);
        assert_eq!((detected.width, detected.height), (3, 2));
        assert!(!detected.animated);
        assert!(!detected.carries_metadata);
        assert_eq!(detected.depth, "uchar");
        assert_eq!(detected.space, "srgb");
    }
}
#[tokio::test]
async fn rejects_excess_decoded_pixels_before_decoding() {
    assert_code(
        detect_image(
            &flat(3, 2, ImageFormat::Png, false).await,
            Some(DecodedImageLimits {
                max_pixels: Some(5),
                max_dimension: None,
            }),
        )
        .await,
        "IMAGE_TOO_MANY_PIXELS",
    );
}
#[tokio::test]
async fn rejects_a_side_above_the_per_side_limit_and_accepts_a_side_exactly_at_it() {
    let data = flat(3, 2, ImageFormat::Png, false).await;
    assert_code(
        detect_image(
            &data,
            Some(DecodedImageLimits {
                max_pixels: None,
                max_dimension: Some(2),
            }),
        )
        .await,
        "IMAGE_DIMENSION_TOO_LARGE",
    );
    assert_eq!(
        detect_image(
            &data,
            Some(DecodedImageLimits {
                max_pixels: None,
                max_dimension: Some(3)
            })
        )
        .await
        .unwrap()
        .width,
        3
    );
}
#[tokio::test]
async fn rejects_malformed_bytes_and_truncated_payloads_with_readable_headers() {
    assert_code(detect_image(&[1, 2, 3], None).await, "INVALID_IMAGE");
    assert_code(
        detect_image(b"II\x2a\0\0\0\0\0", None).await,
        "INVALID_IMAGE",
    );
    let data = png();
    let truncated = &data[..62];
    assert_eq!(probe_image(truncated).await.unwrap().width, 1);
    assert_code(detect_image(truncated, None).await, "INVALID_IMAGE");
}
#[tokio::test]
async fn reports_animation_from_a_multi_frame_container_and_perceived_axes_from_exif_orientation() {
    let mut gif = vec![
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 255, 255, 255,
    ];
    let frame = [
        0x21, 0xf9, 4, 1, 0, 0, 0, 0, 0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0,
    ];
    gif.extend(frame);
    gif.extend(frame);
    gif.push(0x3b);
    assert!(detect_image(&gif, None).await.unwrap().animated);
    for (orientation, dimensions) in [(6, (2, 4)), (3, (4, 2))] {
        let detected = detect_image(&oriented(4, 2, orientation).await, None)
            .await
            .unwrap();
        assert_eq!((detected.width, detected.height), dimensions);
        assert!(detected.carries_metadata);
        assert!(!detected.animated);
    }
}
#[tokio::test]
async fn reports_color_profiles_and_encoder_metadata_as_metadata() {
    assert!(
        detect_image(&profiled_png().await, None)
            .await
            .unwrap()
            .carries_metadata
    );
    let png = png_add_before(
        flat(4, 2, ImageFormat::Png, false).await,
        b"IDAT",
        b"tEXt",
        b"Comment\0foo",
    );
    assert!(detect_image(&png, None).await.unwrap().carries_metadata);
}

async fn check_container(data: Vec<u8>, metadata: bool, animated: bool, passthrough: bool) {
    for facts in [
        probe_image(&data).await.unwrap(),
        detect_image(&data, None).await.unwrap(),
    ] {
        assert_eq!(facts.carries_metadata, metadata);
        assert_eq!(facts.animated, animated);
        assert_eq!(
            crate::can_pass_through_normalization(&facts, data.len() as u64, &policy()),
            passthrough
        );
        let normalized = crate::normalize_image(&data, &facts, &policy())
            .await
            .unwrap();
        assert_eq!(normalized.data == data, passthrough);
    }
}

#[tokio::test]
async fn review_jpeg_com_is_clean_and_only_photo_app13_is_retained() {
    let mut jpeg = flat(4, 2, ImageFormat::Jpeg, false).await;
    jpeg.splice(2..2, [255, 254, 0, 5, b'f', b'o', b'o']);
    check_container(jpeg.clone(), false, false, true).await;
    for (payload, retained) in [
        (b"Photoshop 3.0\0IPTC".as_slice(), true),
        (b"PhotoXYZ".as_slice(), true),
        (b"Photo".as_slice(), false),
        (b"retained APP13".as_slice(), false),
    ] {
        let mut with_app13 = jpeg.clone();
        let mut app13 = vec![255, 0xed];
        app13.extend(((payload.len() + 2) as u16).to_be_bytes());
        app13.extend(payload);
        with_app13.splice(2..2, app13);
        check_container(with_app13, retained, false, !retained).await;
    }
}

#[tokio::test]
async fn review_apng_is_single_frame_for_admission_and_passes_through() {
    let base = flat(3, 2, ImageFormat::Png, false).await;
    let idat = png_offset(&base, b"IDAT");
    let n = u32::from_be_bytes(base[idat..idat + 4].try_into().unwrap()) as usize;
    let compressed = &base[idat + 8..idat + 8 + n];
    let mut control = vec![];
    control.extend(2u32.to_be_bytes());
    control.extend(0u32.to_be_bytes());
    let frame = |sequence: u32| {
        let mut f = sequence.to_be_bytes().to_vec();
        f.extend(3u32.to_be_bytes());
        f.extend(2u32.to_be_bytes());
        f.extend([0; 8]);
        f.extend([0, 1, 0, 10, 0, 0]);
        f
    };
    let mut animated = png_add_before(base.clone(), b"IDAT", b"acTL", &control);
    animated = png_add_before(animated, b"IDAT", b"fcTL", &frame(0));
    animated = png_add_before(animated, b"IEND", b"fcTL", &frame(1));
    let mut second = 2u32.to_be_bytes().to_vec();
    second.extend(compressed);
    animated = png_add_before(animated, b"IEND", b"fdAT", &second);
    check_container(animated, false, false, true).await;
}

#[tokio::test]
async fn review_png_retained_metadata_stops_at_first_idat() {
    let base = flat(3, 2, ImageFormat::Png, false).await;
    let mut compressed = b"Comment\0\0".to_vec();
    compressed.extend(zlib_stored(b"foo"));
    let mut exif = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    exif.extend([1, 0, 0, 0, 0, 0, 0, 0]);
    for (kind, payload) in [
        (*b"tEXt", b"Comment\0foo".to_vec()),
        (*b"zTXt", compressed),
        (*b"iTXt", b"Comment\0\0\0\0\0foo".to_vec()),
        (
            *b"iTXt",
            b"XML:com.adobe.xmp\0\0\0\0\0<x:xmpmeta/>".to_vec(),
        ),
        (*b"eXIf", exif),
    ] {
        check_container(
            png_add_before(base.clone(), b"IDAT", &kind, &payload),
            true,
            false,
            false,
        )
        .await;
        check_container(
            png_add_before(base.clone(), b"IEND", &kind, &payload),
            false,
            false,
            true,
        )
        .await;
        let mut after_end = base.clone();
        after_end.extend(png_chunk(&kind, &payload));
        check_container(after_end, false, false, true).await;
    }
}

#[tokio::test]
async fn review_grey_png_and_animated_webp_force_normalization() {
    let grey = encode(
        ::image::DynamicImage::ImageLuma8(::image::GrayImage::from_pixel(
            3,
            2,
            ::image::Luma([128]),
        )),
        ImageFormat::Png,
    )
    .await;
    let detected = detect_image(&grey, None).await.unwrap();
    assert_eq!(detected.depth, "uchar");
    assert_eq!(detected.space, "b-w");
    check_container(grey, false, false, false).await;
    let still = flat(3, 2, ImageFormat::WebP, false).await;
    let mut contents = b"WEBP".to_vec();
    let mut chunk = |kind: &[u8; 4], payload: &[u8]| {
        contents.extend(kind);
        contents.extend((payload.len() as u32).to_le_bytes());
        contents.extend(payload);
        if !payload.len().is_multiple_of(2) {
            contents.push(0);
        }
    };
    chunk(b"VP8X", &[2, 0, 0, 0, 2, 0, 0, 1, 0, 0]);
    chunk(b"ANIM", &[0; 6]);
    let mut frame = vec![0; 16];
    frame[6] = 2;
    frame[9] = 1;
    frame[12] = 10;
    frame.extend(&still[12..]);
    chunk(b"ANMF", &frame);
    chunk(b"ANMF", &frame);
    let mut animated = b"RIFF".to_vec();
    animated.extend((contents.len() as u32).to_le_bytes());
    animated.extend(contents);
    check_container(animated, false, true, false).await;
}
#[tokio::test]
async fn probes_malformed_bytes_and_unsupported_formats_into_the_same_stable_error() {
    assert_code(probe_image(&[1, 2, 3]).await, "INVALID_IMAGE");
    assert_code(probe_image(b"II\x2a\0\0\0\0\0").await, "INVALID_IMAGE");
}
