use hoh_office_kit::*;
use hoh_timeout::AbortController;

fn code(error: &hoh_error::BoxError) -> Option<OfficeKitErrorCode> {
    error
        .downcast_ref::<OfficeKitError>()
        .map(|error| error.code)
}

#[tokio::test]
async fn reads_a_complete_pdf_at_the_inclusive_limit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preview.pdf");
    let pdf = b"%PDF-1.7\npreview\n%%EOF\n";
    tokio::fs::write(&path, pdf).await.unwrap();
    let result = read_pdf(&path, pdf.len() as u64, &AbortController::new().signal())
        .await
        .unwrap();
    assert_eq!(result, pdf);
}

#[tokio::test]
async fn rejects_a_non_regular_or_link_shaped_output() {
    let directory = tempfile::tempdir().unwrap();
    let error = read_pdf(directory.path(), 100, &AbortController::new().signal())
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some(OfficeKitErrorCode::InvalidOutput));
    #[cfg(unix)]
    {
        let target = directory.path().join("target.pdf");
        let link = directory.path().join("link.pdf");
        tokio::fs::write(&target, b"%PDF-1.7\n%%EOF").await.unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let error = read_pdf(&link, 100, &AbortController::new().signal())
            .await
            .unwrap_err();
        assert_eq!(code(&error), Some(OfficeKitErrorCode::InvalidOutput));
    }
}

#[tokio::test]
async fn rejects_output_above_the_byte_limit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preview.pdf");
    tokio::fs::write(&path, b"%PDF-1.7\npreview\n%%EOF")
        .await
        .unwrap();
    let error = read_pdf(&path, 3, &AbortController::new().signal())
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some(OfficeKitErrorCode::OutputTooLarge));
}

#[tokio::test]
async fn rejects_incomplete_and_non_pdf_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preview.pdf");
    for bytes in [b"diagnostic".as_slice(), b"%PDF-1.7\ntruncated".as_slice()] {
        tokio::fs::write(&path, bytes).await.unwrap();
        let error = read_pdf(&path, 100, &AbortController::new().signal())
            .await
            .unwrap_err();
        assert_eq!(code(&error), Some(OfficeKitErrorCode::InvalidOutput));
    }
}

#[tokio::test]
async fn reads_only_png_signature_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("image.png");
    let png = b"\x89PNG\r\n\x1a\nbytes";
    tokio::fs::write(&path, png).await.unwrap();
    assert_eq!(
        read_png(&path, 100, &AbortController::new().signal())
            .await
            .unwrap(),
        png
    );
    tokio::fs::write(&path, b"not png").await.unwrap();
    let error = read_png(&path, 100, &AbortController::new().signal())
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some(OfficeKitErrorCode::InvalidOutput));
}

#[tokio::test]
async fn cancellation_is_checked_before_opening() {
    let directory = tempfile::tempdir().unwrap();
    let controller = AbortController::new();
    controller.abort();
    let error = read_output(&directory.path().join("absent"), 10, &controller.signal())
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<hoh_error::SharedError>().is_some());
}
