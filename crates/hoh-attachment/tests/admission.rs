use futures_util::future::BoxFuture;
use hoh_attachment::*;
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_timeout::AbortSignal;
use std::sync::Mutex;

const PNG: &str = "AAAA";

struct DelegationStore {
    limits: ImageAttachmentLimits,
    image_batches: Mutex<Vec<Vec<SaveImageAttachment>>>,
    file_inputs: Mutex<Vec<SaveFileAttachment>>,
    rejected: Option<SharedError>,
}

impl DelegationStore {
    fn new() -> Self {
        Self {
            limits: ImageAttachmentLimits {
                max_image_bytes: 100,
                max_images_per_message: 2,
                max_message_image_bytes: 5,
                max_image_pixels: 100,
                max_image_dimension: 2000,
                media_types: vec![ImageMediaType::Png],
            },
            image_batches: Mutex::new(Vec::new()),
            file_inputs: Mutex::new(Vec::new()),
            rejected: None,
        }
    }
}

impl AttachmentStore for DelegationStore {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.limits
    }

    fn validate_image<'a>(
        &'a self,
        _input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(std::io::Error::other("not used").into()) })
    }

    fn save_image<'a>(
        &'a self,
        _input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        Box::pin(async { Err(std::io::Error::other("not used").into()) })
    }

    fn read_image<'a>(
        &'a self,
        _reference: &'a ImageAttachmentRef,
        _signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        Box::pin(async { Err(std::io::Error::other("not used").into()) })
    }

    fn save_images<'a>(
        &'a self,
        inputs: &'a [SaveImageAttachment],
    ) -> BoxFuture<'a, Result<Vec<ImageAttachmentRef>, BoxError>> {
        Box::pin(async move {
            self.image_batches.lock().unwrap().push(inputs.to_vec());
            if let Some(error) = &self.rejected {
                return Err(Box::new(error.clone()) as BoxError);
            }
            Ok(inputs
                .iter()
                .enumerate()
                .map(|(index, input)| ImageAttachmentRef {
                    attachment_id: format!("att-{}", index + 1).into(),
                    media_type: input.media_type,
                    bytes: input.data.len() as u64,
                    width: 1,
                    height: 1,
                    name: input.name.clone(),
                    original_dimensions: None,
                })
                .collect())
        })
    }

    fn save_file<'a>(
        &'a self,
        input: &'a SaveFileAttachment,
    ) -> BoxFuture<'a, Result<FileAttachmentRef, BoxError>> {
        Box::pin(async move {
            self.file_inputs.lock().unwrap().push(input.clone());
            Ok(FileAttachmentRef {
                attachment_id: "file-1".into(),
                name: input.name.clone().unwrap_or_else(|| "file".into()),
                bytes: input.data.len() as u64,
            })
        })
    }
}

fn upload(media_type: ImageMediaType, data: &str, name: Option<&str>) -> EncodedImageAttachment {
    EncodedImageAttachment {
        media_type,
        data: data.into(),
        name: name.map(str::to_owned),
    }
}

fn file_ref() -> FileAttachmentRef {
    FileAttachmentRef {
        attachment_id: "file-1".into(),
        name: "notes.md".into(),
        bytes: 12,
    }
}

fn stored_ref() -> ImageAttachmentRef {
    ImageAttachmentRef {
        attachment_id: "stored-1".into(),
        media_type: ImageMediaType::Png,
        bytes: 8,
        width: 2,
        height: 2,
        name: None,
        original_dimensions: None,
    }
}

fn encoded(data: &str) -> AttachmentAdmissionPart {
    AttachmentAdmissionPart::EncodedImage {
        media_type: ImageMediaType::Png,
        data: data.into(),
        name: None,
    }
}

fn stored(encoded_bytes: u64) -> AttachmentAdmissionPart {
    AttachmentAdmissionPart::StoredImage {
        attachment: stored_ref(),
        encoded_bytes,
    }
}

fn assert_code(error: &BoxError, code: &str) {
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.name(), "AttachmentError");
    assert_eq!(error.code(), code);
}

#[tokio::test]
async fn decodes_every_member_and_delegates_one_ordered_batch_to_save_images() {
    let store = DelegationStore::new();
    let refs = admit_encoded_images(
        &store,
        &[
            upload(ImageMediaType::Png, PNG, Some("first.png")),
            upload(ImageMediaType::Jpeg, PNG, Some("second.jpg")),
        ],
    )
    .await
    .unwrap();
    let batches = store.image_batches.lock().unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(
        batches[0],
        [
            SaveImageAttachment {
                data: vec![0, 0, 0],
                media_type: ImageMediaType::Png,
                name: Some("first.png".into())
            },
            SaveImageAttachment {
                data: vec![0, 0, 0],
                media_type: ImageMediaType::Jpeg,
                name: Some("second.jpg".into())
            },
        ]
    );
    assert_eq!(
        refs.iter()
            .map(|reference| reference.attachment_id.as_str())
            .collect::<Vec<_>>(),
        ["att-1", "att-2"]
    );
}

#[tokio::test]
async fn omits_the_name_from_store_inputs_when_the_upload_has_none() {
    let store = DelegationStore::new();
    let refs = admit_encoded_images(&store, &[upload(ImageMediaType::Webp, PNG, None)])
        .await
        .unwrap();
    assert!(store.image_batches.lock().unwrap()[0][0].name.is_none());
    assert!(refs[0].name.is_none());
}

#[tokio::test]
async fn delegates_an_empty_batch_unchanged() {
    let store = DelegationStore::new();
    assert!(admit_encoded_images(&store, &[]).await.unwrap().is_empty());
    assert_eq!(
        *store.image_batches.lock().unwrap(),
        vec![Vec::<SaveImageAttachment>::new()]
    );
}

#[tokio::test]
async fn rejects_non_canonical_and_empty_base64_payloads_before_any_store_call() {
    let store = DelegationStore::new();
    for data in ["", "AAA", "!!!!"] {
        let error = admit_encoded_images(&store, &[upload(ImageMediaType::Png, data, None)])
            .await
            .unwrap_err();
        assert_code(&error, "INVALID_IMAGE_BASE64");
    }
    assert!(store.image_batches.lock().unwrap().is_empty());
}

#[tokio::test]
async fn propagates_the_store_batch_rejection_unchanged() {
    let mut store = DelegationStore::new();
    let refused = SharedError::new(HarnessError::named(
        "ForeignError",
        "Image batch exceeds the configured image-count limit.",
        "TOO_MANY_IMAGES",
    ));
    store.rejected = Some(refused.clone());
    let error = admit_encoded_images(&store, &[upload(ImageMediaType::Png, PNG, None)])
        .await
        .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&refused));
}

#[tokio::test]
async fn decodes_canonical_base64_and_delegates_verbatim_commit_to_save_file() {
    let store = DelegationStore::new();
    let reference = admit_encoded_file(
        &store,
        &EncodedFileAttachment {
            data: PNG.into(),
            name: Some("blob.bin".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        *store.file_inputs.lock().unwrap(),
        [SaveFileAttachment {
            data: vec![0, 0, 0],
            name: Some("blob.bin".into())
        }]
    );
    assert_eq!(reference.bytes, 3);
}

#[tokio::test]
async fn accepts_an_empty_payload_as_a_zero_byte_file_and_omits_an_absent_name() {
    let store = DelegationStore::new();
    let reference = admit_encoded_file(
        &store,
        &EncodedFileAttachment {
            data: String::new(),
            name: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        *store.file_inputs.lock().unwrap(),
        [SaveFileAttachment {
            data: vec![],
            name: None
        }]
    );
    assert_eq!(reference.bytes, 0);
}

#[tokio::test]
async fn rejects_non_canonical_base64_without_touching_the_store() {
    let store = DelegationStore::new();
    let error = admit_encoded_file(
        &store,
        &EncodedFileAttachment {
            data: "not base64!!".into(),
            name: None,
        },
    )
    .await
    .unwrap_err();
    assert_code(&error, "INVALID_FILE_BASE64");
    assert!(store.file_inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn passes_through_text_and_durable_files_without_touching_image_storage() {
    let store = DelegationStore::new();
    let admitted = store
        .admit_prompt_content(&[
            AttachmentAdmissionPart::Text {
                text: "hello".into(),
            },
            AttachmentAdmissionPart::File {
                attachment: file_ref(),
            },
        ])
        .await
        .unwrap();
    assert_eq!(
        admitted,
        [
            AdmittedPromptContentPart::Text {
                text: "hello".into()
            },
            AdmittedPromptContentPart::File {
                attachment: file_ref()
            }
        ]
    );
    assert!(store.image_batches.lock().unwrap().is_empty());
}

#[tokio::test]
async fn replaces_images_and_passes_through_files_in_part_order() {
    let store = DelegationStore::new();
    let admitted = store
        .admit_prompt_content(&[
            encoded("AQ=="),
            AttachmentAdmissionPart::File {
                attachment: file_ref(),
            },
            AttachmentAdmissionPart::Text {
                text: "between".into(),
            },
            encoded("Ag=="),
        ])
        .await
        .unwrap();
    let reference = |id: &str| ImageAttachmentRef {
        attachment_id: id.into(),
        media_type: ImageMediaType::Png,
        bytes: 1,
        width: 1,
        height: 1,
        name: None,
        original_dimensions: None,
    };
    assert_eq!(
        admitted,
        [
            AdmittedPromptContentPart::Image {
                attachment: reference("att-1")
            },
            AdmittedPromptContentPart::File {
                attachment: file_ref()
            },
            AdmittedPromptContentPart::Text {
                text: "between".into()
            },
            AdmittedPromptContentPart::Image {
                attachment: reference("att-2")
            },
        ]
    );
}

#[tokio::test]
async fn passes_stored_images_through_in_part_order_and_stores_only_the_encoded_ones() {
    let store = DelegationStore::new();
    let admitted = store
        .admit_prompt_content(&[
            stored(4),
            AttachmentAdmissionPart::Text {
                text: "between".into(),
            },
            encoded("AQ=="),
        ])
        .await
        .unwrap();
    assert_eq!(
        admitted,
        [
            AdmittedPromptContentPart::Image {
                attachment: stored_ref()
            },
            AdmittedPromptContentPart::Text {
                text: "between".into()
            },
            AdmittedPromptContentPart::Image {
                attachment: ImageAttachmentRef {
                    attachment_id: "att-1".into(),
                    media_type: ImageMediaType::Png,
                    bytes: 1,
                    width: 1,
                    height: 1,
                    name: None,
                    original_dimensions: None
                }
            },
        ]
    );
    let batches = store.image_batches.lock().unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 1);
}

#[tokio::test]
async fn counts_stored_images_toward_the_image_count_limit_before_storing_anything() {
    let mut store = DelegationStore::new();
    store.limits.max_images_per_message = 1;
    store.limits.max_message_image_bytes = 100;
    for content in [
        vec![stored(1), encoded("AQ==")],
        vec![
            stored(1),
            AttachmentAdmissionPart::StoredImage {
                attachment: ImageAttachmentRef {
                    attachment_id: "stored-2".into(),
                    ..stored_ref()
                },
                encoded_bytes: 1,
            },
        ],
    ] {
        let error = store.admit_prompt_content(&content).await.unwrap_err();
        assert_code(&error, "TOO_MANY_IMAGES");
    }
    assert!(store.image_batches.lock().unwrap().is_empty());
}

#[tokio::test]
async fn counts_stored_upload_bytes_with_encoded_bytes_toward_the_message_byte_limit_before_storing_anything()
 {
    let mut store = DelegationStore::new();
    store.limits.max_images_per_message = 4;
    for content in [
        vec![
            stored(3),
            AttachmentAdmissionPart::StoredImage {
                attachment: ImageAttachmentRef {
                    attachment_id: "stored-2".into(),
                    ..stored_ref()
                },
                encoded_bytes: 3,
            },
        ],
        vec![stored(3), encoded(PNG)],
    ] {
        let error = store.admit_prompt_content(&content).await.unwrap_err();
        assert_code(&error, "IMAGES_TOO_LARGE");
    }
    assert!(store.image_batches.lock().unwrap().is_empty());
}
