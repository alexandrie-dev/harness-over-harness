use hoh_error::BoxError;
use hoh_office_kit::*;
use hoh_timeout::AbortSignal;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub const PDF: &[u8] = b"%PDF-1.7\nconverted\n%%EOF\n";
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x01\x02";
pub const XLSX: &[u8] = &[80, 75, 3, 4, 9];

#[derive(Default)]
pub struct TestConverter {
    pub renders: AtomicUsize,
    pub images: AtomicUsize,
    pub conversions: AtomicUsize,
    pub recalculations: AtomicUsize,
    pub disposals: AtomicUsize,
    pub invalid_pdf: bool,
    pub outside_image: bool,
}

impl Converter for TestConverter {
    fn render(
        &self,
        request: RenderRequest,
        _signal: AbortSignal,
    ) -> ConverterFuture<ConverterResult> {
        self.renders.fetch_add(1, Ordering::SeqCst);
        let bytes = if self.invalid_pdf {
            b"not pdf".as_slice()
        } else {
            PDF
        };
        Box::pin(async move {
            tokio::fs::write(request.output_path, bytes).await?;
            Ok(ConverterResult {
                missing_fonts: vec!["Missing Serif".into()],
                page_count: None,
            })
        })
    }

    fn render_images(
        &self,
        request: RenderImagesRequest,
        _signal: AbortSignal,
    ) -> ConverterFuture<RenderImagesResult> {
        self.images.fetch_add(1, Ordering::SeqCst);
        let outside = self.outside_image;
        Box::pin(async move {
            tokio::fs::create_dir_all(&request.output_dir).await?;
            let first = request.output_dir.join("image-1.png");
            let second = if outside {
                request.input_path.clone()
            } else {
                request.output_dir.join("image-0.png")
            };
            tokio::fs::write(&first, PNG).await?;
            if !outside {
                tokio::fs::write(&second, PNG).await?;
            }
            Ok(RenderImagesResult {
                images: vec![
                    RenderedImage {
                        index: 1,
                        path: first,
                        width: 11,
                        height: 20,
                        byte_length: PNG.len() as u64,
                        page: Some(3),
                        sheet: None,
                        range: None,
                    },
                    RenderedImage {
                        index: 0,
                        path: second,
                        width: 10,
                        height: 20,
                        byte_length: PNG.len() as u64,
                        page: None,
                        sheet: Some("Data".into()),
                        range: Some("A1:B2".into()),
                    },
                ],
                page_count: 3,
                dpi: request.dpi.unwrap_or(144),
                missing_fonts: vec!["Missing Sans".into()],
            })
        })
    }

    fn convert(
        &self,
        request: ConvertRequest,
        _signal: AbortSignal,
    ) -> ConverterFuture<ConverterResult> {
        self.conversions.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let data = if request.format == OfficeConversionFormat::Pdf {
                PDF
            } else {
                XLSX
            };
            tokio::fs::write(request.output_path, data).await?;
            Ok(ConverterResult::default())
        })
    }

    fn recalculate(
        &self,
        request: ConvertRequest,
        _signal: AbortSignal,
    ) -> ConverterFuture<ConverterResult> {
        self.recalculations.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            tokio::fs::write(request.output_path, XLSX).await?;
            Ok(ConverterResult {
                missing_fonts: vec!["Calc Font".into()],
                page_count: Some(2),
            })
        })
    }

    fn dispose(&self) -> ConverterFuture<()> {
        self.disposals.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(())))
    }
}

pub struct TestFactory {
    pub creates: AtomicUsize,
    pub converter: Arc<TestConverter>,
}

impl TestFactory {
    pub fn new(converter: Arc<TestConverter>) -> Arc<Self> {
        Arc::new(Self {
            creates: AtomicUsize::new(0),
            converter,
        })
    }
}

impl ConverterFactory for TestFactory {
    fn create(&self, _options: ConverterOptions) -> ConverterFuture<Arc<dyn Converter>> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        let converter = self.converter.clone();
        Box::pin(async move { Ok(converter as Arc<dyn Converter>) })
    }
}

pub fn source(key: &str, bytes: Vec<u8>) -> OfficeKitSource {
    let len = bytes.len() as u64;
    OfficeKitSource {
        key: OfficeSourceKey::from(key),
        version: "v1".into(),
        bytes: Some(len),
        read: Arc::new(move |_signal, _limit| {
            let bytes = bytes.clone();
            Box::pin(async move {
                Ok(OfficeSourceRead {
                    bytes,
                    version: "v1".into(),
                })
            })
        }),
    }
}

pub fn service(
    converter: Arc<TestConverter>,
) -> (tempfile::TempDir, OfficeKitService, Arc<TestFactory>) {
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        output_directory: directory.path().join("outputs"),
        ..Config::default()
    };
    let factory = TestFactory::new(converter);
    let service = OfficeKitService::new(config, factory.clone(), |_| {}).unwrap();
    (directory, service, factory)
}

pub fn code(error: &hoh_error::SharedError) -> Option<OfficeKitErrorCode> {
    error
        .downcast_ref::<OfficeKitError>()
        .map(|error| error.code)
}

#[allow(dead_code)]
pub fn boxed(message: &str) -> BoxError {
    Box::new(std::io::Error::other(message.to_owned()))
}
