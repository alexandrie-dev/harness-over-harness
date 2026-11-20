//! Bounded reads of regular converter output files.

use crate::{OfficeKitError, OfficeKitErrorCode};
use hoh_error::BoxError;
use hoh_timeout::AbortSignal;
use std::path::Path;
use tokio::io::AsyncReadExt;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

fn boxed(error: OfficeKitError) -> BoxError {
    Box::new(error)
}

fn check_signal(signal: &AbortSignal) -> std::result::Result<(), BoxError> {
    signal
        .throw_if_aborted()
        .map_err(|error| Box::new(error) as BoxError)
}

pub async fn read_output(
    path: &Path,
    limit: u64,
    signal: &AbortSignal,
) -> std::result::Result<Vec<u8>, BoxError> {
    check_signal(signal)?;
    let entry = tokio::fs::symlink_metadata(path).await?;
    if !entry.file_type().is_file() {
        return Err(boxed(OfficeKitError::new(
            OfficeKitErrorCode::InvalidOutput,
            "The converter output is not a regular file.",
        )));
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let mut file = options.open(path).await?;
    let info = file.metadata().await?;
    if !info.is_file() {
        return Err(boxed(OfficeKitError::new(
            OfficeKitErrorCode::InvalidOutput,
            "The converter output is not a regular file.",
        )));
    }
    if info.len() > limit {
        return Err(boxed(OfficeKitError::new(
            OfficeKitErrorCode::OutputTooLarge,
            "The converter output exceeds its byte limit.",
        )));
    }
    let capacity = usize::try_from(info.len().saturating_add(1)).map_err(|_| {
        boxed(OfficeKitError::new(
            OfficeKitErrorCode::OutputTooLarge,
            "The converter output exceeds its byte limit.",
        ))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut chunk = [0_u8; 8192];
    while bytes.len() < capacity {
        check_signal(signal)?;
        let available = (capacity - bytes.len()).min(chunk.len());
        let read = file.read(&mut chunk[..available]).await?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    check_signal(signal)?;
    if bytes.len() as u64 > limit {
        return Err(boxed(OfficeKitError::new(
            OfficeKitErrorCode::OutputTooLarge,
            "The converter output exceeds its byte limit.",
        )));
    }
    if bytes.len() as u64 != info.len() {
        return Err(boxed(OfficeKitError::new(
            OfficeKitErrorCode::InvalidOutput,
            "The converter output changed while it was read.",
        )));
    }
    Ok(bytes)
}

pub async fn read_pdf(
    path: &Path,
    limit: u64,
    signal: &AbortSignal,
) -> std::result::Result<Vec<u8>, BoxError> {
    let pdf = read_output(path, limit, signal).await?;
    let header = pdf.get(..8).unwrap_or(&pdf);
    let valid_header = header.starts_with(b"%PDF-")
        && header.get(5).is_some_and(u8::is_ascii_digit)
        && header.get(6) == Some(&b'.')
        && header.get(7).is_some_and(u8::is_ascii_digit);
    let tail = &pdf[pdf.len().saturating_sub(1024)..];
    let trimmed = tail.trim_ascii_end();
    if !valid_header || !trimmed.ends_with(b"%%EOF") {
        return Err(boxed(OfficeKitError::new(
            OfficeKitErrorCode::InvalidOutput,
            "The converter did not produce a complete PDF.",
        )));
    }
    Ok(pdf)
}

pub async fn read_png(
    path: &Path,
    limit: u64,
    signal: &AbortSignal,
) -> std::result::Result<Vec<u8>, BoxError> {
    let png = read_output(path, limit, signal).await?;
    if !png.starts_with(PNG_SIGNATURE) {
        return Err(boxed(OfficeKitError::new(
            OfficeKitErrorCode::InvalidOutput,
            "The converter did not produce a PNG image.",
        )));
    }
    Ok(png)
}
