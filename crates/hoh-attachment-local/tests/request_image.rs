use crate::{test_support::*, *};
use ::image::ImageFormat;
use hoh_error::SharedError;
use hoh_timeout::AbortController;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
fn target(width: u64, height: u64, max_bytes: u64) -> ImageRequestTarget {
    ImageRequestTarget {
        width,
        height,
        max_bytes,
    }
}
async fn save(service: &LocalAttachmentStore, width: u32, height: u32) -> ImageAttachmentRef {
    service
        .save_image(&input(flat(width, height, ImageFormat::Png, false).await))
        .await
        .unwrap()
}
fn cache_path(home: &tempfile::TempDir, variant: &ImageVariantId) -> String {
    let hash = &variant.as_str()[7..];
    hoh_util_workspace_path::node_path::join(&[
        &home.path().to_string_lossy(),
        "cache",
        "attachments",
        "request-images",
        &hash[..2],
        hash,
    ])
}

#[tokio::test]
async fn rebuilds_a_cleared_cache_without_moving_or_losing_durable_attachments() {
    let home = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();
    let environment = HashMap::from([(
        "DSH_HOME".into(),
        fallback.path().to_string_lossy().into_owned(),
    )]);
    let kernel = hoh_kernel::Kernel::start();
    let config = Config {
        dsh_home: Some(home.path().to_string_lossy().into_owned()),
        ..Config::default()
    };
    let service = LocalAttachmentStore::new_with_environment(
        kernel.root(),
        config.clone(),
        Some(&environment),
    )
    .unwrap();
    let attachment = save(&service, 64, 32).await;
    let stored = service.read_image(&attachment, None).await.unwrap();
    let file = service
        .save_file(&SaveFileAttachment {
            data: vec![0, 1, 2, 255],
            name: Some("notes.bin".into()),
        })
        .await
        .unwrap();
    let policy = target(22, 11, 4096);
    let initial = service
        .read_image_request(&attachment, &policy, None)
        .await
        .unwrap();
    let path = cache_path(&home, &initial.variant_id);
    assert_eq!(tokio::fs::read(&path).await.unwrap(), initial.data);
    assert!(
        !std::path::Path::new(&service.root)
            .join("request-images")
            .exists()
    );
    tokio::fs::remove_dir_all(home.path().join("cache"))
        .await
        .unwrap();
    let reopened =
        LocalAttachmentStore::new_with_environment(kernel.root(), config, Some(&environment))
            .unwrap();
    assert_eq!(
        reopened.read_image(&attachment, None).await.unwrap(),
        stored
    );
    assert_eq!(
        tokio::fs::read(reopened.file_host_path(&file).unwrap().unwrap())
            .await
            .unwrap(),
        [0, 1, 2, 255]
    );
    assert_eq!(
        reopened
            .read_image_request(&attachment, &policy, None)
            .await
            .unwrap(),
        initial
    );
    assert_eq!(tokio::fs::read(&path).await.unwrap(), initial.data);
    assert!(
        tokio::fs::read_dir(fallback.path())
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none()
    );
    kernel.shutdown().await;
}
#[tokio::test]
async fn passes_through_an_in_budget_attachment_and_composes_ordered_request_reads() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let first = save(&service, 8, 4).await;
    let second = save(&service, 4, 8).await;
    let policy = target(8, 8, 1 << 20);
    let request = service
        .read_image_request(&first, &policy, None)
        .await
        .unwrap();
    assert_eq!(
        request.data,
        service.read_image(&first, None).await.unwrap().data
    );
    let refs = [first.clone(), second.clone()];
    let batch = futures_util::future::try_join_all(
        refs.iter()
            .map(|reference| service.read_image_request(reference, &policy, None)),
    )
    .await
    .unwrap();
    assert_eq!(
        batch
            .iter()
            .map(|r| r.attachment.attachment_id.clone())
            .collect::<Vec<_>>(),
        [first.attachment_id, second.attachment_id]
    );
    kernel.shutdown().await;
}
#[tokio::test]
async fn rejects_invalid_request_targets() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = save(&service, 8, 4).await;
    for (policy, message) in [
        (target(0, 4, 100), "Image request width"),
        (target(8, 0, 100), "Image request height"),
        (target(8, 4, 0), "Image request maxBytes"),
    ] {
        let error = assert_code(
            service.read_image_request(&reference, &policy, None).await,
            "INVALID_ATTACHMENT_REF",
        );
        assert_eq!(
            error.to_string(),
            format!("{message} must be a positive integer.")
        );
    }
    assert!(
        serde_json::from_value::<ImageRequestTarget>(
            serde_json::json!({"width":8,"height":1.5,"maxBytes":100})
        )
        .is_err()
    );
    kernel.shutdown().await;
}
#[tokio::test]
async fn resizes_by_the_long_edge_to_the_exact_target_and_keys_the_cache_by_target() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let max = 2 << 20;
    let square = save(&service, 2048, 2048).await;
    let small = save(&service, 800, 800).await;
    let thin = save(&service, 8000, 40).await;
    let wide = save(&service, 1920, 1080).await;
    let tall = save(&service, 1080, 1920).await;
    for (source, policy, dimensions) in [
        (&square, target(1302, 1302, max), (1302, 1302)),
        (&small, target(800, 800, max), (800, 800)),
        (&thin, target(4096, 20, max), (4096, 20)),
        (&wide, target(1708, 961, max), (1708, 961)),
        (&tall, target(961, 1708, max), (961, 1708)),
        (&thin, target(9000, 45, max), (8000, 40)),
    ] {
        let request = service
            .read_image_request(source, &policy, None)
            .await
            .unwrap();
        assert_eq!((request.width, request.height), dimensions);
        if std::ptr::eq(source, &small) {
            assert_eq!(request.media_type, ImageMediaType::Png);
            assert_eq!(
                request.data,
                service.read_image(source, None).await.unwrap().data
            );
        }
    }
    assert_ne!(
        request_image_variant_id(&square, &target(1302, 1302, max)),
        request_image_variant_id(&square, &target(1024, 1024, max))
    );
    assert_ne!(
        request_image_variant_id(&thin, &target(4096, 20, max)),
        request_image_variant_id(&thin, &target(9000, 45, max))
    );
    kernel.shutdown().await;
}
#[tokio::test]
async fn encodes_the_rounded_short_edge_at_the_route_target() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = save(&service, 1224, 1429).await;
    let request = service
        .read_image_request(&reference, &target(1187, 1386, 2 << 20), None)
        .await
        .unwrap();
    assert_eq!((request.width, request.height), (1187, 1386));
    let facts = image::probe_image(&request.data).await.unwrap();
    assert_eq!((facts.width, facts.height), (1187, 1386));
    kernel.shutdown().await;
}
#[tokio::test]
async fn keeps_the_smallest_ladder_output_when_the_encoded_byte_target_is_unreachable() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = save(&service, 1, 1).await;
    let request = service
        .read_image_request(&reference, &target(1, 1, 1), None)
        .await
        .unwrap();
    assert_eq!(request.media_type, ImageMediaType::Jpeg);
    assert!(request.bytes > 1);
    assert_eq!((request.width, request.height), (1, 1));
    kernel.shutdown().await;
}
#[tokio::test]
async fn regenerates_invalid_oversized_incompatible_or_mismatched_cached_variants() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = save(&service, 64, 32).await;
    let policy = target(22, 11, 4096);
    let initial = service
        .read_image_request(&reference, &policy, None)
        .await
        .unwrap();
    let path = cache_path(&home, &initial.variant_id);
    for bad in [
        noisy(64, 64, ImageFormat::Png, None).await,
        png16(16, 8, false).await,
        cmyk_jpeg(),
        flat(23, 11, ImageFormat::Png, false).await,
        flat(16, 8, ImageFormat::Png, true).await,
        vec![1, 2, 3],
    ] {
        tokio::fs::write(&path, bad).await.unwrap();
        assert_eq!(
            service
                .read_image_request(&reference, &policy, None)
                .await
                .unwrap()
                .data,
            initial.data
        );
    }
    kernel.shutdown().await;
}
#[tokio::test]
async fn derives_stable_square_and_wide_previews_and_separates_route_budgets_in_the_cache_key() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let square = save(&service, 2048, 2048).await;
    let wide = save(&service, 2048, 1024).await;
    let first = service
        .read_image_request(&square, &target(800, 800, 1 << 20), None)
        .await
        .unwrap();
    let policy = target(1130, 565, 1 << 20);
    let second = service
        .read_image_request(&wide, &policy, None)
        .await
        .unwrap();
    let repeated = service
        .read_image_request(&wide, &policy, None)
        .await
        .unwrap();
    let low = service
        .read_image_request(&wide, &target(724, 362, 1 << 20), None)
        .await
        .unwrap();
    assert_eq!((first.width, first.height), (800, 800));
    assert_eq!((second.width, second.height), (1130, 565));
    assert_eq!(second, repeated);
    assert_ne!(low.variant_id, second.variant_id);
    assert!(low.width * low.height <= 512 * 512 + low.width);
    kernel.shutdown().await;
}
#[tokio::test]
async fn routes_opaque_pixels_to_jpeg_and_preserves_alpha_on_the_webp_ladder() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let photo = service
        .save_image(&input(noisy(256, 256, ImageFormat::Png, None).await))
        .await
        .unwrap();
    let alpha = service
        .save_image(&input(noisy(256, 256, ImageFormat::Png, Some(true)).await))
        .await
        .unwrap();
    let photo = service
        .read_image_request(&photo, &target(128, 128, 1 << 20), None)
        .await
        .unwrap();
    let alpha = service
        .read_image_request(&alpha, &target(128, 128, 4096), None)
        .await
        .unwrap();
    assert_eq!(photo.media_type, ImageMediaType::Jpeg);
    assert_eq!(alpha.media_type, ImageMediaType::Webp);
    assert!(alpha.bytes > 4096);
    assert_eq!((alpha.width, alpha.height), (128, 128));
    let facts = image::probe_image(&alpha.data).await.unwrap();
    assert!(facts.has_alpha);
    assert_eq!(facts.depth, "uchar");
    assert_eq!(facts.space, "srgb");
    kernel.shutdown().await;
}
#[tokio::test]
async fn projects_a_16_bit_three_and_four_channel_png_as_a_bounded_8_bit_request_image() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    for alpha in [false, true] {
        let reference = service
            .save_image(&input(png16(64, 32, alpha).await))
            .await
            .unwrap();
        let request = service
            .read_image_request(&reference, &target(22, 11, 1 << 20), None)
            .await
            .unwrap();
        assert!(request.bytes <= 1 << 20);
        assert!(request.width * request.height <= 256);
        let facts = image::probe_image(&request.data).await.unwrap();
        assert_eq!(facts.has_alpha, alpha);
        assert_eq!(facts.depth, "uchar");
        assert_eq!(facts.space, "srgb");
    }
    kernel.shutdown().await;
}
#[tokio::test]
async fn accepts_a_resized_webp_request_version_that_omits_an_all_opaque_alpha_plane() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = service
        .save_image(&input(noisy(64, 32, ImageFormat::Png, Some(false)).await))
        .await
        .unwrap();
    let request = service
        .read_image_request(&reference, &target(22, 11, 1 << 20), None)
        .await
        .unwrap();
    assert_eq!(request.media_type, ImageMediaType::Webp);
    assert!(!image::probe_image(&request.data).await.unwrap().has_alpha);
    kernel.shutdown().await;
}
#[tokio::test]
async fn keeps_a_complex_640000_pixel_request_version_below_1_mib() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = service
        .save_image(&input(noisy(1024, 1024, ImageFormat::Png, None).await))
        .await
        .unwrap();
    let request = service
        .read_image_request(&reference, &target(800, 800, 1 << 20), None)
        .await
        .unwrap();
    assert_eq!((request.width, request.height), (800, 800));
    assert!(request.bytes <= 1 << 20);
    kernel.shutdown().await;
}
#[tokio::test]
async fn shares_one_request_transform_between_concurrent_callers_without_sharing_cancellation() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = save(&service, 64, 32).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let root = service.root.clone();
    *service.read_hook.lock().unwrap() = Some(Arc::new(move |reference, signal| {
        counter.fetch_add(1, Ordering::SeqCst);
        let root = root.clone();
        Box::pin(async move { read_image_file(&root, &reference, Some(&signal)).await })
    }));
    let controller = AbortController::new();
    let signal = controller.signal();
    let policy = target(22, 11, 1 << 20);
    let cancelled = service.read_image_request(&reference, &policy, Some(&signal));
    let completed = service.read_image_request(&reference, &policy, None);
    let reason = SharedError::new(std::io::Error::other("cancel one waiter"));
    controller.abort_with(reason.clone());
    let failure = cancelled.await.unwrap_err();
    assert!(SharedError::of(failure.as_ref()).unwrap().ptr_eq(&reason));
    assert_eq!(completed.await.unwrap().width, 22);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}
#[tokio::test]
async fn aborts_the_underlying_request_transform_after_its_only_waiter_cancels() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = save(&service, 64, 32).await;
    let observed = Arc::new(Mutex::new(None));
    let observer = observed.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let start = started.clone();
    *service.read_hook.lock().unwrap() = Some(Arc::new(move |_, signal| {
        *observer.lock().unwrap() = Some(signal.clone());
        start.notify_one();
        Box::pin(async move { Err(signal.cancelled().await.into()) })
    }));
    let controller = AbortController::new();
    let signal = controller.signal();
    let policy = target(22, 11, 1 << 20);
    let request = service.read_image_request(&reference, &policy, Some(&signal));
    started.notified().await;
    let reason = SharedError::new(std::io::Error::other("cancel only transform waiter"));
    controller.abort_with(reason.clone());
    assert!(
        observed
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .reason()
            .unwrap()
            .ptr_eq(&reason)
    );
    let failure = request.await.unwrap_err();
    assert!(SharedError::of(failure.as_ref()).unwrap().ptr_eq(&reason));
    kernel.shutdown().await;
}
#[tokio::test]
async fn the_last_cancelled_waiter_supplies_the_shared_abort_reason_before_polling() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = save(&service, 8, 4).await;
    let observed = Arc::new(Mutex::new(None));
    let observer = observed.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let start = started.clone();
    *service.read_hook.lock().unwrap() = Some(Arc::new(move |_, signal| {
        *observer.lock().unwrap() = Some(signal.clone());
        start.notify_one();
        Box::pin(async move { Err(signal.cancelled().await.into()) })
    }));
    let first = AbortController::new();
    let second = AbortController::new();
    let first_signal = first.signal();
    let second_signal = second.signal();
    let policy = target(4, 2, 4096);
    let first_request = service.read_image_request(&reference, &policy, Some(&first_signal));
    let second_request = service.read_image_request(&reference, &policy, Some(&second_signal));
    started.notified().await;
    first.abort();
    assert!(!observed.lock().unwrap().as_ref().unwrap().aborted());
    let last_reason = SharedError::new(std::io::Error::other("last waiter"));
    second.abort_with(last_reason.clone());
    assert!(
        observed
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .reason()
            .unwrap()
            .ptr_eq(&last_reason)
    );
    assert!(first_request.await.is_err());
    assert!(
        SharedError::of(second_request.await.unwrap_err().as_ref())
            .unwrap()
            .ptr_eq(&last_reason)
    );
    kernel.shutdown().await;
}
#[tokio::test]
async fn normalizes_a_non_error_cancellation_and_replaces_an_aborted_shared_transform() {
    let home = tempfile::tempdir().unwrap();
    let (kernel, service) = local(&home);
    let reference = save(&service, 64, 32).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let start = started.clone();
    let root = service.root.clone();
    *service.read_hook.lock().unwrap() = Some(Arc::new(move |reference, signal| {
        let first = counter.fetch_add(1, Ordering::SeqCst) == 0;
        let root = root.clone();
        start.notify_one();
        Box::pin(async move {
            if first {
                Err(signal.cancelled().await.into())
            } else {
                read_image_file(&root, &reference, Some(&signal)).await
            }
        })
    }));
    let controller = AbortController::new();
    let signal = controller.signal();
    let policy = target(22, 11, 1 << 20);
    let cancelled = service.read_image_request(&reference, &policy, Some(&signal));
    started.notified().await;
    let reason = SharedError::new(NonErrorCancellationReason {
        cause: serde_json::json!("cancelled"),
    });
    controller.abort_with(reason.clone());
    let replacement = service.read_image_request(&reference, &policy, None);
    let failure = cancelled.await.unwrap_err();
    let shared = SharedError::of(failure.as_ref()).unwrap();
    let failure = shared.downcast_ref::<NonErrorCancellationReason>().unwrap();
    assert_eq!(
        failure.to_string(),
        "Attachment request cancelled with a non-Error reason."
    );
    assert_eq!(failure.cause, "cancelled");
    assert_eq!(replacement.await.unwrap().width, 22);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    kernel.shutdown().await;
}
