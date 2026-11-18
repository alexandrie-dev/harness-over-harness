use crate::{
    image::{DetectedImage, detect_image},
    normalization::*,
    test_support::*,
};
use ::image::{DynamicImage, ImageFormat};
use hoh_attachment::ImageMediaType;

async fn normalize(data: &[u8], budget: NormalizationPolicy) -> NormalizedImage {
    normalize_image(data, &detect_image(data, None).await.unwrap(), &budget)
        .await
        .unwrap()
}
fn clean() -> DetectedImage {
    DetectedImage {
        media_type: ImageMediaType::Png,
        width: 4,
        height: 4,
        animated: false,
        carries_metadata: false,
        depth: "uchar".into(),
        space: "srgb".into(),
        has_alpha: false,
    }
}
#[test]
fn accepts_an_in_budget_clean_png_jpeg_webp_and_refuses_gif_animation_metadata_oversized_edges_and_oversized_bytes()
 {
    let policy = policy();
    assert!(can_pass_through_normalization(
        &DetectedImage {
            width: 8192,
            height: 4,
            ..clean()
        },
        100,
        &policy
    ));
    for bad in [
        DetectedImage {
            media_type: ImageMediaType::Gif,
            ..clean()
        },
        DetectedImage {
            animated: true,
            ..clean()
        },
        DetectedImage {
            carries_metadata: true,
            ..clean()
        },
        DetectedImage {
            depth: "ushort".into(),
            ..clean()
        },
        DetectedImage {
            space: "rgb16".into(),
            ..clean()
        },
        DetectedImage {
            width: 2049,
            height: 2048,
            ..clean()
        },
        DetectedImage {
            width: 8193,
            height: 4,
            ..clean()
        },
    ] {
        assert!(!can_pass_through_normalization(&bad, 100, &policy));
    }
    assert!(!can_pass_through_normalization(
        &clean(),
        policy.max_bytes + 1,
        &policy
    ));
}
#[tokio::test]
async fn passes_an_already_normalized_source_through_byte_identically() {
    let data = flat(6, 4, ImageFormat::WebP, false).await;
    let result = normalize(&data, policy()).await;
    assert_eq!(result.data, data);
    assert_eq!(
        (result.media_type, result.width, result.height),
        (ImageMediaType::Webp, 6, 4)
    );
}
#[tokio::test]
async fn converts_a_16_bit_three_and_four_channel_png_to_8_bit_srgb_without_passthrough() {
    for alpha in [false, true] {
        let data = png16(7, 5, alpha).await;
        let detected = detect_image(&data, None).await.unwrap();
        assert_eq!(detected.depth, "ushort");
        assert_eq!(detected.space, "rgb16");
        assert_eq!(detected.has_alpha, alpha);
        let normalized = normalize(&data, policy()).await;
        assert_ne!(normalized.data, data);
        let metadata = detect_image(&normalized.data, None).await.unwrap();
        assert_eq!((metadata.width, metadata.height), (7, 5));
        assert_eq!(metadata.depth, "uchar");
        assert_eq!(metadata.space, "srgb");
        assert_eq!(metadata.has_alpha, alpha);
    }
}
#[tokio::test]
async fn downscales_an_oversized_opaque_png_to_the_long_edge_target_as_jpeg() {
    let data = flat(10, 6, ImageFormat::Png, false).await;
    let policy = NormalizationPolicy {
        max_dimension: 5,
        ..policy()
    };
    let first = normalize(&data, policy).await;
    assert_eq!(
        (first.media_type, first.width, first.height),
        (ImageMediaType::Jpeg, 5, 3)
    );
    let detected = detect_image(&first.data, None).await.unwrap();
    assert!(!detected.animated);
    assert!(!detected.carries_metadata);
    assert_eq!(detected.depth, "uchar");
    assert_eq!(detected.space, "srgb");
    assert_eq!(normalize(&data, policy).await, first);
}
#[tokio::test]
async fn re_encodes_the_normalized_output_of_a_resize_into_itself_idempotence() {
    let data = flat(10, 6, ImageFormat::Png, false).await;
    let budget = NormalizationPolicy {
        max_dimension: 5,
        ..policy()
    };
    let first = normalize(&data, budget).await;
    assert_eq!(normalize(&first.data, budget).await, first);
}
#[tokio::test]
async fn always_re_encodes_gif_as_a_single_still_frame() {
    let data = flat(6, 4, ImageFormat::Gif, false).await;
    assert!(detect_image(&data, None).await.unwrap().has_alpha);
    let normalized = normalize(&data, policy()).await;
    assert_eq!(normalized.media_type, ImageMediaType::Webp);
    let detected = detect_image(&normalized.data, None).await.unwrap();
    assert_eq!((detected.width, detected.height), (6, 4));
    assert!(!detected.animated);
    assert!(!detected.carries_metadata);
    assert_eq!(detected.depth, "uchar");
    assert_eq!(detected.space, "srgb");
}

#[tokio::test]
async fn review_sharp_inside_dimensions_and_recorded_original_dimensions() {
    // Hand-derived from ResolveShrink, libvips JPEG floor-loaded axes and floor(x + .5).
    // For 3000x2001 the policy bounds are the source's 2000x2000 square;
    // initialDimensions supplies 2000x1334 inside that square.
    for (width, height, cap, format, expected) in [
        (4000, 3, 2000, ImageFormat::Png, (1333, 1)),
        (4000, 3, 2000, ImageFormat::Jpeg, (2000, 1)),
        (3000, 2001, 2000, ImageFormat::Png, (2000, 1334)),
        (3000, 2001, 2000, ImageFormat::Jpeg, (2000, 1334)),
        (3, 4000, 2000, ImageFormat::Png, (1, 1333)),
        (3, 4000, 2000, ImageFormat::Jpeg, (1, 2000)),
        (4000, 3000, 2000, ImageFormat::Png, (2000, 1500)),
        (4000, 3000, 2000, ImageFormat::Jpeg, (2000, 1500)),
        (99, 33, 11, ImageFormat::Png, (9, 3)),
        (99, 33, 11, ImageFormat::Jpeg, (9, 3)),
        (6, 4, 60, ImageFormat::Png, (6, 4)),
        (6, 4, 60, ImageFormat::Jpeg, (6, 4)),
        (17, 9, 2, ImageFormat::Png, (2, 1)),
        (17, 9, 2, ImageFormat::Jpeg, (2, 1)),
        // Shrink 8.9 selects 8, then halves to 4 because trunc(8.9)==8.
        (267, 89, 32, ImageFormat::Png, (30, 10)),
        (267, 89, 32, ImageFormat::Jpeg, (30, 10)),
    ] {
        let data = flat(width, height, format, false).await;
        let input = hoh_attachment::SaveImageAttachment {
            data,
            media_type: if format == ImageFormat::Png {
                ImageMediaType::Png
            } else {
                ImageMediaType::Jpeg
            },
            name: None,
        };
        if width == 3000 && height == 2001 {
            let media_type = input.media_type;
            let direct = tokio::task::spawn_blocking(move || {
                crate::image::resize_for_target(
                    DynamicImage::new_rgb8(width, height),
                    Some(2000),
                    Some(2000),
                    media_type,
                )
            })
            .await
            .unwrap();
            assert_eq!(
                (direct.width(), direct.height()),
                (2000, 1334),
                "explicit source 2000x2000 box, {format:?}"
            );
        }
        let mut admission = limits();
        admission.max_image_bytes = 50 * 1024 * 1024;
        admission.max_image_pixels = 64_000_000;
        admission.max_image_dimension = 65_535;
        let budget = NormalizationPolicy {
            max_pixels: 64_000_000,
            max_dimension: cap,
            max_bytes: if cap > u64::from(width.max(height)) {
                1
            } else {
                4 * 1024 * 1024
            },
        };
        let prepared = crate::prepare_image_file(&input, &admission, &budget)
            .await
            .unwrap();
        assert_eq!(
            (prepared.r#ref.width, prepared.r#ref.height),
            expected,
            "{width}x{height}, cap {cap}, {format:?}"
        );
        assert_eq!(
            prepared.r#ref.original_dimensions,
            if expected == (u64::from(width), u64::from(height)) {
                None
            } else {
                Some(hoh_attachment::ProjectedDimensions {
                    width: width.into(),
                    height: height.into(),
                })
            }
        );
        let verified = detect_image(&prepared.data, None).await.unwrap();
        assert_eq!((verified.width, verified.height), expected);
        assert_eq!(
            prepared.r#ref.attachment_id.as_str(),
            format!("sha256:{}", sha256(&prepared.data))
        );
    }
}

#[tokio::test]
async fn review_autorotation_swaps_the_resize_target_before_shrink() {
    let data = oriented(4000, 3, 6).await;
    let facts = detect_image(&data, None).await.unwrap();
    assert_eq!((facts.width, facts.height), (3, 4000));
    let normalized = normalize_image(
        &data,
        &facts,
        &NormalizationPolicy {
            max_pixels: 64_000_000,
            max_dimension: 2000,
            ..policy()
        },
    )
    .await
    .unwrap();
    assert_eq!((normalized.width, normalized.height), (1, 2000));
}

#[tokio::test]
async fn review_webp_scale_on_load_and_gif_plain_resize_dimensions() {
    for (width, height, cap, format, expected) in [
        // 4828 * (9/408) = 106.5: rint rounds to even, hence 106, not 107.
        (4828, 408, 107, ImageFormat::WebP, (106, 9)),
        (4000, 3, 2000, ImageFormat::WebP, (1333, 1)),
        (3000, 2001, 2000, ImageFormat::WebP, (2000, 1334)),
        (3, 4000, 2000, ImageFormat::WebP, (1, 1333)),
        (4000, 3000, 2000, ImageFormat::WebP, (2000, 1500)),
        (99, 33, 11, ImageFormat::WebP, (9, 3)),
        (6, 4, 60, ImageFormat::WebP, (6, 4)),
        (17, 9, 2, ImageFormat::WebP, (2, 1)),
        (267, 89, 32, ImageFormat::WebP, (30, 10)),
        // GIF does not reload at a scale: the same half tie rounds up at output.
        (4828, 408, 107, ImageFormat::Gif, (107, 9)),
    ] {
        let data = flat(width, height, format, false).await;
        let detected = detect_image(&data, None).await.unwrap();
        let budget = NormalizationPolicy {
            max_pixels: 64_000_000,
            max_dimension: cap,
            max_bytes: 1,
        };
        let prepared = normalize_image(&data, &detected, &budget).await.unwrap();
        assert_eq!(
            (prepared.width, prepared.height),
            expected,
            "{width}x{height}, cap {cap}, {format:?}"
        );
        let actual = detect_image(&prepared.data, None).await.unwrap();
        assert_eq!((actual.width, actual.height), expected);
    }
}
#[tokio::test]
async fn keeps_a_transparent_source_on_the_webp_ladder() {
    let data = flat(9, 5, ImageFormat::WebP, true).await;
    let normalized = normalize(
        &data,
        NormalizationPolicy {
            max_dimension: 4,
            ..policy()
        },
    )
    .await;
    assert_eq!(
        (normalized.media_type, normalized.width, normalized.height),
        (ImageMediaType::Webp, 4, 2)
    );
    assert!(
        detect_image(&normalized.data, None)
            .await
            .unwrap()
            .has_alpha
    );
}
#[tokio::test]
async fn accepts_webp_output_that_omits_an_all_opaque_source_alpha_plane() {
    let data = noisy(64, 32, ImageFormat::Png, Some(false)).await;
    assert!(detect_image(&data, None).await.unwrap().has_alpha);
    let normalized = normalize(
        &data,
        NormalizationPolicy {
            max_dimension: 32,
            ..policy()
        },
    )
    .await;
    assert_eq!(
        (normalized.media_type, normalized.width, normalized.height),
        (ImageMediaType::Webp, 32, 16)
    );
    assert!(
        !detect_image(&normalized.data, None)
            .await
            .unwrap()
            .has_alpha
    );
}
#[tokio::test]
async fn keeps_the_smallest_transparent_ladder_output_above_an_unreachable_byte_target_without_shrinking()
 {
    let data = noisy(128, 128, ImageFormat::Png, Some(true)).await;
    let result = normalize(
        &data,
        NormalizationPolicy {
            max_dimension: 128,
            max_bytes: 1024,
            ..policy()
        },
    )
    .await;
    assert!(result.data.len() > 1024);
    assert_eq!(
        (result.media_type, result.width, result.height),
        (ImageMediaType::Webp, 128, 128)
    );
    assert!(detect_image(&result.data, None).await.unwrap().has_alpha);
}
#[tokio::test]
async fn re_encodes_an_oversized_photographic_jpeg_as_jpeg() {
    let data = noisy(64, 32, ImageFormat::Jpeg, None).await;
    let result = normalize(
        &data,
        NormalizationPolicy {
            max_dimension: 32,
            ..policy()
        },
    )
    .await;
    assert_eq!(
        (result.media_type, result.width, result.height),
        (ImageMediaType::Jpeg, 32, 16)
    );
}
#[tokio::test]
async fn re_encodes_an_opaque_gradient_png_as_jpeg_within_the_byte_target() {
    let pixels = ::image::RgbImage::from_fn(256, 256, |x, y| {
        ::image::Rgb([x as u8, y as u8, ((x + y) >> 1) as u8])
    });
    let data = encode(DynamicImage::ImageRgb8(pixels), ImageFormat::Png).await;
    let result = normalize(
        &data,
        NormalizationPolicy {
            max_dimension: 128,
            ..policy()
        },
    )
    .await;
    assert_eq!(
        (result.media_type, result.width, result.height),
        (ImageMediaType::Jpeg, 128, 128)
    );
    assert!(result.data.len() as u64 <= policy().max_bytes);
}
#[tokio::test]
async fn keeps_the_smallest_opaque_ladder_output_above_an_unreachable_byte_target() {
    let data = noisy(64, 64, ImageFormat::Png, None).await;
    let result = normalize(
        &data,
        NormalizationPolicy {
            max_dimension: 2048,
            max_bytes: 512,
            ..policy()
        },
    )
    .await;
    assert!(result.data.len() > 512);
    assert_eq!(
        (result.media_type, result.width, result.height),
        (ImageMediaType::Jpeg, 64, 64)
    );
}
#[tokio::test]
async fn re_encodes_an_in_budget_oriented_jpeg_baking_rotation_and_stripping_metadata() {
    let data = oriented(4, 2, 6).await;
    let before = detect_image(&data, None).await.unwrap();
    assert!(before.carries_metadata);
    assert_eq!((before.width, before.height), (2, 4));
    let result = normalize(&data, policy()).await;
    assert_ne!(result.data, data);
    let after = detect_image(&result.data, None).await.unwrap();
    assert_eq!((after.width, after.height), (2, 4));
    assert!(!after.carries_metadata);
}
#[tokio::test]
async fn re_encodes_an_in_budget_image_with_an_icc_profile_and_strips_the_profile() {
    let data = profiled_png().await;
    assert!(detect_image(&data, None).await.unwrap().carries_metadata);
    let result = normalize(&data, policy()).await;
    assert_ne!(result.data, data);
    assert!(
        !detect_image(&result.data, None)
            .await
            .unwrap()
            .carries_metadata
    );
}
#[tokio::test]
async fn converts_profiled_gray_and_gray_alpha_pixels_to_srgb() {
    for alpha in [false, true] {
        let data = tokio::task::spawn_blocking(move || {
            use ::image::ImageEncoder;
            let mut data = Vec::new();
            let mut encoder = ::image::codecs::png::PngEncoder::new(&mut data);
            encoder
                .set_icc_profile(
                    moxcms::ColorProfile::new_gray_with_gamma(1.0)
                        .encode()
                        .unwrap(),
                )
                .unwrap();
            if alpha {
                encoder
                    .write_image(&[128, 128], 1, 1, ::image::ExtendedColorType::La8)
                    .unwrap();
            } else {
                encoder
                    .write_image(&[128], 1, 1, ::image::ExtendedColorType::L8)
                    .unwrap();
            }
            data
        })
        .await
        .unwrap();
        let result = normalize(&data, policy()).await;
        let decoded = tokio::task::spawn_blocking(move || {
            ::image::load_from_memory(&result.data).unwrap().to_rgba8()
        })
        .await
        .unwrap();
        let pixel = decoded.get_pixel(0, 0).0;
        assert!(
            (180..=196).contains(&pixel[0]),
            "linear gray must be converted to sRGB: {pixel:?}"
        );
        if alpha {
            assert_eq!(pixel[3], 128);
        }
    }
}
#[tokio::test]
async fn maps_an_encoder_fault_on_undecodable_bytes_to_a_storage_failure() {
    let detected = DetectedImage {
        width: 5000,
        height: 5000,
        depth: "ushort".into(),
        space: "rgb16".into(),
        has_alpha: true,
        ..clean()
    };
    let failure = assert_code(
        normalize_image(&[1, 2, 3], &detected, &policy()).await,
        "ATTACHMENT_WRITE_FAILED",
    );
    assert_eq!(
        failure.to_string(),
        "The 16-bit PNG could not be converted to the normalized 8-bit sRGB form."
    );
}
#[tokio::test]
async fn describes_a_failed_conversion_without_exposing_the_encoder_error() {
    for (media_type, depth, description) in [
        (ImageMediaType::Png, "float", "float PNG"),
        (ImageMediaType::Jpeg, "uchar", "uchar JPEG"),
    ] {
        let detected = DetectedImage {
            media_type,
            depth: depth.into(),
            width: 5000,
            height: 5000,
            ..clean()
        };
        let failure = assert_code(
            normalize_image(&[1, 2, 3], &detected, &policy()).await,
            "ATTACHMENT_WRITE_FAILED",
        );
        assert_eq!(
            failure.to_string(),
            format!("The {description} could not be converted to the normalized 8-bit sRGB form.")
        );
    }
}
#[tokio::test]
async fn downscales_by_total_pixels_so_an_extreme_aspect_ratio_keeps_its_short_edge() {
    let result = normalize(
        &flat(10, 40, ImageFormat::Png, false).await,
        NormalizationPolicy {
            max_pixels: 100,
            ..policy()
        },
    )
    .await;
    assert_eq!(
        (result.media_type, result.width, result.height),
        (ImageMediaType::Jpeg, 5, 20)
    );
}
#[tokio::test]
async fn caps_the_long_edge_after_the_total_pixel_budget() {
    let result = normalize(
        &flat(4, 64, ImageFormat::Png, false).await,
        NormalizationPolicy {
            max_pixels: 10_000,
            max_dimension: 16,
            ..policy()
        },
    )
    .await;
    assert_eq!(
        (result.media_type, result.width, result.height),
        (ImageMediaType::Jpeg, 1, 16)
    );
}
#[tokio::test]
async fn keeps_an_antialiased_text_screenshot_readable_on_the_jpeg_ladder() {
    // A deterministic antialiased glyph raster replaces Sharp's SVG/font renderer.
    let pixels = ::image::RgbImage::from_fn(1024, 512, |x, y| {
        let glyph = (50..970).contains(&x)
            && (120..390).contains(&y)
            && ((x - 50) % 95 < 15 || !(138..=372).contains(&y));
        let edge = (49..971).contains(&x)
            && (119..391).contains(&y)
            && ((x - 49) % 95 < 17 || !(139..=371).contains(&y));
        ::image::Rgb(if glyph {
            [17, 47, 77]
        } else if edge {
            [128, 140, 155]
        } else {
            [255, 255, 255]
        })
    });
    let data = encode(DynamicImage::ImageRgb8(pixels), ImageFormat::Png).await;
    let result = normalize(
        &data,
        NormalizationPolicy {
            max_dimension: 512,
            ..policy()
        },
    )
    .await;
    assert_eq!(
        (result.media_type, result.width, result.height),
        (ImageMediaType::Jpeg, 512, 256)
    );
    let decoded = tokio::task::spawn_blocking(move || {
        ::image::load_from_memory(&result.data).unwrap().to_luma8()
    })
    .await
    .unwrap();
    assert!(*decoded.as_raw().iter().min().unwrap() < 80);
    assert!(*decoded.as_raw().iter().max().unwrap() > 240);
}
