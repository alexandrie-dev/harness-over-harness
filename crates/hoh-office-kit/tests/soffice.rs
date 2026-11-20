#![cfg(unix)]

use hoh_office_kit::*;
use hoh_timeout::AbortController;
use std::{os::unix::fs::PermissionsExt, path::Path};

fn options(timeout_ms: u64) -> ConverterOptions {
    let config = Config::default();
    ConverterOptions {
        timeout_ms,
        max_input_bytes: config.max_input_bytes,
        max_output_bytes: config.max_output_bytes,
        max_image_resolution: config.max_image_resolution,
        max_archive_entries: config.max_archive_entries,
        max_uncompressed_bytes: config.max_uncompressed_bytes,
        font_directories: None,
        font_fallbacks: None,
        max_font_files: config.max_font_files,
        max_font_file_bytes: config.max_font_file_bytes,
        max_loaded_font_bytes: config.max_loaded_font_bytes,
    }
}

fn script(dir: &Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("soffice");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn request(dir: &Path, name: &str, sheet: Option<&str>) -> ConvertRequest {
    let input = dir.join(name);
    std::fs::write(&input, b"input").unwrap();
    ConvertRequest {
        input_path: input,
        output_path: dir.join("out/result.xlsx"),
        format: OfficeConversionFormat::Xlsx,
        sheet: sheet.map(str::to_owned),
    }
}

fn code(error: &hoh_error::BoxError) -> OfficeKitErrorCode {
    error.downcast_ref::<OfficeKitError>().unwrap().code
}

async fn converter(dir: &Path, body: &str, timeout_ms: u64) -> std::sync::Arc<dyn Converter> {
    ExternalSofficeFactory::new(script(dir, body))
        .create(options(timeout_ms))
        .await
        .unwrap()
}

#[tokio::test]
async fn a_hung_soffice_fails_with_timeout() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("out")).unwrap();
    let converter = converter(dir.path(), "sleep 30", 200).await;
    let signal = AbortController::new().signal();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        converter.convert(request(dir.path(), "source.xlsx", None), signal),
    )
    .await
    .expect("timeout_ms must bound the child");
    assert_eq!(code(&outcome.unwrap_err()), OfficeKitErrorCode::Timeout);
}

#[tokio::test]
async fn sheet_selection_and_recalculation_are_refused_not_silently_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("out")).unwrap();
    let converter = converter(dir.path(), "exit 0", 5_000).await;
    let signal = AbortController::new().signal();
    let error = converter
        .convert(
            request(dir.path(), "source.xlsx", Some("Data")),
            signal.clone(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&error), OfficeKitErrorCode::InvalidRequest);
    let error = converter
        .recalculate(request(dir.path(), "source.xlsx", None), signal)
        .await
        .unwrap_err();
    assert_eq!(code(&error), OfficeKitErrorCode::Unavailable);
}

#[tokio::test]
async fn same_format_conversion_never_overwrites_the_input() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("out")).unwrap();
    // The fake engine writes "converted" next to the requested --outdir.
    let body = r#"while [ "$1" != "--outdir" ]; do shift; done; out="$2"; in="$3"
b=$(basename "$in" .xlsx); printf converted > "$out/$b.xlsx""#;
    let converter = converter(dir.path(), body, 5_000).await;
    let request = request(dir.path(), "source.xlsx", None);
    let input = request.input_path.clone();
    let output = request.output_path.clone();
    converter
        .convert(request, AbortController::new().signal())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), b"converted");
    assert_eq!(std::fs::read(&input).unwrap(), b"input");
}
