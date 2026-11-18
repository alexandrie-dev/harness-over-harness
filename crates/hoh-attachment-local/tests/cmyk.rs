use crate::{image::*, normalization::*, test_support::*};
use ::image::DynamicImage;
use hoh_attachment::ImageMediaType;

fn segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.extend([255, marker]);
    out.extend(u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    out.extend(payload);
}

// Constant, quantization-free 8x8 blocks with four independently encoded channels.
// Adobe CMYK stores inverted ink; YCCK stores Y/Cb/Cr and inverted K.
fn jpeg(samples: [u8; 4], profile: Option<&[u8]>, ycck: bool) -> Vec<u8> {
    let mut out = vec![255, 216];
    let mut adobe = b"Adobe\0\x64\0\0\0\0\0".to_vec();
    adobe[11] = if ycck { 2 } else { 0 };
    segment(&mut out, 0xee, &adobe);
    if let Some(profile) = profile {
        let chunks: Vec<_> = profile.chunks(65519).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            let mut app2 = b"ICC_PROFILE\0".to_vec();
            app2.extend([
                u8::try_from(index + 1).unwrap(),
                u8::try_from(chunks.len()).unwrap(),
            ]);
            app2.extend(*chunk);
            segment(&mut out, 0xe2, &app2);
        }
    }
    let mut quant = vec![0];
    quant.extend([1; 64]);
    segment(&mut out, 0xdb, &quant);
    segment(
        &mut out,
        0xc0,
        &[
            8, 0, 8, 0, 16, 4, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 4, 0x11, 0,
        ],
    );
    // DC categories 0..11 have four-bit canonical codes; AC has only EOB.
    let mut huffman = vec![0, 0, 0, 0, 12];
    huffman.extend([0; 12]);
    huffman.extend(0..12);
    huffman.extend([0x10, 1]);
    huffman.extend([0; 15]);
    huffman.push(0);
    segment(&mut out, 0xc4, &huffman);
    segment(&mut out, 0xda, &[4, 1, 0, 2, 0, 3, 0, 4, 0, 0, 63, 0]);
    let mut bits = Vec::new();
    let mut push = |value: u16, count: u32| {
        for bit in (0..count).rev() {
            bits.push(value & (1 << bit) != 0);
        }
    };
    for mcu in 0..2 {
        for sample in samples {
            let difference = if mcu == 0 {
                (i16::from(sample) - 128) * 8
            } else {
                0
            };
            let category = 16 - difference.unsigned_abs().leading_zeros();
            push(category as u16, 4);
            let amplitude = if difference < 0 {
                i32::from(difference) + (1 << category) - 1
            } else {
                i32::from(difference)
            };
            push(amplitude as u16, category);
            push(0, 1);
        }
    }
    while bits.len() % 8 != 0 {
        bits.push(true);
    }
    for byte in bits.chunks_exact(8) {
        let value = byte
            .iter()
            .fold(0u8, |acc, bit| (acc << 1) | u8::from(*bit));
        out.push(value);
        if value == 255 {
            out.push(0);
        }
    }
    out.extend([255, 217]);
    out
}

fn linear_cmyk_profile() -> Vec<u8> {
    use moxcms::{
        ColorProfile, DataColorSpace, LutDataType, LutStore, LutType, LutWarehouse, Matrix3d,
        ProfileClass,
    };
    // Analytic device: linear RGB = (1 - [C,M,Y]) * (1 - K).
    // The D50-adapted sRGB matrix maps it into ICC's u1Fixed15 XYZ PCS.
    let matrix = [
        [0.4360747, 0.3850649, 0.1430804],
        [0.2225045, 0.7168786, 0.0606169],
        [0.0139322, 0.0971045, 0.7141733],
    ];
    let mut clut = Vec::new();
    for c in 0..2 {
        for m in 0..2 {
            for y in 0..2 {
                for k in 0..2 {
                    let rgb =
                        [(1 - c) * (1 - k), (1 - m) * (1 - k), (1 - y) * (1 - k)].map(f64::from);
                    for row in matrix {
                        let xyz = row.iter().zip(rgb).map(|(a, b)| a * b).sum::<f64>();
                        clut.push((xyz * 32768.0).round() as u16);
                    }
                }
            }
        }
    }
    let mut profile = ColorProfile::new_srgb();
    profile.color_space = DataColorSpace::Cmyk;
    profile.pcs = DataColorSpace::Xyz;
    profile.profile_class = ProfileClass::InputDevice;
    profile.cicp = None;
    profile.chromatic_adaptation = None;
    profile.media_white_point = Some(moxcms::Xyzd {
        x: 0.9642,
        y: 1.0,
        z: 0.8249,
    });
    profile.red_trc = None;
    profile.green_trc = None;
    profile.blue_trc = None;
    profile.lut_a_to_b_perceptual = Some(LutWarehouse::Lut(LutDataType {
        num_input_channels: 4,
        num_output_channels: 3,
        num_clut_grid_points: 2,
        matrix: Matrix3d::IDENTITY,
        num_input_table_entries: 2,
        num_output_table_entries: 2,
        input_table: LutStore::Store16([0, 65535].repeat(4)),
        clut_table: LutStore::Store16(clut),
        output_table: LutStore::Store16([0, 65535].repeat(3)),
        lut_type: LutType::Lut16,
    }));
    let bytes = profile.encode().unwrap();
    moxcms::ColorProfile::new_from_slice(&bytes).expect("generated CMYK fixture profile parses");
    bytes
}

fn assert_pixels(image: DynamicImage, expected: [u8; 3], tolerance: u8) {
    assert_eq!((image.width(), image.height()), (16, 8));
    for pixel in image.to_rgb8().pixels() {
        for (actual, expected) in pixel.0.into_iter().zip(expected) {
            assert!(
                actual.abs_diff(expected) <= tolerance,
                "pixel {:?}, expected {:?}, tolerance {tolerance}",
                pixel.0,
                expected
            );
        }
    }
}

async fn check_conversion(data: Vec<u8>, expected: [u8; 3], profiled: bool) {
    let facts = detect_image(&data, None).await.unwrap();
    assert_eq!(facts.space, "cmyk");
    assert_eq!(facts.carries_metadata, profiled);
    assert!(!can_pass_through_normalization(
        &facts,
        data.len() as u64,
        &policy()
    ));
    let copy = data.clone();
    let converted = tokio::task::spawn_blocking(move || decode_srgb(&copy, false))
        .await
        .unwrap()
        .unwrap();
    // Three levels cover 8-bit ICC interpolation/rounding, not a different colour model.
    assert_pixels(converted, expected, 3);
    let normalized = normalize_image(&data, &facts, &policy()).await.unwrap();
    assert_ne!(normalized.data, data);
    assert_eq!(normalized.media_type, ImageMediaType::Jpeg);
    let output = detect_image(&normalized.data, None).await.unwrap();
    assert_eq!(output.space, "srgb");
    assert_eq!(output.depth, "uchar");
    assert!(!output.carries_metadata);
    let decoded = tokio::task::spawn_blocking(move || ::image::load_from_memory(&normalized.data))
        .await
        .unwrap()
        .unwrap();
    // The final JPEG quality-85 encode can introduce another small error.
    assert_pixels(decoded, expected, 6);
}

#[tokio::test]
async fn converts_cmyk_jpeg_using_its_embedded_profile() {
    let profile = linear_cmyk_profile();
    for (ink, expected) in [
        ([0, 0, 0, 0], [255, 255, 255]),
        ([255, 0, 0, 0], [0, 255, 255]),
        ([0, 255, 0, 0], [255, 0, 255]),
        ([0, 0, 255, 0], [255, 255, 0]),
        ([0, 0, 0, 255], [0, 0, 0]),
        ([64, 128, 192, 32], [212, 176, 128]),
    ] {
        check_conversion(
            jpeg(ink.map(|value| 255 - value), Some(&profile), false),
            expected,
            true,
        )
        .await;
    }
}

#[tokio::test]
async fn converts_unprofiled_cmyk_using_sharps_builtin_chemical_proof_profile() {
    // Golden RGB: source cmyk profile's corner Lab values, ICC v2 Lab decoding,
    // D50->D65 Bradford adaptation, standard sRGB matrix and transfer function.
    // These are intentionally different from naive (1-C)*(1-K) RGB values.
    for (ink, expected) in [
        ([0, 0, 0, 0], [255, 255, 255]),
        ([255, 0, 0, 0], [0, 156, 228]),
        ([0, 255, 0, 0], [234, 23, 133]),
        ([0, 0, 255, 0], [255, 240, 0]),
        ([0, 0, 0, 255], [41, 35, 43]),
        ([255, 255, 255, 255], [33, 33, 44]),
    ] {
        check_conversion(
            jpeg(ink.map(|value| 255 - value), None, false),
            expected,
            false,
        )
        .await;
    }
}

#[tokio::test]
async fn converts_ycck_to_four_ink_channels_before_applying_the_profile() {
    let profile = linear_cmyk_profile();
    check_conversion(
        jpeg([150, 100, 180, 223], Some(&profile), true),
        [93, 180, 193],
        true,
    )
    .await;
}
