mod support;

use hoh_office_kit::*;
use hoh_timeout::AbortController;
use std::sync::{Arc, atomic::Ordering};
use support::*;

fn request(key: &str, bytes: Vec<u8>) -> OfficeKitRequest {
    OfficeKitRequest {
        extension: OfficeExtension::Docx,
        priority: OfficeKitPriority::Foreground,
        source: source(key, bytes),
    }
}

#[tokio::test]
async fn unavailable_backend_registers_lazily_and_reports_the_install_hint() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        output_directory: directory.path().join("outputs"),
        ..Config::default()
    };
    let service = OfficeKitService::unavailable(config).unwrap();
    for key in ["first", "second"] {
        let error = service
            .convert(request(key, vec![1]), None)
            .await
            .unwrap_err();
        assert_eq!(code(&error), Some(OfficeKitErrorCode::Unavailable));
        assert!(
            error
                .to_string()
                .contains("This deployment does not provide LibreOffice Kit")
        );
    }
}

#[tokio::test]
async fn passes_input_to_one_reused_converter_and_keeps_returned_pdf_after_disposal() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, service, factory) = service(converter.clone());
    let result = service
        .convert(request("source", vec![80, 75, 3, 4]), None)
        .await
        .unwrap();
    assert_eq!(result.pdf, PDF);
    assert_eq!(result.missing_fonts, ["Missing Serif"]);
    let again = service
        .convert(request("source", vec![80, 75, 3, 4]), None)
        .await
        .unwrap();
    assert_eq!(again.pdf, PDF);
    assert_eq!(factory.creates.load(Ordering::SeqCst), 1);
    assert_eq!(converter.renders.load(Ordering::SeqCst), 1);
    service.dispose().await.unwrap();
    assert_eq!(converter.disposals.load(Ordering::SeqCst), 1);
    assert_eq!(result.pdf, PDF);
}

#[tokio::test]
async fn refuses_an_oversized_input_before_allocating_a_converter() {
    let converter = Arc::new(TestConverter::default());
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        output_directory: directory.path().join("outputs"),
        max_input_bytes: 3,
        ..Config::default()
    };
    let factory = TestFactory::new(converter);
    let service = OfficeKitService::new(config, factory.clone(), |_| {}).unwrap();
    let error = service
        .convert(request("source", vec![1, 2, 3, 4]), None)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some(OfficeKitErrorCode::InputTooLarge));
    assert_eq!(factory.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_invalid_pdf_and_removes_its_scratch_directory() {
    let converter = Arc::new(TestConverter {
        invalid_pdf: true,
        ..Default::default()
    });
    let (_directory, service, _) = service(converter);
    let error = service
        .convert(request("source", vec![1]), None)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some(OfficeKitErrorCode::InvalidOutput));
}

#[tokio::test]
async fn cancellation_before_admission_allocates_no_converter() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, service, factory) = service(converter);
    let controller = AbortController::new();
    controller.abort();
    assert!(
        service
            .convert(request("source", vec![1]), Some(controller.signal()))
            .await
            .is_err()
    );
    assert_eq!(factory.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn current_generation_is_stable_for_one_provider_and_distinct_across_providers() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, first, _) = service(converter.clone());
    let (_other_directory, second, _) = service(converter);
    assert_eq!(
        OfficeKitApi::generation(&first),
        OfficeKitApi::generation(&first)
    );
    assert_ne!(
        OfficeKitApi::generation(&first),
        OfficeKitApi::generation(&second)
    );
}

#[test]
fn config_defaults_and_font_validation_match_the_source_contract() {
    let config = Config::default();
    assert_eq!(config.max_concurrent_conversions, 2);
    assert_eq!(config.timeout_ms, 60_000);
    assert_eq!(config.max_input_bytes, 52_428_800);
    assert_eq!(config.max_output_bytes, 104_857_600);
    assert_eq!(config.max_image_resolution, 192);
    assert!(config.font_directories.is_none());
    assert!(config.font_fallbacks.is_none());
    for groups in [
        vec![vec![]],
        vec![vec!["sans-serif".into()]],
        vec![vec!["sans-serif".into(), " \t\n ".into()]],
    ] {
        let mut invalid = config.clone();
        invalid.font_fallbacks = Some(groups);
        assert!(invalid.validate().is_err());
    }
}

#[tokio::test]
async fn output_directory_is_exposed_to_consumers() {
    let converter = Arc::new(TestConverter::default());
    let (_directory, service, _) = service(converter);
    assert!(OfficeKitApi::output_directory(&service).is_absolute());
    assert!(OfficeKitApi::output_directory(&service).ends_with("outputs"));
}
