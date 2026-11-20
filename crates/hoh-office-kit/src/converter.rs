//! Converter seam replacing the independently published JavaScript LibreOffice Kit.

use crate::{OfficeConversionFormat, OfficeKitError, OfficeKitErrorCode};
use hoh_error::BoxError;
use hoh_timeout::AbortSignal;
use std::{future::Future, path::PathBuf, pin::Pin, process::Stdio, sync::Arc, time::Duration};
use tokio::process::Command;

pub type ConverterFuture<T> =
    Pin<Box<dyn Future<Output = std::result::Result<T, BoxError>> + Send + 'static>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConverterResult {
    pub missing_fonts: Vec<String>,
    pub page_count: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ConvertRequest {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub format: OfficeConversionFormat,
    pub sheet: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RenderImagesRequest {
    pub input_path: PathBuf,
    pub output_dir: PathBuf,
    pub pages: Option<Vec<u32>>,
    pub sheet: Option<String>,
    pub range: Option<String>,
    pub dpi: Option<u32>,
    pub max_pages: u64,
    pub max_pixels: u64,
    pub max_dimension: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedImage {
    pub index: u64,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub byte_length: u64,
    pub page: Option<u32>,
    pub sheet: Option<String>,
    pub range: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderImagesResult {
    pub images: Vec<RenderedImage>,
    pub page_count: u32,
    pub dpi: u32,
    pub missing_fonts: Vec<String>,
}

pub trait Converter: Send + Sync + 'static {
    fn render(
        &self,
        request: RenderRequest,
        signal: AbortSignal,
    ) -> ConverterFuture<ConverterResult>;
    fn render_images(
        &self,
        request: RenderImagesRequest,
        signal: AbortSignal,
    ) -> ConverterFuture<RenderImagesResult>;
    fn convert(
        &self,
        request: ConvertRequest,
        signal: AbortSignal,
    ) -> ConverterFuture<ConverterResult>;
    fn recalculate(
        &self,
        request: ConvertRequest,
        signal: AbortSignal,
    ) -> ConverterFuture<ConverterResult>;
    fn dispose(&self) -> ConverterFuture<()> {
        Box::pin(std::future::ready(Ok(())))
    }
}

pub trait ConverterFactory: Send + Sync + 'static {
    fn create(&self, options: ConverterOptions) -> ConverterFuture<Arc<dyn Converter>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConverterOptions {
    pub timeout_ms: u64,
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
    pub max_image_resolution: u32,
    pub max_archive_entries: u64,
    pub max_uncompressed_bytes: u64,
    pub font_directories: Option<Vec<PathBuf>>,
    pub font_fallbacks: Option<Vec<Vec<String>>>,
    pub max_font_files: u64,
    pub max_font_file_bytes: u64,
    pub max_loaded_font_bytes: u64,
}

const KIT_INSTALL_HINT: &str = "This deployment does not provide LibreOffice Kit; install or enable it as this deployment requires, then retry.";

#[derive(Clone, Debug, Default)]
pub struct UnavailableConverterFactory;

impl ConverterFactory for UnavailableConverterFactory {
    fn create(&self, _options: ConverterOptions) -> ConverterFuture<Arc<dyn Converter>> {
        Box::pin(async {
            Err(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::Unavailable,
                format!("LibreOffice Kit is not installed. {KIT_INSTALL_HINT}"),
            )) as BoxError)
        })
    }
}

/// Opt-in backend which invokes a caller-provided `soffice` executable.
#[derive(Clone, Debug)]
pub struct ExternalSofficeFactory {
    executable: PathBuf,
}

impl ExternalSofficeFactory {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }
}

impl ConverterFactory for ExternalSofficeFactory {
    fn create(&self, options: ConverterOptions) -> ConverterFuture<Arc<dyn Converter>> {
        let executable = self.executable.clone();
        let timeout = Duration::from_millis(options.timeout_ms);
        Box::pin(async move {
            Ok(Arc::new(ExternalSoffice {
                executable,
                timeout,
            }) as Arc<dyn Converter>)
        })
    }
}

#[derive(Debug)]
struct ExternalSoffice {
    executable: PathBuf,
    timeout: Duration,
}

impl ExternalSoffice {
    async fn run(
        executable: PathBuf,
        timeout: Duration,
        request: ConvertRequest,
        signal: AbortSignal,
    ) -> std::result::Result<ConverterResult, BoxError> {
        if request.sheet.is_some() {
            return Err(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::InvalidRequest,
                "The external soffice backend cannot select a sheet.",
            )));
        }
        signal
            .throw_if_aborted()
            .map_err(|error| Box::new(error) as BoxError)?;
        let output_dir = request.output_path.parent().ok_or_else(|| {
            Box::new(OfficeKitError::new(
                OfficeKitErrorCode::InvalidRequest,
                "The output path has no parent directory.",
            )) as BoxError
        })?;
        // Convert in a private directory so the result never overwrites the input
        // (or any file next to the output) when both share a name.
        let scratch = tempfile::Builder::new()
            .prefix("soffice-")
            .tempdir_in(output_dir)?;
        let mut command = Command::new(executable);
        command
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .arg("--headless")
            .arg("--convert-to")
            .arg(request.format.to_string())
            .arg("--outdir")
            .arg(scratch.path())
            .arg(&request.input_path);
        let mut child = command.spawn().map_err(|error| {
            Box::new(OfficeKitError::with_source(
                OfficeKitErrorCode::Unavailable,
                "LibreOffice Kit could not be loaded.",
                error,
            )) as BoxError
        })?;
        let status = tokio::select! {
            result = tokio::time::timeout(timeout, child.wait()) => match result {
                Ok(result) => result?,
                Err(_) => {
                    let _ = child.kill().await;
                    return Err(Box::new(OfficeKitError::new(
                        OfficeKitErrorCode::Timeout,
                        "LibreOffice conversion timed out.",
                    )));
                }
            },
            reason = signal.cancelled() => {
                return Err(Box::new(reason.clone()) as BoxError);
            }
        };
        if !status.success() {
            return Err(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::Failed,
                "LibreOffice conversion failed.",
            )));
        }
        let stem = request
            .input_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("source");
        let produced = scratch.path().join(format!("{stem}.{}", request.format));
        tokio::fs::rename(&produced, &request.output_path).await?;
        Ok(ConverterResult::default())
    }
}

impl Converter for ExternalSoffice {
    fn render(
        &self,
        request: RenderRequest,
        signal: AbortSignal,
    ) -> ConverterFuture<ConverterResult> {
        let request = ConvertRequest {
            input_path: request.input_path,
            output_path: request.output_path,
            format: OfficeConversionFormat::Pdf,
            sheet: None,
        };
        let executable = self.executable.clone();
        Box::pin(Self::run(executable, self.timeout, request, signal))
    }

    fn render_images(
        &self,
        _request: RenderImagesRequest,
        _signal: AbortSignal,
    ) -> ConverterFuture<RenderImagesResult> {
        Box::pin(async {
            Err(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::Unavailable,
                "The external soffice backend does not provide image rendering.",
            )) as BoxError)
        })
    }

    fn convert(
        &self,
        request: ConvertRequest,
        signal: AbortSignal,
    ) -> ConverterFuture<ConverterResult> {
        let executable = self.executable.clone();
        Box::pin(Self::run(executable, self.timeout, request, signal))
    }

    fn recalculate(
        &self,
        _request: ConvertRequest,
        _signal: AbortSignal,
    ) -> ConverterFuture<ConverterResult> {
        // A plain `--convert-to` does not guarantee formula recalculation.
        Box::pin(async {
            Err(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::Unavailable,
                "The external soffice backend does not provide recalculation.",
            )) as BoxError)
        })
    }
}
