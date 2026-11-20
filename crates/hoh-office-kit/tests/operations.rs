mod support;

use hoh_office_kit::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::*;

fn images(source_key: &str) -> OfficeImagesRequest {
    OfficeImagesRequest {
        extension: OfficeImageInputExtension::Pptx,
        source: source(source_key, vec![80, 75, 3, 4]),
        pages: Some(vec![3, 1]),
        sheet: None,
        range: None,
        dpi: Some(96),
        max_images: 4,
        max_pixels: 1_000_000,
        max_dimension: 4096,
        max_bytes: 1024,
    }
}

#[tokio::test]
async fn renders_images_in_document_order_with_locations_and_limits() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, service, _) = service(converter.clone());
    let result = service.render_images(images("source"), None).await.unwrap();
    assert_eq!(result.page_count, 3);
    assert_eq!(result.dpi, 96);
    assert_eq!(result.missing_fonts, ["Missing Sans"]);
    assert_eq!(result.images[0].sheet.as_deref(), Some("Data"));
    assert_eq!(result.images[0].range.as_deref(), Some("A1:B2"));
    assert_eq!(result.images[1].page, Some(3));
    assert_eq!(result.images[0].png, PNG);
    assert_eq!(converter.images.load(Ordering::SeqCst), 1);
    service.dispose().await.unwrap();
}

#[tokio::test]
async fn returns_independent_cached_image_copies_and_separates_selections() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, service, _) = service(converter.clone());
    let mut request = images("source");
    let mut first = service.render_images(request.clone(), None).await.unwrap();
    first.images[0].png[0] = 0;
    let second = service.render_images(request.clone(), None).await.unwrap();
    assert_eq!(second.images[0].png, PNG);
    assert_eq!(converter.images.load(Ordering::SeqCst), 1);
    request.pages = Some(vec![2]);
    service.render_images(request, None).await.unwrap();
    assert_eq!(converter.images.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rejects_declared_image_bytes_above_the_limit() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, service, _) = service(converter);
    let mut request = images("source");
    request.max_bytes = PNG.len() as u64;
    let error = service.render_images(request, None).await.unwrap_err();
    assert_eq!(code(&error), Some(OfficeKitErrorCode::OutputTooLarge));
    assert!(error.to_string().contains("above the 10-byte limit"));
}

#[tokio::test]
async fn rejects_an_image_outside_its_output_directory() {
    let converter = Arc::new(TestConverter {
        outside_image: true,
        ..Default::default()
    });
    let (_directory, service, _) = service(converter);
    let error = service
        .render_images(images("source"), None)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some(OfficeKitErrorCode::InvalidOutput));
}

#[tokio::test]
async fn converts_to_pdf_and_csv_and_returns_new_saved_paths_from_one_cached_result() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, service, _) = service(converter.clone());
    for (format, expected) in [
        (OfficeConversionFormat::Pdf, PDF),
        (OfficeConversionFormat::Csv, XLSX),
    ] {
        let request = OfficeConversionRequest {
            extension: OfficeDocumentExtension::Xlsx,
            format,
            sheet: (format == OfficeConversionFormat::Csv).then(|| "Data".into()),
            source: source(&format.to_string(), vec![1]),
            output: OfficeOutputTarget {
                owner: "session".into(),
                name: format!("book.{format}"),
            },
        };
        let first = service.convert_to(request.clone(), None).await.unwrap();
        let second = service.convert_to(request, None).await.unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(tokio::fs::read(first.path).await.unwrap(), expected);
        assert_eq!(tokio::fs::read(second.path).await.unwrap(), expected);
    }
    assert_eq!(converter.conversions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn recalculates_a_workbook_and_preserves_page_count_and_fonts() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, service, _) = service(converter.clone());
    let result = service
        .recalculate(
            OfficeRecalculationRequest {
                extension: OfficeWorkbookExtension::Xls,
                format: OfficeRecalculationFormat::Xlsx,
                source: source("book", vec![1]),
                output: OfficeOutputTarget {
                    owner: "session".into(),
                    name: "book.xlsx".into(),
                },
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.bytes, XLSX.len() as u64);
    assert_eq!(result.page_count, Some(2));
    assert_eq!(result.missing_fonts, ["Calc Font"]);
    assert_eq!(tokio::fs::read(result.path).await.unwrap(), XLSX);
    assert_eq!(converter.recalculations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn release_deletes_one_owner_and_dispose_deletes_the_rest() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, service, _) = service(converter);
    let request = |owner: &str| OfficeConversionRequest {
        extension: OfficeDocumentExtension::Docx,
        format: OfficeConversionFormat::Pdf,
        sheet: None,
        source: source(owner, vec![1]),
        output: OfficeOutputTarget {
            owner: owner.into(),
            name: "doc.pdf".into(),
        },
    };
    let first = service.convert_to(request("first"), None).await.unwrap();
    let second = service.convert_to(request("second"), None).await.unwrap();
    service.release("first").await;
    assert!(tokio::fs::metadata(first.path).await.is_err());
    assert!(tokio::fs::metadata(&second.path).await.is_ok());
    service.dispose().await.unwrap();
    assert!(tokio::fs::metadata(second.path).await.is_err());
}

#[tokio::test]
async fn rejects_relative_output_and_font_directories_and_too_small_source_capacity() {
    for configure in [
        |config: &mut Config| config.output_directory = "relative".into(),
        |config: &mut Config| config.font_directories = Some(vec!["relative/fonts".into()]),
        |config: &mut Config| {
            config.max_input_bytes = 4;
            config.max_source_bytes = 3;
        },
    ] {
        let mut config = Config::default();
        configure(&mut config);
        assert!(
            OfficeKitService::new(
                config,
                TestFactory::new(Arc::new(TestConverter::default())),
                |_| {}
            )
            .is_err()
        );
    }
}

#[test]
fn all_operation_test_helpers_remain_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OfficeKitService>();
    let _ = AtomicUsize::new(0);
}
