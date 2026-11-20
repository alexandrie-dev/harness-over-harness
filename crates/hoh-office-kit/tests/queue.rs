use hoh_error::BoxError;
use hoh_office_kit::*;
use hoh_timeout::AbortController;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

fn output(byte: u8) -> OfficeOperationOutput {
    OfficeOperationOutput::Pdf {
        pdf: vec![byte, 80, 68, 70],
        missing_fonts: vec!["Font".into()],
    }
}

fn source(
    key: &str,
    byte: u8,
    priority: OfficeKitPriority,
    reads: Arc<AtomicUsize>,
) -> QueueRequest {
    QueueRequest {
        extension: "docx".into(),
        priority,
        source: OfficeKitSource {
            key: OfficeSourceKey::from(key),
            version: "v1".into(),
            bytes: Some(1),
            read: Arc::new(move |_signal, _limit| {
                reads.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    Ok(OfficeSourceRead {
                        bytes: vec![byte],
                        version: "v1".into(),
                    })
                })
            }),
        },
        operation: OfficeOperation::Pdf,
    }
}

fn config() -> Config {
    Config::default()
}

fn error_code(error: &hoh_error::SharedError) -> Option<OfficeKitErrorCode> {
    error
        .downcast_ref::<OfficeKitError>()
        .map(|error| error.code)
}

#[tokio::test]
async fn shares_source_metadata_and_content_across_paths_with_independent_outputs() {
    let conversions = Arc::new(AtomicUsize::new(0));
    let observed = conversions.clone();
    let queue = ConversionQueue::new(
        config(),
        OfficeKitGeneration::from("test"),
        move |bytes, _, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(output(bytes[0])) })
        },
    );
    let a_reads = Arc::new(AtomicUsize::new(0));
    let b_reads = Arc::new(AtomicUsize::new(0));
    let a = source("a", 1, OfficeKitPriority::Foreground, a_reads.clone());
    let b = source("b", 1, OfficeKitPriority::Foreground, b_reads.clone());
    let first = queue.read(a.clone(), None).await.unwrap();
    let same = queue.read(a, None).await.unwrap();
    let alias = queue.read(b.clone(), None).await.unwrap();
    assert_eq!(first.cache_key, same.cache_key);
    assert_eq!(same.cache_key, alias.cache_key);
    assert_eq!(a_reads.load(Ordering::SeqCst), 1);
    assert_eq!(b_reads.load(Ordering::SeqCst), 1);
    assert_eq!(conversions.load(Ordering::SeqCst), 1);
    let mut changed = first;
    if let OfficeOperationOutput::Pdf { pdf, missing_fonts } = &mut changed.output {
        pdf[0] = 0;
        missing_fonts.clear();
    }
    assert_eq!(queue.read(b, None).await.unwrap(), alias);
    queue.dispose().await;
}

#[tokio::test]
async fn rejects_changed_and_oversized_sources_without_conversion() {
    let conversions = Arc::new(AtomicUsize::new(0));
    let observed = conversions.clone();
    let mut cfg = config();
    cfg.max_input_bytes = 2;
    cfg.max_source_bytes = 2;
    let queue = ConversionQueue::new(cfg, OfficeKitGeneration::from("test"), move |_, _, _| {
        observed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(output(1)) })
    });
    let changed = QueueRequest {
        extension: "docx".into(),
        priority: OfficeKitPriority::Foreground,
        source: OfficeKitSource {
            key: OfficeSourceKey::from("changed"),
            version: "v1".into(),
            bytes: Some(1),
            read: Arc::new(|_, _| {
                Box::pin(async {
                    Ok(OfficeSourceRead {
                        bytes: vec![1],
                        version: "v2".into(),
                    })
                })
            }),
        },
        operation: OfficeOperation::Pdf,
    };
    let error = queue.read(changed, None).await.unwrap_err();
    assert_eq!(error_code(&error), Some(OfficeKitErrorCode::SourceChanged));
    let oversized = QueueRequest {
        extension: "docx".into(),
        priority: OfficeKitPriority::Foreground,
        source: OfficeKitSource {
            key: OfficeSourceKey::from("oversized"),
            version: "v1".into(),
            bytes: Some(1),
            read: Arc::new(|_, _| {
                Box::pin(async {
                    Ok(OfficeSourceRead {
                        bytes: vec![1, 2],
                        version: "v1".into(),
                    })
                })
            }),
        },
        operation: OfficeOperation::Pdf,
    };
    let error = queue.read(oversized, None).await.unwrap_err();
    assert_eq!(error_code(&error), Some(OfficeKitErrorCode::InputTooLarge));
    assert_eq!(conversions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_known_overflow_before_read_and_reserves_the_cap_for_unknown_size() {
    let mut cfg = config();
    cfg.max_input_bytes = 2;
    cfg.max_source_bytes = 2;
    let queue = ConversionQueue::new(cfg, OfficeKitGeneration::from("test"), |bytes, _, _| {
        Box::pin(async move { Ok(output(bytes[0])) })
    });
    let reads = Arc::new(AtomicUsize::new(0));
    let mut request = source("a", 1, OfficeKitPriority::Foreground, reads.clone());
    request.source.bytes = Some(3);
    let error = queue.read(request, None).await.unwrap_err();
    assert_eq!(error_code(&error), Some(OfficeKitErrorCode::InputTooLarge));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let reserved = Arc::new(AtomicUsize::new(0));
    let capture = reserved.clone();
    let request = QueueRequest {
        extension: "docx".into(),
        priority: OfficeKitPriority::Foreground,
        source: OfficeKitSource {
            key: OfficeSourceKey::from("unknown"),
            version: "v1".into(),
            bytes: None,
            read: Arc::new(move |_, limit| {
                capture.store(limit as usize, Ordering::SeqCst);
                Box::pin(async {
                    Ok(OfficeSourceRead {
                        bytes: vec![1],
                        version: "v1".into(),
                    })
                })
            }),
        },
        operation: OfficeOperation::Pdf,
    };
    queue.read(request, None).await.unwrap();
    assert_eq!(reserved.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn refuses_disabled_background_work_but_admits_foreground() {
    let mut cfg = config();
    cfg.max_background_conversions = 0;
    let queue = ConversionQueue::new(cfg, OfficeKitGeneration::from("test"), |bytes, _, _| {
        Box::pin(async move { Ok(output(bytes[0])) })
    });
    let reads = Arc::new(AtomicUsize::new(0));
    let background = source("a", 1, OfficeKitPriority::Background, reads.clone());
    let error = queue.read(background.clone(), None).await.unwrap_err();
    assert_eq!(error_code(&error), Some(OfficeKitErrorCode::Busy));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let mut foreground = background;
    foreground.priority = OfficeKitPriority::Foreground;
    queue.read(foreground, None).await.unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn preserves_a_foreground_reader_slot() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let entered_convert = entered.clone();
    let release_convert = release.clone();
    let mut cfg = config();
    cfg.max_readers = 2;
    let queue = ConversionQueue::new(
        cfg,
        OfficeKitGeneration::from("test"),
        move |bytes, _, _| {
            let entered = entered_convert.clone();
            let release = release_convert.clone();
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                Ok(output(bytes[0]))
            })
        },
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let background = source("a", 1, OfficeKitPriority::Background, reads);
    let queued = tokio::spawn({
        let queue = queue.clone();
        let request = background.clone();
        async move { queue.read(request, None).await }
    });
    entered.notified().await;
    let error = queue.read(background.clone(), None).await.unwrap_err();
    assert_eq!(error_code(&error), Some(OfficeKitErrorCode::Busy));
    let mut foreground = background;
    foreground.priority = OfficeKitPriority::Foreground;
    let foreground = tokio::spawn({
        let queue = queue.clone();
        async move { queue.read(foreground, None).await }
    });
    release.notify_waiters();
    queued.await.unwrap().unwrap();
    foreground.await.unwrap().unwrap();
}

#[tokio::test]
async fn cancellation_releases_only_its_reader_and_the_last_reader_cancels_work() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let saw_abort = Arc::new(AtomicUsize::new(0));
    let convert_entered = entered.clone();
    let convert_release = release.clone();
    let observed_abort = saw_abort.clone();
    let queue = ConversionQueue::new(
        config(),
        OfficeKitGeneration::from("test"),
        move |bytes, _, signal| {
            let entered = convert_entered.clone();
            let release = convert_release.clone();
            let observed_abort = observed_abort.clone();
            Box::pin(async move {
                entered.notify_one();
                tokio::select! {
                    _ = release.notified() => Ok(output(bytes[0])),
                    _ = signal.cancelled() => {
                        observed_abort.fetch_add(1, Ordering::SeqCst);
                        Err(Box::new(std::io::Error::other("cancelled")) as BoxError)
                    }
                }
            })
        },
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let request = source("a", 1, OfficeKitPriority::Foreground, reads);
    let first_controller = AbortController::new();
    let first = tokio::spawn({
        let queue = queue.clone();
        let request = request.clone();
        let signal = first_controller.signal();
        async move { queue.read(request, Some(signal)).await }
    });
    entered.notified().await;
    let second = tokio::spawn({
        let queue = queue.clone();
        let request = request.clone();
        async move { queue.read(request, None).await }
    });
    first_controller.abort();
    assert!(first.await.unwrap().is_err());
    assert_eq!(saw_abort.load(Ordering::SeqCst), 0);
    release.notify_waiters();
    second.await.unwrap().unwrap();
}

#[tokio::test]
async fn separates_extensions_operations_and_generations() {
    let reads = Arc::new(AtomicUsize::new(0));
    let request = source("a", 1, OfficeKitPriority::Foreground, reads);
    let make = |generation: &str| {
        ConversionQueue::new(
            config(),
            OfficeKitGeneration::from(generation),
            |bytes, _, _| Box::pin(async move { Ok(output(bytes[0])) }),
        )
    };
    let first_queue = make("first");
    let first = first_queue.read(request.clone(), None).await.unwrap();
    let mut extension = request.clone();
    extension.extension = "pptx".into();
    let other_extension = first_queue.read(extension, None).await.unwrap();
    let mut operation = request.clone();
    operation.operation = OfficeOperation::Convert {
        format: OfficeConversionFormat::Pdf,
        sheet: None,
    };
    let other_operation = first_queue.read(operation, None).await.unwrap();
    let replacement = make("replacement").read(request, None).await.unwrap();
    assert_ne!(first.cache_key, other_extension.cache_key);
    assert_ne!(first.cache_key, other_operation.cache_key);
    assert_ne!(first.cache_key, replacement.cache_key);
}

#[tokio::test]
async fn failures_and_outputs_above_the_budget_are_not_retained() {
    let conversions = Arc::new(AtomicUsize::new(0));
    let observed = conversions.clone();
    let mut cfg = config();
    cfg.max_cached_bytes = 1;
    let queue = ConversionQueue::new(cfg, OfficeKitGeneration::from("test"), move |_, _, _| {
        let call = observed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if call == 0 {
                Err(Box::new(std::io::Error::other("engine failure")) as BoxError)
            } else {
                Ok(output(1))
            }
        })
    });
    let request = source(
        "a",
        1,
        OfficeKitPriority::Foreground,
        Arc::new(AtomicUsize::new(0)),
    );
    assert!(queue.read(request.clone(), None).await.is_err());
    queue.read(request.clone(), None).await.unwrap();
    queue.read(request, None).await.unwrap();
    assert_eq!(conversions.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn disposal_rejects_running_and_later_work_as_unavailable() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let convert_entered = entered.clone();
    let convert_release = release.clone();
    let queue = ConversionQueue::new(
        config(),
        OfficeKitGeneration::from("test"),
        move |_, _, _| {
            let entered = convert_entered.clone();
            let release = convert_release.clone();
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                Ok(output(1))
            })
        },
    );
    let request = source(
        "a",
        1,
        OfficeKitPriority::Foreground,
        Arc::new(AtomicUsize::new(0)),
    );
    let pending = tokio::spawn({
        let queue = queue.clone();
        let request = request.clone();
        async move { queue.read(request, None).await }
    });
    entered.notified().await;
    let closing = tokio::spawn({
        let queue = queue.clone();
        async move { queue.dispose().await }
    });
    let error = pending.await.unwrap().unwrap_err();
    assert_eq!(error_code(&error), Some(OfficeKitErrorCode::Unavailable));
    release.notify_waiters();
    closing.await.unwrap();
    let error = queue.read(request, None).await.unwrap_err();
    assert_eq!(error_code(&error), Some(OfficeKitErrorCode::Unavailable));
}

#[test]
fn image_resolution_above_the_int32_ceiling_is_rejected() {
    let mut cfg = config();
    cfg.max_image_resolution = 2_147_483_647;
    cfg.validate().unwrap();
    cfg.max_image_resolution = 2_147_483_648;
    assert!(cfg.validate().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_content_under_many_keys_never_sees_unavailable_even_when_uncached() {
    let mut cfg = config();
    cfg.max_cached_bytes = 1;
    cfg.max_readers = 1_000;
    cfg.max_queued_jobs = 1_000;
    cfg.max_source_entries = 1_000;
    cfg.max_concurrent_conversions = 4;
    cfg.max_source_bytes = 1_000_000;
    let queue = ConversionQueue::new(cfg, OfficeKitGeneration::from("test"), |bytes, _, _| {
        Box::pin(async move {
            tokio::task::yield_now().await;
            Ok(output(bytes[0]))
        })
    });
    for round in 0..50 {
        let mut tasks = Vec::new();
        for index in 0..40 {
            let queue = queue.clone();
            let request = source(
                &format!("k-{round}-{index}"),
                7,
                OfficeKitPriority::Foreground,
                Arc::new(AtomicUsize::new(0)),
            );
            tasks.push(tokio::spawn(async move { queue.read(request, None).await }));
        }
        for task in tasks {
            if let Err(error) = task.await.unwrap() {
                panic!("unexpected failure {:?}", error_code(&error));
            }
        }
    }
    queue.dispose().await;
}

#[tokio::test]
async fn a_foreground_read_evicts_a_queued_background_job_when_the_queue_is_full() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (entered_convert, release_convert) = (entered.clone(), release.clone());
    let mut cfg = config();
    cfg.max_concurrent_conversions = 1;
    cfg.max_queued_jobs = 1;
    cfg.max_background_conversions = 1;
    let queue = ConversionQueue::new(
        cfg,
        OfficeKitGeneration::from("test"),
        move |bytes, _, _| {
            let (entered, release) = (entered_convert.clone(), release_convert.clone());
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                Ok(output(bytes[0]))
            })
        },
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let running = tokio::spawn({
        let queue = queue.clone();
        let request = source("run", 1, OfficeKitPriority::Foreground, reads.clone());
        async move { queue.read(request, None).await }
    });
    entered.notified().await;
    let background = tokio::spawn({
        let queue = queue.clone();
        let request = source("bg", 2, OfficeKitPriority::Background, reads.clone());
        async move { queue.read(request, None).await }
    });
    tokio::task::yield_now().await;
    // A second background job finds the queue full and is refused.
    let refused = queue
        .read(
            source("bg2", 3, OfficeKitPriority::Background, reads.clone()),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error_code(&refused), Some(OfficeKitErrorCode::Busy));
    let foreground = tokio::spawn({
        let queue = queue.clone();
        let request = source("fg", 4, OfficeKitPriority::Foreground, reads);
        async move { queue.read(request, None).await }
    });
    let evicted = background.await.unwrap().unwrap_err();
    assert_eq!(error_code(&evicted), Some(OfficeKitErrorCode::Busy));
    for _ in 0..2 {
        release.notify_waiters();
        tokio::task::yield_now().await;
    }
    release.notify_one();
    running.await.unwrap().unwrap();
    release.notify_one();
    foreground.await.unwrap().unwrap();
}

#[tokio::test]
async fn the_cache_keeps_at_most_max_cached_entries() {
    let conversions = Arc::new(AtomicUsize::new(0));
    let observed = conversions.clone();
    let mut cfg = config();
    cfg.max_cached_entries = 1;
    let queue = ConversionQueue::new(
        cfg,
        OfficeKitGeneration::from("test"),
        move |bytes, _, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(output(bytes[0])) })
        },
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let a = source("a", 1, OfficeKitPriority::Foreground, reads.clone());
    let b = source("b", 2, OfficeKitPriority::Foreground, reads);
    queue.read(a.clone(), None).await.unwrap();
    queue.read(b, None).await.unwrap();
    queue.read(a, None).await.unwrap();
    assert_eq!(conversions.load(Ordering::SeqCst), 3);
}
