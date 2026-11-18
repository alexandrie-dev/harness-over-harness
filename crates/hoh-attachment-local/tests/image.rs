use crate::{image::*, test_support::*};

#[tokio::test]
async fn resizing_transparency_does_not_mix_hidden_colour_into_visible_edges() {
    tokio::task::spawn_blocking(|| {
        let mut pixels = ::image::RgbaImage::new(2, 1);
        pixels.put_pixel(0, 0, ::image::Rgba([255, 0, 0, 0]));
        pixels.put_pixel(1, 0, ::image::Rgba([0, 0, 255, 255]));
        let output = resize_srgb(::image::DynamicImage::ImageRgba8(pixels), 1, 1, true).to_rgba8();
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
    let mut jpeg = flat(4, 2, ImageFormat::Jpeg, false).await;
    jpeg.splice(2..2, [255, 254, 0, 5, b'f', b'o', b'o']);
    assert!(detect_image(&jpeg, None).await.unwrap().carries_metadata);
}
#[tokio::test]
async fn probes_malformed_bytes_and_unsupported_formats_into_the_same_stable_error() {
    assert_code(probe_image(&[1, 2, 3]).await, "INVALID_IMAGE");
    assert_code(probe_image(b"II\x2a\0\0\0\0\0").await, "INVALID_IMAGE");
}
