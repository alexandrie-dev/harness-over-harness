use crate::{OfficeKitGeneration, OfficeKitKey, OfficeSourceKey};
use hoh_error::BoxError;
use hoh_timeout::AbortSignal;
use std::{future::Future, pin::Pin, sync::Arc};

pub type SourceReadFuture =
    Pin<Box<dyn Future<Output = std::result::Result<OfficeSourceRead, BoxError>> + Send + 'static>>;
pub type SourceReader = dyn Fn(AbortSignal, u64) -> SourceReadFuture + Send + Sync + 'static;

#[derive(Clone)]
pub struct OfficeKitSource {
    pub key: OfficeSourceKey,
    pub version: String,
    pub bytes: Option<u64>,
    pub read: Arc<SourceReader>,
}

impl std::fmt::Debug for OfficeKitSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OfficeKitSource")
            .field("key", &self.key)
            .field("version", &self.version)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfficeSourceRead {
    pub bytes: Vec<u8>,
    pub version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeKitPriority {
    Foreground,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeExtension {
    Doc,
    Docx,
    Xls,
    Xlsx,
    Ppt,
    Pptx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeDocumentExtension {
    Doc,
    Docx,
    Xls,
    Xlsx,
    Ppt,
    Pptx,
    Odt,
    Ods,
    Odp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeImageInputExtension {
    Doc,
    Docx,
    Xls,
    Xlsx,
    Ppt,
    Pptx,
    Odt,
    Ods,
    Odp,
    Pdf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeWorkbookExtension {
    Xls,
    Xlsx,
    Ods,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeConversionFormat {
    Pdf,
    Docx,
    Odt,
    Txt,
    Xlsx,
    Ods,
    Csv,
    Pptx,
    Odp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeRecalculationFormat {
    Xlsx,
    Ods,
}

macro_rules! display_lowercase {
    ($($ty:ty),+ $(,)?) => {$(
        impl std::fmt::Display for $ty {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let value = serde_json::to_value(self).map_err(|_| std::fmt::Error)?;
                formatter.write_str(value.as_str().ok_or(std::fmt::Error)?)
            }
        }
    )+};
}
display_lowercase!(
    OfficeExtension,
    OfficeDocumentExtension,
    OfficeImageInputExtension,
    OfficeWorkbookExtension,
    OfficeConversionFormat,
    OfficeRecalculationFormat
);

#[derive(Clone, Debug)]
pub struct OfficeKitRequest {
    pub extension: OfficeExtension,
    pub priority: OfficeKitPriority,
    pub source: OfficeKitSource,
}

#[derive(Clone, Debug)]
pub struct OfficeImagesRequest {
    pub extension: OfficeImageInputExtension,
    pub source: OfficeKitSource,
    pub pages: Option<Vec<u32>>,
    pub sheet: Option<String>,
    pub range: Option<String>,
    pub dpi: Option<u32>,
    pub max_images: u64,
    pub max_pixels: u64,
    pub max_dimension: u64,
    pub max_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfficeImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub page: Option<u32>,
    pub sheet: Option<String>,
    pub range: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfficeImagesResult {
    pub images: Vec<OfficeImage>,
    pub page_count: u32,
    pub dpi: u32,
    pub missing_fonts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfficeOutputTarget {
    pub owner: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct OfficeConversionRequest {
    pub extension: OfficeDocumentExtension,
    pub format: OfficeConversionFormat,
    pub sheet: Option<String>,
    pub source: OfficeKitSource,
    pub output: OfficeOutputTarget,
}

#[derive(Clone, Debug)]
pub struct OfficeRecalculationRequest {
    pub extension: OfficeWorkbookExtension,
    pub format: OfficeRecalculationFormat,
    pub source: OfficeKitSource,
    pub output: OfficeOutputTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfficeFileResult {
    pub path: std::path::PathBuf,
    pub bytes: u64,
    pub page_count: Option<u32>,
    pub missing_fonts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfficeKitResult {
    pub pdf: Vec<u8>,
    pub missing_fonts: Vec<String>,
    pub cache_key: OfficeKitKey,
    pub generation: OfficeKitGeneration,
}
