//! Shared Office rendering, conversion, recalculation, admission, and output retention.

mod converter;
mod errors;
mod identity;
pub mod locate;
mod output;
mod outputs;
mod queue;
mod types;

pub use converter::*;
pub use errors::*;
pub use identity::*;
pub use output::*;
pub use outputs::*;
pub use queue::*;
pub use types::*;

use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Plugin, Undo};
use hoh_timeout::AbortSignal;
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub max_concurrent_conversions: u64,
    pub max_queued_jobs: u64,
    pub max_readers: u64,
    pub max_source_bytes: u64,
    pub max_background_conversions: u64,
    pub max_cached_entries: u64,
    pub max_cached_bytes: u64,
    pub max_source_entries: u64,
    pub output_directory: PathBuf,
    pub output_retention_ms: u64,
    pub max_output_store_bytes: u64,
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

impl Default for Config {
    fn default() -> Self {
        let output_directory = hoh_home_paths::default_dsh_home()
            .unwrap_or_else(|_| PathBuf::from(".dsh"))
            .join("office-kit")
            .join("outputs");
        Self {
            max_concurrent_conversions: 2,
            max_queued_jobs: 8,
            max_readers: 32,
            max_source_bytes: 104_857_600,
            max_background_conversions: 1,
            max_cached_entries: 8,
            max_cached_bytes: 134_217_728,
            max_source_entries: 64,
            output_directory,
            output_retention_ms: 86_400_000,
            max_output_store_bytes: 536_870_912,
            timeout_ms: 60_000,
            max_input_bytes: 52_428_800,
            max_output_bytes: 104_857_600,
            max_image_resolution: 192,
            max_archive_entries: 10_000,
            max_uncompressed_bytes: 262_144_000,
            font_directories: None,
            font_fallbacks: None,
            max_font_files: 20_000,
            max_font_file_bytes: 268_435_456,
            max_loaded_font_bytes: 536_870_912,
        }
    }
}

impl Config {
    pub fn validate(&self) -> std::result::Result<(), ConfigError> {
        for (name, value) in [
            ("maxConcurrentConversions", self.max_concurrent_conversions),
            ("maxQueuedJobs", self.max_queued_jobs),
            ("maxReaders", self.max_readers),
            ("maxSourceBytes", self.max_source_bytes),
            ("maxCachedEntries", self.max_cached_entries),
            ("maxCachedBytes", self.max_cached_bytes),
            ("maxSourceEntries", self.max_source_entries),
            ("maxOutputStoreBytes", self.max_output_store_bytes),
            ("maxInputBytes", self.max_input_bytes),
            ("maxOutputBytes", self.max_output_bytes),
            ("maxArchiveEntries", self.max_archive_entries),
            ("maxUncompressedBytes", self.max_uncompressed_bytes),
            ("maxFontFiles", self.max_font_files),
            ("maxFontFileBytes", self.max_font_file_bytes),
            ("maxLoadedFontBytes", self.max_loaded_font_bytes),
        ] {
            if value == 0 || value > MAX_SAFE_INTEGER {
                return Err(ConfigError(format!(
                    "{name} must be a positive safe integer"
                )));
            }
        }
        if self.max_background_conversions > MAX_SAFE_INTEGER {
            return Err(ConfigError(
                "maxBackgroundConversions must be a natural safe integer".into(),
            ));
        }
        if self.timeout_ms == 0 || self.timeout_ms > 2_147_483_647 {
            return Err(ConfigError(
                "timeoutMs must be between 1 and 2147483647".into(),
            ));
        }
        if self.output_retention_ms == 0 || self.output_retention_ms > 2_147_483_647 {
            return Err(ConfigError(
                "outputRetentionMs must be between 1 and 2147483647".into(),
            ));
        }
        if self.max_image_resolution == 0 || self.max_image_resolution > 2_147_483_647 {
            return Err(ConfigError(
                "maxImageResolution must be between 1 and 2147483647".into(),
            ));
        }
        if self.max_input_bytes >= MAX_SAFE_INTEGER || self.max_output_bytes >= MAX_SAFE_INTEGER {
            return Err(ConfigError(
                "input and output byte limits must be below Number.MAX_SAFE_INTEGER".into(),
            ));
        }
        if self
            .font_directories
            .as_ref()
            .is_some_and(|paths| paths.iter().any(|path| path.as_os_str().is_empty()))
        {
            return Err(ConfigError(
                "fontDirectories entries must not be empty".into(),
            ));
        }
        if self.font_fallbacks.as_ref().is_some_and(|groups| {
            groups.iter().any(|group| {
                group.len() < 2
                    || group.iter().any(|name| {
                        !name
                            .chars()
                            .any(|ch| !hoh_util_values::is_js_white_space(ch))
                    })
            })
        }) {
            return Err(ConfigError(
                "fontFallbacks groups require at least two non-whitespace names".into(),
            ));
        }
        Ok(())
    }

    fn converter_options(&self) -> ConverterOptions {
        ConverterOptions {
            timeout_ms: self.timeout_ms,
            max_input_bytes: self.max_input_bytes,
            max_output_bytes: self.max_output_bytes,
            max_image_resolution: self.max_image_resolution,
            max_archive_entries: self.max_archive_entries,
            max_uncompressed_bytes: self.max_uncompressed_bytes,
            font_directories: self.font_directories.clone(),
            font_fallbacks: self.font_fallbacks.clone(),
            max_font_files: self.max_font_files,
            max_font_file_bytes: self.max_font_file_bytes,
            max_loaded_font_bytes: self.max_loaded_font_bytes,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);

pub type OfficeFuture<T> =
    Pin<Box<dyn Future<Output = std::result::Result<T, SharedError>> + Send + 'static>>;

pub trait OfficeKitApi: Send + Sync + 'static {
    fn generation(&self) -> OfficeKitGeneration;
    fn output_directory(&self) -> &Path;
    fn convert(
        &self,
        request: OfficeKitRequest,
        signal: Option<AbortSignal>,
    ) -> OfficeFuture<OfficeKitResult>;
    fn render_images(
        &self,
        request: OfficeImagesRequest,
        signal: Option<AbortSignal>,
    ) -> OfficeFuture<OfficeImagesResult>;
    fn convert_to(
        &self,
        request: OfficeConversionRequest,
        signal: Option<AbortSignal>,
    ) -> OfficeFuture<OfficeFileResult>;
    fn recalculate(
        &self,
        request: OfficeRecalculationRequest,
        signal: Option<AbortSignal>,
    ) -> OfficeFuture<OfficeFileResult>;
    fn release(&self, owner: String) -> OfficeFuture<()>;
}

hoh_kernel::service!(pub OfficeKit: dyn OfficeKitApi = "officeKit");

struct Slot {
    busy: AtomicBool,
    converter: tokio::sync::Mutex<Option<Arc<dyn Converter>>>,
}

struct ProviderInner {
    config: Config,
    options: ConverterOptions,
    factory: Arc<dyn ConverterFactory>,
    slots: Mutex<Vec<Arc<Slot>>>,
    disposed: AtomicBool,
}

#[derive(Clone)]
pub struct OfficeKitService {
    inner: Arc<ProviderInner>,
    queue: ConversionQueue,
    outputs: OutputStore,
    generation: OfficeKitGeneration,
    output_directory: PathBuf,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn as_shared(error: impl Into<BoxError>) -> SharedError {
    SharedError::new(error)
}

impl OfficeKitService {
    pub fn new(
        config: Config,
        factory: Arc<dyn ConverterFactory>,
        warn: impl Fn(String) + Send + Sync + 'static,
    ) -> std::result::Result<Self, ConfigError> {
        config.validate()?;
        if config
            .font_directories
            .as_ref()
            .is_some_and(|paths| paths.iter().any(|path| !path.is_absolute()))
        {
            return Err(ConfigError(
                "fontDirectories must contain absolute paths.".into(),
            ));
        }
        if config.max_source_bytes < config.max_input_bytes {
            return Err(ConfigError(
                "maxSourceBytes must be at least maxInputBytes.".into(),
            ));
        }
        if !config.output_directory.is_absolute() {
            return Err(ConfigError(
                "outputDirectory must be an absolute path.".into(),
            ));
        }
        let generation = OfficeKitGeneration::from(random_uuid()?);
        let output_directory = config.output_directory.clone();
        let outputs = OutputStore::new(
            OutputStoreOptions {
                root: output_directory.clone(),
                retention: Duration::from_millis(config.output_retention_ms),
                max_bytes: config.max_output_store_bytes,
            },
            warn,
        );
        let inner = Arc::new(ProviderInner {
            options: config.converter_options(),
            config: config.clone(),
            factory,
            slots: Mutex::new(Vec::new()),
            disposed: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&inner);
        let queue =
            ConversionQueue::new(config, generation.clone(), move |bytes, request, signal| {
                let weak = weak.clone();
                Box::pin(async move {
                    let inner = weak.upgrade().ok_or_else(|| {
                        Box::new(OfficeKitError::new(
                            OfficeKitErrorCode::Unavailable,
                            "The document converter is unavailable.",
                        )) as BoxError
                    })?;
                    inner.run_operation(bytes, request, signal).await
                })
            });
        Ok(Self {
            inner,
            queue,
            outputs,
            generation,
            output_directory,
        })
    }

    pub fn unavailable(config: Config) -> std::result::Result<Self, ConfigError> {
        Self::new(config, Arc::new(UnavailableConverterFactory), |_| {})
    }

    pub async fn convert(
        &self,
        request: OfficeKitRequest,
        signal: Option<AbortSignal>,
    ) -> std::result::Result<OfficeKitResult, SharedError> {
        let result = self
            .queue
            .read(
                QueueRequest {
                    extension: request.extension.to_string(),
                    priority: request.priority,
                    source: request.source,
                    operation: OfficeOperation::Pdf,
                },
                signal,
            )
            .await?;
        match result.output {
            OfficeOperationOutput::Pdf { pdf, missing_fonts } => Ok(OfficeKitResult {
                pdf,
                missing_fonts,
                cache_key: result.cache_key,
                generation: result.generation,
            }),
            _ => Err(as_shared(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::InvalidOutput,
                "The converter returned the wrong operation kind.",
            )) as BoxError)),
        }
    }

    pub async fn render_images(
        &self,
        request: OfficeImagesRequest,
        signal: Option<AbortSignal>,
    ) -> std::result::Result<OfficeImagesResult, SharedError> {
        let operation = OfficeOperation::Images {
            pages: request.pages,
            sheet: request.sheet,
            range: request.range,
            dpi: request.dpi,
            max_images: request.max_images,
            max_pixels: request.max_pixels,
            max_dimension: request.max_dimension,
            max_bytes: request.max_bytes,
        };
        let result = self
            .queue
            .read(
                QueueRequest {
                    extension: request.extension.to_string(),
                    priority: OfficeKitPriority::Foreground,
                    source: request.source,
                    operation,
                },
                signal,
            )
            .await?;
        match result.output {
            OfficeOperationOutput::Images {
                images,
                page_count,
                dpi,
                missing_fonts,
            } => Ok(OfficeImagesResult {
                images,
                page_count,
                dpi,
                missing_fonts,
            }),
            _ => Err(as_shared(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::InvalidOutput,
                "The converter returned the wrong operation kind.",
            )) as BoxError)),
        }
    }

    pub async fn convert_to(
        &self,
        request: OfficeConversionRequest,
        signal: Option<AbortSignal>,
    ) -> std::result::Result<OfficeFileResult, SharedError> {
        let result = self
            .queue
            .read(
                QueueRequest {
                    extension: request.extension.to_string(),
                    priority: OfficeKitPriority::Foreground,
                    source: request.source,
                    operation: OfficeOperation::Convert {
                        format: request.format,
                        sheet: request.sheet,
                    },
                },
                signal.clone(),
            )
            .await?;
        self.save_file(request.output, result.output, signal).await
    }

    pub async fn recalculate(
        &self,
        request: OfficeRecalculationRequest,
        signal: Option<AbortSignal>,
    ) -> std::result::Result<OfficeFileResult, SharedError> {
        let result = self
            .queue
            .read(
                QueueRequest {
                    extension: request.extension.to_string(),
                    priority: OfficeKitPriority::Foreground,
                    source: request.source,
                    operation: OfficeOperation::Recalculate {
                        format: request.format,
                    },
                },
                signal.clone(),
            )
            .await?;
        self.save_file(request.output, result.output, signal).await
    }

    async fn save_file(
        &self,
        target: OfficeOutputTarget,
        output: OfficeOperationOutput,
        signal: Option<AbortSignal>,
    ) -> std::result::Result<OfficeFileResult, SharedError> {
        if let Some(signal) = &signal {
            signal.throw_if_aborted()?;
        }
        let OfficeOperationOutput::File {
            data,
            page_count,
            missing_fonts,
        } = output
        else {
            return Err(as_shared(Box::new(OfficeKitError::new(
                OfficeKitErrorCode::InvalidOutput,
                "The converter returned the wrong operation kind.",
            )) as BoxError));
        };
        let path = self
            .outputs
            .save(&target.owner, &target.name, &data)
            .await
            .map_err(SharedError::new)?;
        Ok(OfficeFileResult {
            path,
            bytes: data.len() as u64,
            page_count,
            missing_fonts,
        })
    }

    pub async fn release(&self, owner: &str) {
        self.outputs.release(owner).await;
    }

    pub async fn dispose(&self) -> std::result::Result<(), SharedError> {
        if self.inner.disposed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.queue.dispose().await;
        self.outputs.dispose().await;
        let slots = lock(&self.inner.slots).clone();
        let mut first = None;
        for slot in slots {
            if let Some(converter) = slot.converter.lock().await.as_ref()
                && let Err(error) = converter.dispose().await
                && first.is_none()
            {
                first = Some(SharedError::new(error));
            }
        }
        first.map_or(Ok(()), Err)
    }
}

impl OfficeKitApi for OfficeKitService {
    fn generation(&self) -> OfficeKitGeneration {
        self.generation.clone()
    }

    fn output_directory(&self) -> &Path {
        &self.output_directory
    }

    fn convert(
        &self,
        request: OfficeKitRequest,
        signal: Option<AbortSignal>,
    ) -> OfficeFuture<OfficeKitResult> {
        let service = self.clone();
        Box::pin(async move { service.convert(request, signal).await })
    }

    fn render_images(
        &self,
        request: OfficeImagesRequest,
        signal: Option<AbortSignal>,
    ) -> OfficeFuture<OfficeImagesResult> {
        let service = self.clone();
        Box::pin(async move { service.render_images(request, signal).await })
    }

    fn convert_to(
        &self,
        request: OfficeConversionRequest,
        signal: Option<AbortSignal>,
    ) -> OfficeFuture<OfficeFileResult> {
        let service = self.clone();
        Box::pin(async move { service.convert_to(request, signal).await })
    }

    fn recalculate(
        &self,
        request: OfficeRecalculationRequest,
        signal: Option<AbortSignal>,
    ) -> OfficeFuture<OfficeFileResult> {
        let service = self.clone();
        Box::pin(async move { service.recalculate(request, signal).await })
    }

    fn release(&self, owner: String) -> OfficeFuture<()> {
        let service = self.clone();
        Box::pin(async move {
            service.release(&owner).await;
            Ok(())
        })
    }
}

impl ProviderInner {
    fn acquire_slot(&self) -> Arc<Slot> {
        let mut slots = lock(&self.slots);
        if let Some(slot) = slots
            .iter()
            .find(|slot| {
                slot.busy
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            })
            .cloned()
        {
            return slot;
        }
        let slot = Arc::new(Slot {
            busy: AtomicBool::new(true),
            converter: tokio::sync::Mutex::new(None),
        });
        slots.push(slot.clone());
        slot
    }

    async fn run_operation(
        self: Arc<Self>,
        bytes: Vec<u8>,
        request: QueueRequest,
        signal: AbortSignal,
    ) -> std::result::Result<OfficeOperationOutput, BoxError> {
        signal
            .throw_if_aborted()
            .map_err(|error| Box::new(error) as BoxError)?;
        let slot = self.acquire_slot();
        let outcome = self
            .run_in_slot(slot.clone(), bytes, request, signal.clone())
            .await;
        slot.busy.store(false, Ordering::SeqCst);
        match outcome {
            Ok(output) => Ok(output),
            Err(error) => {
                signal
                    .throw_if_aborted()
                    .map_err(|error| Box::new(error) as BoxError)?;
                if error.downcast_ref::<OfficeKitError>().is_some() {
                    return Err(error);
                }
                Err(Box::new(OfficeKitError::with_source(
                    OfficeKitErrorCode::Failed,
                    "LibreOffice conversion failed.",
                    error,
                )))
            }
        }
    }

    async fn run_in_slot(
        &self,
        slot: Arc<Slot>,
        bytes: Vec<u8>,
        request: QueueRequest,
        signal: AbortSignal,
    ) -> std::result::Result<OfficeOperationOutput, BoxError> {
        let converter = {
            let mut converter = slot.converter.lock().await;
            if converter.is_none() {
                match self.factory.create(self.options.clone()).await {
                    Ok(created) => *converter = Some(created),
                    Err(error) => return Err(error),
                }
            }
            converter.as_ref().expect("converter initialized").clone()
        };
        signal
            .throw_if_aborted()
            .map_err(|error| Box::new(error) as BoxError)?;
        let directory = tempfile::Builder::new()
            .prefix("dsh-office-kit-")
            .tempdir()
            .map_err(|error| Box::new(error) as BoxError)?;
        let input_path = directory
            .path()
            .join(format!("source.{}", request.extension));
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let mut file = options.open(&input_path).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        signal
            .throw_if_aborted()
            .map_err(|error| Box::new(error) as BoxError)?;
        self.run_engine(
            converter.as_ref(),
            input_path,
            directory.path(),
            request.operation,
            signal,
        )
        .await
    }

    async fn run_engine(
        &self,
        converter: &dyn Converter,
        input_path: PathBuf,
        directory: &Path,
        operation: OfficeOperation,
        signal: AbortSignal,
    ) -> std::result::Result<OfficeOperationOutput, BoxError> {
        match operation {
            OfficeOperation::Pdf => {
                let output_path = directory.join("converted.pdf");
                let result = converter
                    .render(
                        RenderRequest {
                            input_path,
                            output_path: output_path.clone(),
                        },
                        signal.clone(),
                    )
                    .await?;
                Ok(OfficeOperationOutput::Pdf {
                    pdf: read_engine_file(
                        &output_path,
                        "pdf",
                        self.config.max_output_bytes,
                        &signal,
                    )
                    .await?,
                    missing_fonts: result.missing_fonts,
                })
            }
            OfficeOperation::Convert { format, sheet } => {
                let output_path = directory.join(format!("output.{format}"));
                let result = converter
                    .convert(
                        ConvertRequest {
                            input_path,
                            output_path: output_path.clone(),
                            format,
                            sheet,
                        },
                        signal.clone(),
                    )
                    .await?;
                Ok(OfficeOperationOutput::File {
                    data: read_engine_file(
                        &output_path,
                        &format.to_string(),
                        self.config.max_output_bytes,
                        &signal,
                    )
                    .await?,
                    page_count: result.page_count,
                    missing_fonts: result.missing_fonts,
                })
            }
            OfficeOperation::Recalculate { format } => {
                let conversion_format = match format {
                    OfficeRecalculationFormat::Xlsx => OfficeConversionFormat::Xlsx,
                    OfficeRecalculationFormat::Ods => OfficeConversionFormat::Ods,
                };
                let output_path = directory.join(format!("output.{format}"));
                let result = converter
                    .recalculate(
                        ConvertRequest {
                            input_path,
                            output_path: output_path.clone(),
                            format: conversion_format,
                            sheet: None,
                        },
                        signal.clone(),
                    )
                    .await?;
                Ok(OfficeOperationOutput::File {
                    data: read_engine_file(
                        &output_path,
                        &format.to_string(),
                        self.config.max_output_bytes,
                        &signal,
                    )
                    .await?,
                    page_count: result.page_count,
                    missing_fonts: result.missing_fonts,
                })
            }
            OfficeOperation::Images {
                pages,
                sheet,
                range,
                dpi,
                max_images,
                max_pixels,
                max_dimension,
                max_bytes,
            } => {
                let output_dir = directory.join("images");
                let result = converter
                    .render_images(
                        RenderImagesRequest {
                            input_path,
                            output_dir: output_dir.clone(),
                            pages,
                            sheet,
                            range,
                            dpi,
                            max_pages: max_images,
                            max_pixels,
                            max_dimension,
                        },
                        signal.clone(),
                    )
                    .await?;
                let declared: u64 = result.images.iter().map(|image| image.byte_length).sum();
                if declared > max_bytes {
                    return Err(Box::new(OfficeKitError::new(
                        OfficeKitErrorCode::OutputTooLarge,
                        format!(
                            "The rendered images total {declared} bytes, above the {max_bytes}-byte limit."
                        ),
                    )));
                }
                let mut entries = result.images;
                entries.sort_by_key(|image| image.index);
                let mut remaining = max_bytes;
                let mut images = Vec::with_capacity(entries.len());
                for image in entries {
                    let within = image.path.strip_prefix(&output_dir).ok();
                    if within.is_none_or(|path| path.as_os_str().is_empty()) {
                        return Err(Box::new(OfficeKitError::new(
                            OfficeKitErrorCode::InvalidOutput,
                            "The converter reported an image outside its output directory.",
                        )));
                    }
                    let png = read_image_file(&image.path, remaining, &signal).await?;
                    remaining -= png.len() as u64;
                    images.push(OfficeImage {
                        png,
                        width: image.width,
                        height: image.height,
                        page: image.page,
                        sheet: image.sheet,
                        range: image.range,
                    });
                }
                Ok(OfficeOperationOutput::Images {
                    images,
                    page_count: result.page_count,
                    dpi: result.dpi,
                    missing_fonts: result.missing_fonts,
                })
            }
        }
    }
}

async fn read_engine_file(
    path: &Path,
    format: &str,
    limit: u64,
    signal: &AbortSignal,
) -> std::result::Result<Vec<u8>, BoxError> {
    let result = if format == "pdf" {
        read_pdf(path, limit, signal).await
    } else {
        read_output(path, limit, signal).await
    };
    result.map_err(|cause| {
        if cause.downcast_ref::<OfficeKitError>().is_some() {
            cause
        } else {
            Box::new(OfficeKitError::with_source(
                OfficeKitErrorCode::InvalidOutput,
                "The converter output could not be read.",
                cause,
            )) as BoxError
        }
    })
}

async fn read_image_file(
    path: &Path,
    limit: u64,
    signal: &AbortSignal,
) -> std::result::Result<Vec<u8>, BoxError> {
    read_png(path, limit, signal).await.map_err(|cause| {
        if cause.downcast_ref::<OfficeKitError>().is_some() {
            cause
        } else {
            Box::new(OfficeKitError::with_source(
                OfficeKitErrorCode::InvalidOutput,
                "The rendered image could not be read.",
                cause,
            )) as BoxError
        }
    })
}

fn random_uuid() -> std::result::Result<String, ConfigError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| ConfigError(error.to_string()))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut value = String::with_capacity(36);
    for (index, byte) in bytes.into_iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            value.push('-');
        }
        use std::fmt::Write as _;
        write!(value, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(value)
}

pub struct OfficeKitPlugin;

impl Plugin for OfficeKitPlugin {
    const NAME: &'static str = "@deepseek-ai/dsh-office-kit";
    type Config = Config;
    type Deps = ();

    async fn apply(
        ctx: Context,
        _: (),
        config: Cfg<Self::Config>,
    ) -> std::result::Result<(), hoh_kernel::Error> {
        let log = ctx.logger();
        let service = Arc::new(OfficeKitService::new(
            (*config.get()).clone(),
            Arc::new(UnavailableConverterFactory),
            move |message| log.warn(message),
        )?);
        let releasing = service.clone();
        ctx.on_contained_async::<hoh_session::SessionDisposed, _>(move |event| {
            let service = releasing.clone();
            let owner = event.session.id().to_string();
            async move {
                service.release(&owner).await;
                Ok(())
            }
        })?;
        let disposing = service.clone();
        ctx.effect("office-kit dispose", move || {
            Undo::future(move || async move {
                disposing
                    .dispose()
                    .await
                    .map_err(|error| Box::new(error) as BoxError)
            })
        })?;
        ctx.provide::<OfficeKit>(service).await?;
        Ok(())
    }
}
