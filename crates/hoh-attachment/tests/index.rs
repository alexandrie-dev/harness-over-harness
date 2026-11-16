use futures_util::{StreamExt, TryStreamExt, future::BoxFuture, stream};
use hoh_attachment::*;
use hoh_attachment::{
    AttachmentError, AttachmentErrorCode, is_attachment_error, is_image_admission_error,
};
use hoh_error::BoxError;
use hoh_error::{HarnessError, SharedError};
use hoh_timeout::{AbortController, AbortSignal};
use std::io;
use std::sync::Mutex;

struct RecordingStore {
    limits: ImageAttachmentLimits,
    calls: Mutex<Vec<String>>,
    reject_validation_at: Option<u8>,
    reject_save_at: Option<u8>,
}

impl RecordingStore {
    fn new() -> Self {
        Self {
            limits: ImageAttachmentLimits {
                max_image_bytes: 4,
                max_images_per_message: 2,
                max_message_image_bytes: 5,
                max_image_pixels: 4,
                max_image_dimension: 2000,
                media_types: vec![ImageMediaType::Png],
            },
            calls: Mutex::new(vec![]),
            reject_validation_at: None,
            reject_save_at: None,
        }
    }
}

impl AttachmentStore for RecordingStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.limits
    }

    fn validate_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move {
            let value = input.data.first().copied().unwrap_or(0);
            self.calls.lock().unwrap().push(format!("validate:{value}"));
            if Some(value) == self.reject_validation_at {
                return Err(io::Error::other(format!("invalid:{value}")).into());
            }
            Ok(())
        })
    }

    fn save_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        Box::pin(async move {
            let value = input.data.first().copied().unwrap_or(0);
            self.calls.lock().unwrap().push(format!("save:{value}"));
            if Some(value) == self.reject_save_at {
                return Err(io::Error::other(format!("write:{value}")).into());
            }
            Ok(ImageAttachmentRef {
                attachment_id: format!("sha256:{value:064}").into(),
                media_type: input.media_type,
                bytes: input.data.len() as u64,
                width: 1,
                height: 1,
                name: input.name.clone(),
                original_dimensions: None,
            })
        })
    }

    fn read_image<'a>(
        &'a self,
        _reference: &'a ImageAttachmentRef,
        _signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        Box::pin(async { Err(io::Error::other("not used").into()) })
    }

    fn read_image_request<'a>(
        &'a self,
        reference: &'a ImageAttachmentRef,
        _target: &'a ImageRequestTarget,
        _signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<RequestImageAttachment, BoxError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(format!(
                "request:{}",
                reference.name.as_deref().unwrap_or("undefined")
            ));
            Ok(RequestImageAttachment {
                variant_id: format!("sha256:{:064}", reference.bytes).into(),
                attachment: reference.clone(),
                data: vec![reference.bytes as u8],
                media_type: reference.media_type,
                bytes: 1,
                width: reference.width,
                height: reference.height,
                depth: RequestImageDepth::Uchar,
                space: RequestImageSpace::Srgb,
                has_alpha: false,
            })
        })
    }
}

fn image(value: u8) -> SaveImageAttachment {
    SaveImageAttachment {
        data: vec![value],
        media_type: ImageMediaType::Png,
        name: Some(format!("{value}.png")),
    }
}

#[tokio::test]
async fn validates_the_complete_batch_before_saving_in_input_order() {
    let store = RecordingStore::new();
    let refs = store.save_images(&[image(1), image(2)]).await.unwrap();
    assert_eq!(
        *store.calls.lock().unwrap(),
        ["validate:1", "validate:2", "save:1", "save:2"]
    );
    assert_eq!(
        refs.iter()
            .map(|reference| reference.name.as_deref())
            .collect::<Vec<_>>(),
        [Some("1.png"), Some("2.png")]
    );
}

#[tokio::test]
async fn rejects_count_aggregate_bytes_and_deployment_media_types_before_validation() {
    let store = RecordingStore::new();
    let count = store
        .save_images(&[image(1), image(2), image(3)])
        .await
        .unwrap_err();
    assert_eq!(
        HarnessError::of(count.as_ref()).unwrap().code(),
        "TOO_MANY_IMAGES"
    );
    let bytes = store
        .save_images(&[
            SaveImageAttachment {
                data: vec![1, 2, 3],
                ..image(1)
            },
            SaveImageAttachment {
                data: vec![4, 5, 6],
                ..image(2)
            },
        ])
        .await
        .unwrap_err();
    assert_eq!(
        HarnessError::of(bytes.as_ref()).unwrap().code(),
        "IMAGES_TOO_LARGE"
    );
    let media = store
        .save_images(&[SaveImageAttachment {
            media_type: ImageMediaType::Jpeg,
            ..image(1)
        }])
        .await
        .unwrap_err();
    assert_eq!(
        HarnessError::of(media.as_ref()).unwrap().code(),
        "UNSUPPORTED_IMAGE_TYPE"
    );
    assert!(store.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn starts_no_writes_when_any_member_fails_validation() {
    let mut store = RecordingStore::new();
    store.reject_validation_at = Some(2);
    assert_eq!(
        store
            .save_images(&[image(1), image(2)])
            .await
            .unwrap_err()
            .to_string(),
        "invalid:2"
    );
    assert_eq!(*store.calls.lock().unwrap(), ["validate:1", "validate:2"]);
}

#[tokio::test]
async fn returns_no_partial_references_when_storage_fails_after_an_earlier_commit() {
    let mut store = RecordingStore::new();
    store.reject_save_at = Some(2);
    assert_eq!(
        store
            .save_images(&[image(1), image(2)])
            .await
            .unwrap_err()
            .to_string(),
        "write:2"
    );
    assert_eq!(
        *store.calls.lock().unwrap(),
        ["validate:1", "validate:2", "save:1", "save:2"]
    );
}

struct RecordingFileStore {
    images: RecordingStore,
    file_input: Mutex<Option<SaveFileAttachment>>,
}

impl AttachmentStore for RecordingFileStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        self.images.image_limits()
    }
    fn validate_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        self.images.validate_image(input)
    }
    fn save_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        self.images.save_image(input)
    }
    fn read_image<'a>(
        &'a self,
        reference: &'a ImageAttachmentRef,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        self.images.read_image(reference, signal)
    }
    fn read_image_request<'a>(
        &'a self,
        reference: &'a ImageAttachmentRef,
        target: &'a ImageRequestTarget,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<RequestImageAttachment, BoxError>> {
        self.images.read_image_request(reference, target, signal)
    }
    fn save_file<'a>(
        &'a self,
        input: &'a SaveFileAttachment,
    ) -> BoxFuture<'a, Result<FileAttachmentRef, BoxError>> {
        Box::pin(async move {
            *self.file_input.lock().unwrap() = Some(input.clone());
            Ok(FileAttachmentRef {
                attachment_id: format!("sha256:{}", "cd".repeat(32)).into(),
                name: input.name.clone().unwrap_or_else(|| "unnamed".into()),
                bytes: input.data.len() as u64,
            })
        })
    }
}

struct UnsupportedProjectionStore(RecordingStore);

impl AttachmentStore for UnsupportedProjectionStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        self.0.image_limits()
    }

    fn validate_image<'a>(
        &'a self,
        _input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn save_image<'a>(
        &'a self,
        _input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        Box::pin(async { Err(io::Error::other("not used").into()) })
    }

    fn read_image<'a>(
        &'a self,
        _reference: &'a ImageAttachmentRef,
        _signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        Box::pin(async { Err(io::Error::other("not used").into()) })
    }
}

#[tokio::test]
async fn reports_unsupported_request_projection_while_preserving_cancellation() {
    let store = UnsupportedProjectionStore(RecordingStore::new());
    let reference = RecordingStore::new().save_image(&image(1)).await.unwrap();
    let target = ImageRequestTarget {
        width: 1,
        height: 1,
        max_bytes: 1,
    };
    let error = store
        .read_image_request(&reference, &target, None)
        .await
        .unwrap_err();
    let failure = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(failure.code(), "ATTACHMENT_PROJECTION_UNSUPPORTED");
    assert_eq!(
        failure.message(),
        "The mounted attachment provider cannot derive model-request images."
    );

    let controller = AbortController::new();
    let reason = SharedError::new(io::Error::other("cancel unsupported projection"));
    controller.abort_with(reason.clone());
    let signal = controller.signal();
    let error = store
        .read_image_request(&reference, &target, Some(&signal))
        .await
        .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&reason));
}

async fn read_unsupported_file(
    store: &dyn AttachmentStore,
    reference: &FileAttachmentRef,
    signal: Option<&AbortSignal>,
) -> Result<(), BoxError> {
    store
        .read_file_stream(reference, signal)
        .try_for_each(|_chunk| async {
            Err::<(), BoxError>(io::Error::other("unsupported store yielded a chunk").into())
        })
        .await
}

#[tokio::test]
async fn rejects_generic_file_storage_and_exposes_no_provider_owned_host_path_by_default() {
    let store = RecordingStore::new();
    let reference = store.save_image(&image(1)).await.unwrap();
    assert!(store.image_host_path(&reference).unwrap().is_none());
    let error = store
        .save_file(&SaveFileAttachment {
            data: vec![1],
            name: Some("notes.txt".into()),
        })
        .await
        .unwrap_err();
    let failure = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(failure.code(), "ATTACHMENT_FILES_UNSUPPORTED");
    assert_eq!(
        failure.message(),
        "The mounted attachment provider cannot store verbatim files."
    );
    let error = store
        .save_file_stream(SaveFileStreamAttachment {
            data: Box::pin(stream::once(async { Ok(vec![1]) })),
            signal: None,
            name: Some("notes.txt".into()),
        })
        .await
        .unwrap_err();
    let failure = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(failure.code(), "ATTACHMENT_FILES_UNSUPPORTED");
    assert_eq!(
        failure.message(),
        "The mounted attachment provider cannot stream verbatim files."
    );
    let reference = FileAttachmentRef {
        attachment_id: format!("sha256:{}", "ab".repeat(32)).into(),
        name: "notes.txt".into(),
        bytes: 1,
    };
    assert!(store.file_host_path(&reference).unwrap().is_none());
    let error = read_unsupported_file(&store, &reference, None)
        .await
        .unwrap_err();
    let failure = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(failure.code(), "ATTACHMENT_FILES_UNSUPPORTED");
    assert_eq!(
        failure.message(),
        "The mounted attachment provider cannot read verbatim files."
    );

    let controller = AbortController::new();
    let reason = SharedError::new(io::Error::other("cancel unsupported file read"));
    controller.abort_with(reason.clone());
    let signal = controller.signal();
    let error = read_unsupported_file(&store, &reference, Some(&signal))
        .await
        .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&reason));
}

#[tokio::test]
async fn projection_checks_cancellation_at_call_and_file_read_checks_at_first_poll() {
    let store = UnsupportedProjectionStore(RecordingStore::new());
    let reference = RecordingStore::new().save_image(&image(1)).await.unwrap();
    let target = ImageRequestTarget {
        width: 1,
        height: 1,
        max_bytes: 1,
    };
    let file = FileAttachmentRef {
        attachment_id: "file-1".into(),
        name: "notes.txt".into(),
        bytes: 1,
    };
    let controller = AbortController::new();
    let signal = controller.signal();
    let projection = store.read_image_request(&reference, &target, Some(&signal));
    let mut read = store.read_file_stream(&file, Some(&signal));
    let reason = SharedError::new(io::Error::other("cancel after constructing the operations"));
    controller.abort_with(reason.clone());

    let error = projection.await.unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "ATTACHMENT_PROJECTION_UNSUPPORTED"
    );
    let error = read.next().await.unwrap().unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&reason));
    assert!(read.next().await.is_none());
}

#[tokio::test]
async fn unsupported_streamed_writes_do_not_poll_the_source_or_override_their_error_with_cancellation()
 {
    let store = RecordingStore::new();
    let controller = AbortController::new();
    controller.abort_with(io::Error::other("cancel unsupported write"));
    let polls = std::sync::atomic::AtomicUsize::new(0);
    let source = stream::poll_fn(|_| {
        polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok(vec![1])))
    });
    let error = store
        .save_file_stream(SaveFileStreamAttachment {
            data: Box::pin(source),
            signal: Some(controller.signal()),
            name: Some("notes.txt".into()),
        })
        .await
        .unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "ATTACHMENT_FILES_UNSUPPORTED"
    );
    assert_eq!(polls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn decodes_encoded_files_through_the_service_and_exposes_attachment_errors() {
    let store = RecordingFileStore {
        images: RecordingStore::new(),
        file_input: Mutex::new(None),
    };
    let reference = store
        .admit_encoded_file(&EncodedFileAttachment {
            data: "AQID".into(),
            name: Some("notes.bin".into()),
        })
        .await
        .unwrap();
    assert_eq!(reference.name, "notes.bin");
    assert_eq!(reference.bytes, 3);
    assert_eq!(
        *store.file_input.lock().unwrap(),
        Some(SaveFileAttachment {
            data: vec![1, 2, 3],
            name: Some("notes.bin".into())
        })
    );
    assert!(store.is_attachment_error(&AttachmentError::new(
        "disk failed",
        AttachmentErrorCode::AttachmentWriteFailed
    )));
    assert!(!store.is_attachment_error(&io::Error::other("unknown failure")));
}

#[test]
fn separates_caller_correctable_image_admission_failures_from_storage_faults() {
    for code in [
        AttachmentErrorCode::InvalidImage,
        AttachmentErrorCode::InvalidImageBase64,
        AttachmentErrorCode::TooManyImages,
    ] {
        assert!(is_image_admission_error(&AttachmentError::new(
            "bad bytes",
            code
        )));
    }
    assert!(is_image_admission_error(&HarnessError::named(
        "ForeignError",
        "foreign policy error",
        "IMAGE_TOO_LARGE"
    )));
    for code in [
        AttachmentErrorCode::AttachmentCorrupt,
        AttachmentErrorCode::AttachmentWriteFailed,
    ] {
        assert!(!is_image_admission_error(&AttachmentError::new(
            "storage error",
            code
        )));
    }
    assert!(!is_image_admission_error(&io::Error::other(
        "unknown failure"
    )));
}

#[test]
fn is_a_harness_error_that_keeps_its_literal_name_and_attachment_code() {
    let cause = SharedError::new(io::Error::other("disk full"));
    let error: HarnessError =
        AttachmentError::new("disk failed", AttachmentErrorCode::AttachmentWriteFailed)
            .with_cause(cause.clone());
    assert_eq!(error.name(), "AttachmentError");
    assert_eq!(error.code(), "ATTACHMENT_WRITE_FAILED");
    assert_eq!(error.message(), "disk failed");
    let actual_cause = error
        .cause()
        .unwrap()
        .downcast_ref::<SharedError>()
        .unwrap();
    assert!(actual_cause.ptr_eq(&cause));
}

#[test]
fn recognizes_attachment_failures_from_another_package_installation_by_code() {
    assert!(is_attachment_error(&AttachmentError::new(
        "bad base64",
        AttachmentErrorCode::InvalidFileBase64
    )));
    assert!(is_attachment_error(&HarnessError::named(
        "ForeignError",
        "foreign storage error",
        "ATTACHMENT_WRITE_FAILED"
    )));
    assert!(!is_attachment_error(&HarnessError::named(
        "ForeignError",
        "other failure",
        "OTHER"
    )));
    // Plain JSON objects have no Error implementation; plain errors with no code do not match.
    assert!(!is_attachment_error(&io::Error::other(
        "ATTACHMENT_WRITE_FAILED"
    )));
}
