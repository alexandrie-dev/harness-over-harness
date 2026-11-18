use crate::{test_support::*, *};
use ::image::ImageFormat;
use futures_util::stream;

#[tokio::test]
async fn resolves_every_omitted_admission_limit_explicitly() {
    let kernel = hoh_kernel::Kernel::start();
    let service = LocalAttachmentStore::new(kernel.root(), Config::default()).unwrap();
    assert_eq!(DEFAULT_MAX_IMAGE_BYTES, 50 * 1024 * 1024);
    assert_eq!(DEFAULT_MAX_IMAGES_PER_MESSAGE, 20);
    assert_eq!(DEFAULT_MAX_MESSAGE_IMAGE_BYTES, 200 * 1024 * 1024);
    assert_eq!(DEFAULT_MAX_IMAGE_PIXELS, 64_000_000);
    assert_eq!(DEFAULT_MAX_IMAGE_DIMENSION, 65_535);
    assert_eq!(
        service.image_limits,
        ImageAttachmentLimits {
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
            max_images_per_message: DEFAULT_MAX_IMAGES_PER_MESSAGE,
            max_message_image_bytes: DEFAULT_MAX_MESSAGE_IMAGE_BYTES,
            max_image_pixels: DEFAULT_MAX_IMAGE_PIXELS,
            max_image_dimension: DEFAULT_MAX_IMAGE_DIMENSION,
            media_types: vec![
                ImageMediaType::Png,
                ImageMediaType::Jpeg,
                ImageMediaType::Webp,
                ImageMediaType::Gif
            ]
        }
    );
    assert_eq!(service.normalization_policy, policy());
    assert_eq!(service.image_compression_concurrency, 2);
    let reference = ImageAttachmentRef {
        attachment_id: format!("sha256:{}", "a".repeat(64)).into(),
        media_type: ImageMediaType::Png,
        bytes: 1,
        width: 1,
        height: 1,
        name: None,
        original_dimensions: None,
    };
    assert_eq!(
        service.image_host_path(&reference).unwrap(),
        Some(hoh_util_workspace_path::node_path::join(&[
            &service.root,
            "objects",
            "aa",
            &"a".repeat(64)
        ]))
    );
    assert_eq!(
        service
            .image_host_path(&ImageAttachmentRef {
                attachment_id: "invalid".into(),
                ..reference
            })
            .unwrap_err()
            .code(),
        "INVALID_ATTACHMENT_REF"
    );
    kernel.shutdown().await;
}
#[tokio::test]
async fn resolves_and_validates_the_instance_image_compression_concurrency() {
    let kernel = hoh_kernel::Kernel::start();
    assert_eq!(
        LocalAttachmentStore::new(
            kernel.root(),
            Config {
                image_compression_concurrency: 1.0,
                ..Config::default()
            }
        )
        .unwrap()
        .image_compression_concurrency,
        1
    );
    for value in [0.0, 1.5, 9.0] {
        let result = LocalAttachmentStore::new(
            kernel.root(),
            Config {
                image_compression_concurrency: value,
                ..Config::default()
            },
        );
        let error = match result {
            Err(e) => e,
            Ok(_) => panic!("invalid concurrency accepted"),
        };
        assert_eq!(
            error.to_string(),
            "attachment-local: imageCompressionConcurrency must be an integer from 1 through 8"
        );
        assert!(hoh_error::HarnessError::of(error.as_ref()).is_none());
    }
    kernel.shutdown().await;
}
#[tokio::test]
async fn saves_and_reads_through_the_service_boundary() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = service.save_image(&input(png())).await.unwrap();
    let read = service.read_image(&reference, None).await.unwrap();
    assert_eq!(read.data, png());
    assert_eq!(read.r#ref, reference);
    let path = service.image_host_path(&reference).unwrap().unwrap();
    assert_eq!(tokio::fs::read(path).await.unwrap(), png());
    let request = service
        .read_image_request(
            &reference,
            &ImageRequestTarget {
                width: 1,
                height: 1,
                max_bytes: 1024,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(request.attachment, reference);
    let data = vec![0, 1, 2, 255];
    let file = service
        .save_file(&SaveFileAttachment {
            data: data.clone(),
            name: Some("notes.bin".into()),
        })
        .await
        .unwrap();
    let path = service.file_host_path(&file).unwrap().unwrap();
    assert!(path.contains("/files/"));
    assert_eq!(tokio::fs::read(path).await.unwrap(), data);
    let file = service
        .save_file_stream(SaveFileStreamAttachment {
            data: Box::pin(stream::once(async { Ok(data.clone()) })),
            signal: None,
            name: Some("stream.bin".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        collect(service.read_file_stream(&file, None))
            .await
            .unwrap(),
        data
    );
    kernel.shutdown().await;
}
#[tokio::test]
async fn commits_a_fully_prepared_image_batch_in_input_order() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let first = SaveImageAttachment {
        name: Some("first.png".into()),
        ..input(flat(2, 1, ImageFormat::Png, false).await)
    };
    let second = SaveImageAttachment {
        name: Some("second.png".into()),
        ..input(flat(1, 2, ImageFormat::Png, false).await)
    };
    let refs = service.save_images(&[first, second]).await.unwrap();
    assert_eq!(
        refs.iter().map(|r| r.name.as_deref()).collect::<Vec<_>>(),
        [Some("first.png"), Some("second.png")]
    );
    assert_eq!(
        futures_util::future::try_join_all(refs.iter().map(|r| service.read_image(r, None)))
            .await
            .unwrap()
            .len(),
        2
    );
    kernel.shutdown().await;
}
#[tokio::test]
async fn admits_a_16_bit_three_and_four_channel_png_as_an_8_bit_normalized_object() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    for alpha in [false, true] {
        let source = png16(7, 5, alpha).await;
        let saved = service.save_image(&input(source.clone())).await.unwrap();
        let read = service.read_image(&saved, None).await.unwrap();
        assert_ne!(read.data, source);
        let facts = image::detect_image(&read.data, None).await.unwrap();
        assert_eq!(facts.depth, "uchar");
        assert_eq!(facts.space, "srgb");
        assert_eq!(facts.has_alpha, alpha);
    }
    kernel.shutdown().await;
}
#[tokio::test]
async fn prepares_every_batch_member_before_any_write() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let result = service
        .save_images(&[input(png()), input(vec![1, 2, 3])])
        .await;
    let failure = assert_code(result, "INVALID_IMAGE");
    assert_eq!(failure.to_string(), "Unsupported or malformed image data.");
    assert!(!std::path::Path::new(&service.root).exists());
    kernel.shutdown().await;
}
#[tokio::test]
async fn validates_without_persisting_a_rejected_image_leaves_no_storage_root_behind() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    assert_code(
        service.validate_image(&input(vec![1, 2, 3])).await,
        "INVALID_IMAGE",
    );
    let limited = LocalAttachmentStore::new(
        kernel.root(),
        Config {
            dsh_home: Some(home.path().to_string_lossy().into_owned()),
            max_image_bytes: 1,
            ..Config::default()
        },
    )
    .unwrap();
    assert_code(
        limited.validate_image(&input(png())).await,
        "IMAGE_TOO_LARGE",
    );
    service.validate_image(&input(png())).await.unwrap();
    assert!(!std::path::Path::new(&service.root).exists());
    kernel.shutdown().await;
}
