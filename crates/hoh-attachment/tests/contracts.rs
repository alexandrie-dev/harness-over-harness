use hoh_attachment::*;
use hoh_error::{HarnessError, SharedError};
use serde_json::{json, to_string, to_value};

fn reference() -> ImageAttachmentRef {
    ImageAttachmentRef {
        attachment_id: "sha256:abc".into(),
        media_type: ImageMediaType::Png,
        bytes: 3,
        width: 2,
        height: 1,
        name: None,
        original_dimensions: None,
    }
}

#[test]
fn durable_references_preserve_wire_key_order_and_omit_absent_display_fields() {
    let mut image = reference();
    assert_eq!(
        to_string(&image).unwrap(),
        r#"{"attachmentId":"sha256:abc","mediaType":"image/png","bytes":3,"width":2,"height":1}"#
    );
    image.name = Some("view.png".into());
    image.original_dimensions = Some(ProjectedDimensions {
        width: 4,
        height: 2,
    });
    assert_eq!(
        to_string(&image).unwrap(),
        r#"{"attachmentId":"sha256:abc","mediaType":"image/png","bytes":3,"width":2,"height":1,"name":"view.png","originalDimensions":{"width":4,"height":2}}"#
    );
    assert_eq!(
        to_string(&FileAttachmentRef {
            attachment_id: "file-1".into(),
            name: "notes.md".into(),
            bytes: 12
        })
        .unwrap(),
        r#"{"attachmentId":"file-1","name":"notes.md","bytes":12}"#
    );
}

#[test]
fn encoded_and_stored_image_admission_forms_share_the_image_wire_tag() {
    let parts = [
        AttachmentAdmissionPart::Text {
            text: "before".into(),
        },
        AttachmentAdmissionPart::EncodedImage {
            media_type: ImageMediaType::Gif,
            data: "AQ==".into(),
            name: None,
        },
        AttachmentAdmissionPart::StoredImage {
            attachment: reference(),
            encoded_bytes: 4,
        },
        AttachmentAdmissionPart::File {
            attachment: FileAttachmentRef {
                attachment_id: "file-1".into(),
                name: "x".into(),
                bytes: 0,
            },
        },
    ];
    for part in parts {
        let value = to_value(&part).unwrap();
        let round_trip: AttachmentAdmissionPart = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip, part);
    }
    assert_eq!(
        to_string(&AttachmentAdmissionPart::EncodedImage {
            media_type: ImageMediaType::Gif,
            data: "AQ==".into(),
            name: None
        })
        .unwrap(),
        r#"{"type":"image","mediaType":"image/gif","data":"AQ=="}"#
    );
    assert_eq!(
        to_value(AttachmentAdmissionPart::StoredImage {
            attachment: reference(),
            encoded_bytes: 4
        })
        .unwrap()["encodedBytes"],
        4
    );
}

#[test]
fn wire_uploads_limits_and_request_targets_keep_original_field_names() {
    assert_eq!(
        to_string(&EncodedImageAttachment {
            media_type: ImageMediaType::Webp,
            data: "AAAA".into(),
            name: Some("a.webp".into())
        })
        .unwrap(),
        r#"{"mediaType":"image/webp","data":"AAAA","name":"a.webp"}"#
    );
    assert_eq!(
        to_string(&EncodedFileAttachment {
            data: String::new(),
            name: None
        })
        .unwrap(),
        r#"{"data":""}"#
    );
    let limits = ImageAttachmentLimits {
        max_image_bytes: 1,
        max_images_per_message: 2,
        max_message_image_bytes: 3,
        max_image_pixels: 4,
        max_image_dimension: 5,
        media_types: vec![ImageMediaType::Jpeg],
    };
    assert_eq!(
        to_string(&limits).unwrap(),
        r#"{"maxImageBytes":1,"maxImagesPerMessage":2,"maxMessageImageBytes":3,"maxImagePixels":4,"maxImageDimension":5,"mediaTypes":["image/jpeg"]}"#
    );
    assert_eq!(
        to_string(&ImageRequestTarget {
            width: 1,
            height: 2,
            max_bytes: 3
        })
        .unwrap(),
        r#"{"width":1,"height":2,"maxBytes":3}"#
    );
    assert_eq!(to_value(RequestImageDepth::Uchar).unwrap(), "uchar");
    assert_eq!(to_value(RequestImageSpace::Srgb).unwrap(), "srgb");
    assert_eq!(
        to_value(PromptContentPart::Image {
            media_type: ImageMediaType::Png,
            data: "AQ==".into(),
            name: None
        })
        .unwrap(),
        json!({"type":"image","mediaType":"image/png","data":"AQ=="})
    );
    assert_eq!(
        to_value(AdmittedPromptContentPart::Image {
            attachment: reference()
        })
        .unwrap(),
        json!({"type":"image","attachment":reference()})
    );
}

#[test]
fn every_source_code_is_classified_and_shared_errors_keep_their_code() {
    let admission = [
        "TOO_MANY_IMAGES",
        "IMAGES_TOO_LARGE",
        "UNSUPPORTED_IMAGE_TYPE",
        "INVALID_IMAGE_BASE64",
        "INVALID_IMAGE",
        "IMAGE_TYPE_MISMATCH",
        "IMAGE_TOO_LARGE",
        "IMAGE_TOO_MANY_PIXELS",
        "IMAGE_DIMENSION_TOO_LARGE",
    ];
    let storage = [
        "INVALID_FILE_BASE64",
        "INVALID_ATTACHMENT_REF",
        "ATTACHMENT_CORRUPT",
        "ATTACHMENT_WRITE_FAILED",
        "ATTACHMENT_NOT_FOUND",
        "ATTACHMENT_READ_FAILED",
        "ATTACHMENT_PROJECTION_UNSUPPORTED",
        "ATTACHMENT_FILES_UNSUPPORTED",
    ];
    for code in admission.into_iter().chain(storage) {
        let typed = AttachmentErrorCode::from_code(code).unwrap();
        assert_eq!(typed.as_str(), code);
        assert_eq!(to_value(typed).unwrap(), code);
        let error = SharedError::new(HarnessError::named("ForeignError", "foreign failure", code));
        assert!(is_attachment_error(&error));
        assert_eq!(is_image_admission_error(&error), admission.contains(&code));
        if admission.contains(&code) {
            let subset = ImageAdmissionErrorCode::from_code(code).unwrap();
            assert_eq!(subset.as_str(), code);
            assert_eq!(AttachmentErrorCode::from(subset), typed);
        }
    }
    assert!(AttachmentErrorCode::from_code("OTHER").is_none());
    assert!(ImageAdmissionErrorCode::from_code("OTHER").is_none());
    let inner = AttachmentError::new("inner", AttachmentErrorCode::InvalidImage);
    assert!(!is_attachment_error(
        &HarnessError::new("outer", "OTHER").with_cause(inner)
    ));
}

#[test]
fn brands_are_transparent_and_do_not_validate_backend_identifiers() {
    let id: AttachmentIdType = AttachmentId::from("not-a-digest");
    let variant = ImageVariantId::from("request-1");
    assert_eq!(to_value(id).unwrap(), "not-a-digest");
    assert_eq!(to_value(variant).unwrap(), "request-1");
}
