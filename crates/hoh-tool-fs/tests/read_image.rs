//! The `read_image` tool over the real local filesystem and the real local attachment store:
//! extension routing, extension-less content sniffing (attachment object paths included), the
//! strict image-modality gate (every refusal arm), durable commit and image-block rendering,
//! attachment admission failures, and the regression that `read` keeps its text-only contract.
//! Stores whose behavior the real one cannot be asked to show are small in-file implementations of
//! the `AttachmentStore` seam.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine;
use futures_util::future::BoxFuture;
use hoh_attachment::{
    AttachmentError, AttachmentErrorCode, AttachmentId, AttachmentStore, Attachments,
    ImageAttachmentLimits, ImageAttachmentRef, ImageMediaType, ProjectedDimensions,
    SaveImageAttachment, StoredImageAttachment,
};
use hoh_attachment_local::LocalAttachmentStore;
use hoh_error::BoxError;
use hoh_fs::{FsError, FsErrorCode, FsObservation};
use hoh_kernel::Fiber;
use hoh_llm::{
    GenerateOptions, Llm, LlmAdapter, LlmFuture, LlmModelInfo, LlmResolvedModelInfo, LlmRuntime,
    LlmStream,
};
use hoh_timeout::{AbortController, AbortError};
use hoh_tool_fs::read_image::{
    format_image_read_output, image_media_type_for_path, image_ref_from_value,
    sniff_image_media_type,
};
use hoh_tools::Agent;
use serde_json::{Value, json};
use support::patch::*;
use support::{TestAgent, error_info, named};

fn decode(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap()
}

/// 1x1 red PNG (valid signature, IHDR, IDAT).
fn png_1x1() -> Vec<u8> {
    decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC",
    )
}

/// 3x3 red PNG used to trip a tiny configured pixel limit.
fn png_3x3() -> Vec<u8> {
    decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAMAAAADCAIAAADZSiLoAAAAEElEQVR4nGP4z8AAQQxYWACPjgj4kWPEuQAAAABJRU5ErkJggg==",
    )
}

/// 1x1 red GIF (GIF89a).
fn gif_1x1() -> Vec<u8> {
    decode("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7")
}

/// The model catalog of the fake adapter: `(id, name, input modalities)`.
type Catalog = Vec<(&'static str, &'static str, Option<Vec<&'static str>>)>;

fn default_catalog() -> Catalog {
    vec![
        ("vision-model", "Vision", Some(vec!["text", "image"])),
        ("text-model", "Text", Some(vec!["text"])),
        ("legacy-model", "Legacy", None),
    ]
}

/// Exact-route fake adapter; `stream` is unreachable in these tests.
struct CatalogAdapter {
    models: Catalog,
}

impl LlmAdapter for CatalogAdapter {
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move {
            let found = self.models.iter().find(|(id, ..)| *id == model);
            Ok(LlmResolvedModelInfo {
                model: LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: found.map_or(model, |(_, name, _)| *name).into(),
                    input_modalities: found.and_then(|(.., modalities)| {
                        modalities
                            .as_ref()
                            .map(|list| list.iter().map(|m| (*m).to_owned()).collect())
                    }),
                    ..Default::default()
                },
                ..Default::default()
            })
        })
    }

    fn stream(&self, _options: GenerateOptions) -> Result<LlmStream, BoxError> {
        Err("read_image tests never stream".into())
    }
}

static AGENTS: AtomicUsize = AtomicUsize::new(0);

/// A calling agent whose Session is pinned to one routed provider/model (none when `model` is
/// `None`: no request header was ever appended).
fn agent_on(t: &T, model: Option<&str>) -> Arc<dyn Agent> {
    let id = format!("image-agent-{}", AGENTS.fetch_add(1, Ordering::SeqCst));
    let agent = TestAgent::in_dir(&t.ctx, &id, &t.workspace.to_string_lossy());
    if let Some(model) = model {
        agent
            .session
            .append(
                "request/header",
                json!({
                    "header": { "config": { "provider": "visual", "model": model } },
                    "reason": "initial",
                }),
                None,
            )
            .unwrap();
    }
    agent
}

/// One composition: tool-fs without apply_patch, an optional attachment store and Llm service.
struct Image {
    t: T,
    _home: tempfile::TempDir,
    _registrations: Vec<hoh_llm::AdapterRegistrationHandle>,
}

struct SetupOptions {
    tool_fs: Value,
    /// `None` mounts no store; `Some(config)` mounts the real local store with it.
    store: Option<Value>,
    llm: Option<Catalog>,
}

impl Default for SetupOptions {
    fn default() -> Self {
        Self {
            tool_fs: json!({}),
            store: Some(json!({})),
            llm: Some(default_catalog()),
        }
    }
}

async fn mount_llm(t: &T, catalog: Catalog) -> hoh_llm::AdapterRegistrationHandle {
    let runtime = Arc::new(LlmRuntime::with_context(&t.ctx));
    let provided = runtime.clone();
    t.ctx
        .inject(move |ctx, (): ()| {
            let runtime = provided.clone();
            async move {
                ctx.provide::<Llm>(runtime).await?;
                Ok(())
            }
        })
        .unwrap()
        .settled()
        .await
        .unwrap();
    runtime
        .register_adapter(
            &t.ctx,
            vec!["visual".into()],
            Arc::new(CatalogAdapter { models: catalog }),
        )
        .unwrap()
}

async fn mount_store_instance(t: &T, store: Arc<dyn AttachmentStore>) {
    t.ctx
        .inject(move |ctx, (): ()| {
            let store = store.clone();
            async move {
                ctx.provide::<Attachments>(store).await?;
                Ok(())
            }
        })
        .unwrap()
        .settled()
        .await
        .unwrap();
}

async fn mount_local_store(t: &T, home: &Path, config: &Value) -> Fiber {
    let mut config = config.clone();
    config["dshHome"] = json!(home.to_string_lossy());
    let fiber = t.ctx.plugin::<LocalAttachmentStore>(config).unwrap();
    fiber.settled().await.unwrap();
    fiber
}

/// Replace the default tool-fs of the harness with one configured by `config`.
async fn remount_tool_fs(t: &mut T, config: Value) {
    t.tool_fs.dispose().await;
    let fiber = t.ctx.plugin::<hoh_tool_fs::ToolFs>(config).unwrap();
    fiber.settled().await.unwrap();
    t.tool_fs = fiber;
}

async fn image(options: SetupOptions) -> Image {
    let mut t = setup(PatchOptions {
        without_apply_patch: true,
        ..PatchOptions::default()
    })
    .await;
    if options.tool_fs != json!({}) {
        remount_tool_fs(&mut t, options.tool_fs).await;
    }
    let home = tempfile::tempdir().unwrap();
    if let Some(config) = &options.store {
        mount_local_store(&t, home.path(), config).await;
    }
    let mut registrations = Vec::new();
    if let Some(catalog) = options.llm {
        registrations.push(mount_llm(&t, catalog).await);
    }
    Image {
        t,
        _home: home,
        _registrations: registrations,
    }
}

async fn standard() -> Image {
    image(SetupOptions::default()).await
}

fn names(t: &T) -> Vec<String> {
    let mut names: Vec<String> = t
        .env
        .tools
        .schemas(None, None)
        .unwrap()
        .into_iter()
        .map(|schema| schema.name)
        .collect();
    names.sort();
    names
}

fn write(t: &T, name: &str, bytes: &[u8]) {
    std::fs::write(t.path(name), bytes).unwrap();
}

async fn read_image(
    t: &T,
    arguments: Value,
    agent: Option<Arc<dyn Agent>>,
) -> hoh_tools::ToolExecutionResult {
    support::call(&t.ctx, "read_image", arguments, agent).await
}

fn image_ref(result: &hoh_tools::ToolExecutionResult) -> ImageAttachmentRef {
    let block = &result.content()[1];
    assert_eq!(block.r#type, "image");
    serde_json::from_value(block.data["attachment"].clone()).unwrap()
}

fn code(result: &hoh_tools::ToolExecutionResult) -> Option<(String, String)> {
    error_info(result)
}

mod image_media_type_for_path_mapping {
    use super::*;

    #[test]
    fn maps_the_four_extensions_case_insensitively_and_rejects_everything_else() {
        assert_eq!(
            image_media_type_for_path("a.png"),
            Some(ImageMediaType::Png)
        );
        assert_eq!(
            image_media_type_for_path("a.JPG"),
            Some(ImageMediaType::Jpeg)
        );
        assert_eq!(
            image_media_type_for_path("b.jpeg"),
            Some(ImageMediaType::Jpeg)
        );
        assert_eq!(
            image_media_type_for_path("c.webp"),
            Some(ImageMediaType::Webp)
        );
        assert_eq!(
            image_media_type_for_path("d.Gif"),
            Some(ImageMediaType::Gif)
        );
        assert_eq!(image_media_type_for_path("note.txt"), None);
        assert_eq!(image_media_type_for_path("png"), None);
    }

    /// `read-image.ts:325` takes the shared Node `extname`: `..` and `a/..` have none (a
    /// directory, not a ".": extension that declares no format).
    #[test]
    fn dot_dot_has_no_extension_like_node() {
        assert_eq!(image_media_type_for_path(".."), None);
        assert_eq!(image_media_type_for_path("a/.."), None);
        assert_eq!(image_media_type_for_path("a/../"), None);
        assert_eq!(image_media_type_for_path("dir/.png"), None);
    }
}

mod sniffing {
    use super::*;

    #[test]
    fn identifies_each_supported_container_from_its_complete_signature() {
        assert_eq!(
            sniff_image_media_type(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00]),
            Some(ImageMediaType::Png)
        );
        assert_eq!(
            sniff_image_media_type(&[0xff, 0xd8, 0xff, 0xe0]),
            Some(ImageMediaType::Jpeg)
        );
        assert_eq!(
            sniff_image_media_type(b"GIF87a..."),
            Some(ImageMediaType::Gif)
        );
        assert_eq!(
            sniff_image_media_type(b"GIF89a..."),
            Some(ImageMediaType::Gif)
        );
        assert_eq!(
            sniff_image_media_type(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageMediaType::Webp)
        );
    }

    #[test]
    fn returns_none_for_other_bytes_incomplete_signatures_and_non_webp_riff_containers() {
        assert_eq!(sniff_image_media_type(&[]), None);
        assert_eq!(sniff_image_media_type(b"plain text"), None);
        assert_eq!(sniff_image_media_type(&[0x89, 0x50, 0x4e]), None);
        assert_eq!(sniff_image_media_type(&[0xff, 0xd8]), None);
        assert_eq!(sniff_image_media_type(b"GIF90a"), None);
        assert_eq!(sniff_image_media_type(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(sniff_image_media_type(b"RIFF\0\0\0"), None);
    }
}

mod image_ref_from_value_branding {
    use super::*;

    #[test]
    fn re_brands_with_and_without_the_optional_display_name() {
        let base = json!({ "attachmentId": "sha256:00", "mediaType": "image/png", "bytes": 1, "width": 1, "height": 1 });
        let expected = ImageAttachmentRef {
            attachment_id: AttachmentId::from("sha256:00"),
            media_type: ImageMediaType::Png,
            bytes: 1,
            width: 1,
            height: 1,
            name: None,
            original_dimensions: None,
        };
        assert_eq!(image_ref_from_value(&base), Some(expected.clone()));
        let mut named = base.clone();
        named["name"] = json!("a.png");
        assert_eq!(
            image_ref_from_value(&named),
            Some(ImageAttachmentRef {
                name: Some("a.png".into()),
                ..expected.clone()
            })
        );
        let mut scaled = base;
        scaled["originalDimensions"] = json!({ "width": 4, "height": 2 });
        assert_eq!(
            image_ref_from_value(&scaled),
            Some(ImageAttachmentRef {
                original_dimensions: Some(ProjectedDimensions {
                    width: 4,
                    height: 2
                }),
                ..expected
            })
        );
    }
}

mod happy_path {
    use super::*;

    #[tokio::test]
    async fn commits_the_bytes_durably_and_renders_the_envelope_beside_an_image_block() {
        let i = standard().await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;

        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(result.content().len(), 2);
        let reference = image_ref(&result);
        assert_eq!(reference.media_type, ImageMediaType::Png);
        assert_eq!((reference.width, reference.height), (1, 1));
        assert_eq!(reference.bytes, png_1x1().len() as u64);
        assert_eq!(reference.name.as_deref(), Some("red.png"));
        let id = reference.attachment_id.as_str();
        assert!(
            id.starts_with("sha256:")
                && id.len() == 7 + 64
                && id[7..].chars().all(|c| c.is_ascii_hexdigit())
        );
        assert_eq!(
            result.text(),
            format_image_read_output(
                &t.path_text("red.png"),
                &ImageAttachmentRef {
                    name: None,
                    ..reference.clone()
                }
            )
        );

        // The committed object must read back verbatim through the store.
        let attachments = t.ctx.get::<Attachments>().expect("the attachment service");
        let stored = attachments.read_image(&reference, None).await.unwrap();
        assert_eq!(stored.data, png_1x1());
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn commits_a_gif_durably_and_renders_the_normalized_envelope_beside_an_image_block() {
        let i = standard().await;
        let t = &i.t;
        write(t, "red.gif", &gif_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red.gif" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;

        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(result.content().len(), 2);
        let reference = image_ref(&result);
        // Normalization re-encodes this transparent 1x1 GIF as WebP: the bytes do not pass through
        // unchanged, only the source file name survives.
        assert_eq!(reference.media_type, ImageMediaType::Webp);
        assert_eq!((reference.width, reference.height), (1, 1));
        assert_eq!(reference.bytes, 72);
        assert_eq!(reference.name.as_deref(), Some("red.gif"));
        assert_eq!(
            result.text(),
            format_image_read_output(
                &t.path_text("red.gif"),
                &ImageAttachmentRef {
                    name: None,
                    ..reference.clone()
                }
            )
        );
        let attachments = t.ctx.get::<Attachments>().expect("the attachment service");
        let stored = attachments.read_image(&reference, None).await.unwrap();
        assert_eq!(&stored.data[..4], b"RIFF");
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn emits_fs_observed_for_the_read_image() {
        let i = standard().await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        let observed: Vec<String> = support::lock(&t.observations)
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(observed, vec![t.path_text("red.png")]);
        t.kernel.shutdown().await;
    }
}

mod extension_less_paths {
    use super::*;

    #[tokio::test]
    async fn reads_a_normalized_attachment_object_path_directly_and_dedups_to_the_stored_reference()
    {
        let i = standard().await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let first = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(!first.is_error(), "{}", first.text());
        let reference = image_ref(&first);
        let attachments = t.ctx.get::<Attachments>().unwrap();
        let object_path = attachments
            .image_host_path(&reference)
            .unwrap()
            .expect("a host-file-backed store");

        let second = read_image(
            t,
            json!({ "file_path": object_path }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(!second.is_error(), "{}", second.text());
        let reread = image_ref(&second);
        assert_eq!(reread.attachment_id, reference.attachment_id);
        assert_eq!(reread.media_type, ImageMediaType::Png);
        let real = std::fs::canonicalize(&object_path).unwrap();
        assert!(
            second
                .text()
                .contains(&format!("<path>{}</path>", real.display()))
        );
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn reads_an_ordinary_extension_less_image_file_by_sniffing_its_content() {
        let i = standard().await;
        let t = &i.t;
        write(t, "avatar", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "avatar" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        let reference = image_ref(&result);
        assert_eq!(reference.media_type, ImageMediaType::Png);
        assert_eq!(reference.name.as_deref(), Some("avatar"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn pins_the_trailing_dot_refusal_and_reads_a_dotfile_through_sniffing() {
        let i = standard().await;
        let t = &i.t;
        write(t, ".hidden", &png_1x1());
        let trailing = read_image(
            t,
            json!({ "file_path": "foo." }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(trailing.is_error());
        assert!(trailing.text().contains(
            "cannot read \"foo.\": the . extension does not declare a supported image format"
        ));
        assert_eq!(
            code(&trailing),
            named("ImageInputError", "IMAGE_UNSUPPORTED_FORMAT")
        );

        let dotfile = read_image(
            t,
            json!({ "file_path": ".hidden" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(!dotfile.is_error(), "{}", dotfile.text());
        let reference = image_ref(&dotfile);
        assert_eq!(reference.media_type, ImageMediaType::Png);
        assert_eq!(reference.name.as_deref(), Some(".hidden"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn refuses_extension_less_bytes_that_are_not_a_supported_image() {
        let i = standard().await;
        let t = &i.t;
        write(t, "notes", b"plain text, not an image");
        let result = read_image(
            t,
            json!({ "file_path": "notes" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains(&format!(
            "cannot read \"{}\": the file content is not a supported image format",
            t.path_text("notes")
        )));
        assert_eq!(
            code(&result),
            named("ImageInputError", "IMAGE_UNSUPPORTED_FORMAT")
        );
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn explains_extension_less_bytes_that_sniff_as_an_image_but_do_not_decode() {
        let i = standard().await;
        let t = &i.t;
        write(t, "broken", &png_1x1()[..16]);
        let result = read_image(
            t,
            json!({ "file_path": "broken" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("do not decode as a supported PNG/JPEG/WebP/GIF image")
        );
        assert_eq!(code(&result), named("AttachmentError", "INVALID_IMAGE"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn applies_the_deployment_media_type_policy_to_the_sniffed_format() {
        let i = image(SetupOptions {
            store: None,
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        mount_store_instance(
            t,
            Stub::new(&[ImageMediaType::Jpeg])
                .unreachable_validation()
                .unreachable_save()
                .arc(),
        )
        .await;
        write(t, "avatar", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "avatar" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("image/png images are not accepted by this deployment")
        );
        assert_eq!(
            code(&result),
            named("ImageInputError", "IMAGE_UNSUPPORTED_FORMAT")
        );
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn names_a_signature_decoded_format_disagreement_on_an_extension_less_path() {
        let i = image(SetupOptions {
            store: None,
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        mount_store_instance(
            t,
            Stub::new(&[ImageMediaType::Png])
                .on_save(|_| {
                    Err(AttachmentError::new(
                        "Declared image type does not match its bytes.",
                        AttachmentErrorCode::ImageTypeMismatch,
                    )
                    .into())
                })
                .arc(),
        )
        .await;
        write(t, "sniffed", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "sniffed" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains(
            "the file signature claims image/png, but the bytes decode as a different image format"
        ));
        assert_eq!(
            code(&result),
            named("AttachmentError", "IMAGE_TYPE_MISMATCH")
        );
        t.kernel.shutdown().await;
    }
}

/// The scripted commit of a [`Stub`].
type SaveFn =
    Box<dyn Fn(&SaveImageAttachment) -> Result<ImageAttachmentRef, BoxError> + Send + Sync>;

/// A store whose limits, validation and commit the test scripts.
struct Stub {
    limits: ImageAttachmentLimits,
    validate_panics: bool,
    save: SaveFn,
}

impl Stub {
    fn new(media_types: &[ImageMediaType]) -> Self {
        Self {
            limits: ImageAttachmentLimits {
                max_image_bytes: 1024,
                max_images_per_message: 1,
                max_message_image_bytes: 1024,
                max_image_pixels: 100,
                max_image_dimension: 2000,
                media_types: media_types.to_vec(),
            },
            validate_panics: false,
            save: Box::new(|_| panic!("unreachable: the commit was not scripted")),
        }
    }

    fn unreachable_validation(mut self) -> Self {
        self.validate_panics = true;
        self
    }

    fn unreachable_save(mut self) -> Self {
        self.save = Box::new(|_| panic!("unreachable: admission refuses before save"));
        self
    }

    fn on_save(
        mut self,
        save: impl Fn(&SaveImageAttachment) -> Result<ImageAttachmentRef, BoxError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.save = Box::new(save);
        self
    }

    fn arc(self) -> Arc<dyn AttachmentStore> {
        Arc::new(self)
    }
}

impl AttachmentStore for Stub {
    fn image_limits(&self) -> &ImageAttachmentLimits {
        &self.limits
    }

    fn validate_image<'a>(
        &'a self,
        _input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        assert!(
            !self.validate_panics,
            "unreachable: the sniffed-format policy refuses before validation"
        );
        Box::pin(async { Ok(()) })
    }

    fn save_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
        Box::pin(async move { (self.save)(input) })
    }

    fn read_image<'a>(
        &'a self,
        _reference: &'a ImageAttachmentRef,
        _signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
        panic!("unreachable in this test")
    }
}

mod strict_image_modality_gate {
    use super::*;

    #[tokio::test]
    async fn accepts_an_exact_visual_route_even_when_the_advisory_model_catalog_omits_it() {
        let i = image(SetupOptions {
            llm: Some(vec![(
                "hidden-vision",
                "Hidden Vision",
                Some(vec!["text", "image"]),
            )]),
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("hidden-vision"))),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn refuses_on_a_text_only_a_modality_less_or_unknown_model() {
        for model in ["text-model", "legacy-model", "unknown-model"] {
            let i = standard().await;
            let t = &i.t;
            write(t, "red.png", &png_1x1());
            let result = read_image(
                t,
                json!({ "file_path": "red.png" }),
                Some(agent_on(t, Some(model))),
            )
            .await;
            assert!(result.is_error(), "{model}");
            assert!(
                result.text().contains("does not declare image input"),
                "{model}"
            );
            assert_eq!(
                code(&result),
                named("ImageInputError", "MODEL_DOES_NOT_SUPPORT_IMAGES"),
                "{model}"
            );
            t.kernel.shutdown().await;
        }
    }

    #[tokio::test]
    async fn refuses_when_the_route_cannot_be_resolved_no_agent_or_no_header() {
        let i = standard().await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let no_agent = read_image(t, json!({ "file_path": "red.png" }), None).await;
        assert!(no_agent.is_error());
        assert!(no_agent.text().contains("route could not be resolved"));
        assert_eq!(
            code(&no_agent),
            named("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE")
        );

        let no_route = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, None)),
        )
        .await;
        assert!(no_route.is_error());
        assert!(no_route.text().contains("route could not be resolved"));
        assert_eq!(
            code(&no_route),
            named("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE")
        );
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn refuses_when_no_llm_service_is_mounted() {
        let i = image(SetupOptions {
            llm: None,
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("route could not be resolved"));
        assert_eq!(
            code(&result),
            named("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE")
        );
        t.kernel.shutdown().await;
    }
}

mod argument_and_service_preconditions {
    use super::*;

    #[tokio::test]
    async fn rejects_an_empty_path_and_a_non_image_extension() {
        let i = standard().await;
        let t = &i.t;
        let empty = read_image(
            t,
            json!({ "file_path": "   " }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(empty.is_error());
        assert!(
            empty
                .text()
                .contains("file_path must be a non-empty string")
        );
        assert_eq!(code(&empty), named("ToolArgsError", "INVALID_ARGS"));

        let non_image = read_image(
            t,
            json!({ "file_path": "notes.txt" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(non_image.is_error());
        assert!(
            non_image
                .text()
                .contains("the .txt extension does not declare a supported image format")
        );
        assert_eq!(
            code(&non_image),
            named("ImageInputError", "IMAGE_UNSUPPORTED_FORMAT")
        );
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn refuses_when_no_attachment_service_is_mounted() {
        for tool_fs in [
            json!({}),
            json!({ "read": false, "write": false, "edit": false }),
        ] {
            let i = image(SetupOptions {
                tool_fs: tool_fs.clone(),
                store: None,
                ..SetupOptions::default()
            })
            .await;
            let t = &i.t;
            write(t, "red.png", &png_1x1());
            assert!(
                t.env.tools.get("read_image", None, None).is_none(),
                "{tool_fs}"
            );
            assert!(!names(t).contains(&"read_image".to_owned()), "{tool_fs}");
            let result = read_image(
                t,
                json!({ "file_path": "red.png" }),
                Some(agent_on(t, Some("vision-model"))),
            )
            .await;
            assert!(result.is_error(), "{tool_fs}");
            assert!(
                result.text().contains("unknown tool \"read_image\""),
                "{tool_fs}: {}",
                result.text()
            );
            t.kernel.shutdown().await;
        }
    }

    #[tokio::test]
    async fn defensively_refuses_execution_without_an_attachment_service() {
        let i = image(SetupOptions {
            store: None,
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let env = Arc::new(t.env.clone());
        hoh_tool_fs::read_image::apply_read_image_tool(&env).unwrap();
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("no attachment service is mounted"));
        assert_eq!(
            code(&result),
            named("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE")
        );
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn refuses_a_media_type_the_deployment_does_not_accept() {
        let i = image(SetupOptions {
            store: None,
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        mount_store_instance(
            t,
            Stub::new(&[ImageMediaType::Jpeg])
                .unreachable_validation()
                .unreachable_save()
                .arc(),
        )
        .await;
        write(t, "red.png", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("image/png images are not accepted by this deployment")
        );
        assert_eq!(
            code(&result),
            named("ImageInputError", "IMAGE_UNSUPPORTED_FORMAT")
        );
        t.kernel.shutdown().await;
    }
}

mod image_admission_failures {
    use super::*;

    #[tokio::test]
    async fn explains_how_to_repair_a_declared_actual_media_type_mismatch() {
        let i = standard().await;
        let t = &i.t;
        write(t, "wrong.jpg", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "wrong.jpg" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("the .jpg extension declares image/jpeg")
        );
        assert!(result.text().contains(
            "rename the file to match its actual format if it is PNG/JPEG/WebP/GIF, or convert it to one of those formats"
        ));
        assert_eq!(
            code(&result),
            named("AttachmentError", "IMAGE_TYPE_MISMATCH")
        );
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn caps_an_extension_less_read_at_max_image_bytes_before_format_detection() {
        let i = image(SetupOptions {
            store: Some(json!({ "maxImageBytes": png_1x1().len() - 1 })),
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        write(t, "red", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("exceeds"));
        assert_eq!(code(&result), named("FsError", "FS_TOO_LARGE"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn honors_the_tighter_per_message_aggregate_byte_bound() {
        let i = image(SetupOptions {
            store: Some(json!({ "maxMessageImageBytes": png_1x1().len() - 1 })),
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(result.text().contains("exceeds"));
        assert_eq!(code(&result), named("FsError", "FS_TOO_LARGE"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn surfaces_the_pixel_limit_from_the_attachment_admission() {
        let i = image(SetupOptions {
            store: Some(json!({ "maxImagePixels": 4 })),
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        write(t, "big.png", &png_3x3());
        let result = read_image(
            t,
            json!({ "file_path": "big.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("exceeds the 4-pixel decoded-size limit")
        );
        assert_eq!(
            code(&result),
            named("AttachmentError", "IMAGE_TOO_MANY_PIXELS")
        );
        assert!(
            result
                .text()
                .contains("downscale the image and read the smaller copy")
        );
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn surfaces_the_per_side_limit_from_attachment_admission() {
        let i = image(SetupOptions {
            store: Some(json!({ "maxImageDimension": 2 })),
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        write(t, "wide.png", &png_3x3());
        let result = read_image(
            t,
            json!({ "file_path": "wide.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        assert!(
            result
                .text()
                .contains("at least one image side exceeds the 2px limit")
        );
        assert_eq!(
            code(&result),
            named("AttachmentError", "IMAGE_DIMENSION_TOO_LARGE")
        );
        assert!(
            result
                .text()
                .contains("downscale the image and read the smaller copy")
        );
        t.kernel.shutdown().await;
    }

    /// A store whose commit runs a scripted failure; admission itself passes.
    async fn failing_save(
        failure: impl Fn() -> BoxError + Send + Sync + 'static,
        controller: Option<Arc<AbortController>>,
    ) -> (hoh_tools::ToolExecutionResult, String) {
        let i = image(SetupOptions {
            store: None,
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        mount_store_instance(
            t,
            Stub::new(&[ImageMediaType::Png])
                .on_save(move |_| Err(failure()))
                .arc(),
        )
        .await;
        write(t, "red.png", &png_1x1());
        let agent = agent_on(t, Some("vision-model"));
        let result = match &controller {
            Some(controller) => {
                support::call_with_signal(
                    &t.ctx,
                    "read_image",
                    json!({ "file_path": "red.png" }),
                    Some(agent),
                    controller.signal(),
                )
                .await
            }
            None => read_image(t, json!({ "file_path": "red.png" }), Some(agent)).await,
        };
        let path = t.path_text("red.png");
        t.kernel.shutdown().await;
        (result, path)
    }

    #[tokio::test]
    async fn keeps_attachment_error_codes_and_adds_the_read_path_and_remedy() {
        let byte_limits = "the image cannot be stored within the deployment's byte limits; downscale the image and read the smaller copy";
        let cases: [(AttachmentErrorCode, &str, &str); 5] = [
            (
                AttachmentErrorCode::ImageTooLarge,
                "Image cannot be encoded within the configured normalized-image byte cap.",
                byte_limits,
            ),
            (
                AttachmentErrorCode::ImagesTooLarge,
                "Image batch exceeds the configured aggregate image-byte limit.",
                byte_limits,
            ),
            (
                AttachmentErrorCode::UnsupportedImageType,
                "Image type image/png is not accepted by this deployment.",
                "image/png images are not accepted by this deployment",
            ),
            (
                AttachmentErrorCode::InvalidImage,
                "Unsupported or malformed image data.",
                "the bytes do not decode as a supported PNG/JPEG/WebP/GIF image; the file may be truncated or corrupt",
            ),
            (
                AttachmentErrorCode::AttachmentWriteFailed,
                "The 16-bit PNG could not be converted to the normalized 8-bit sRGB form.",
                "the 16-bit PNG could not be converted to the normalized 8-bit sRGB form; convert it to an 8-bit PNG/JPEG/WebP and retry",
            ),
        ];
        for (code, message, detail) in cases {
            let (result, path) =
                failing_save(move || AttachmentError::new(message, code).into(), None).await;
            assert!(result.is_error(), "{code}");
            assert_eq!(
                error_info(&result),
                named("AttachmentError", code.as_str()),
                "{code}"
            );
            assert_eq!(
                result.text(),
                format!("Error: cannot read \"{path}\": {detail}"),
                "{code}"
            );
        }
    }

    #[tokio::test]
    async fn passes_attachment_error_codes_without_a_read_remedy_through_unchanged() {
        for code in [
            AttachmentErrorCode::AttachmentWriteFailed,
            AttachmentErrorCode::TooManyImages,
            AttachmentErrorCode::InvalidImageBase64,
            AttachmentErrorCode::InvalidFileBase64,
            AttachmentErrorCode::InvalidAttachmentRef,
            AttachmentErrorCode::AttachmentCorrupt,
            AttachmentErrorCode::AttachmentNotFound,
            AttachmentErrorCode::AttachmentReadFailed,
            AttachmentErrorCode::AttachmentProjectionUnsupported,
            AttachmentErrorCode::AttachmentFilesUnsupported,
        ] {
            let (result, _) = failing_save(
                move || AttachmentError::new("Stored attachment failed.", code).into(),
                None,
            )
            .await;
            assert!(result.is_error(), "{code}");
            assert_eq!(
                error_info(&result),
                named("AttachmentError", code.as_str()),
                "{code}"
            );
            assert_eq!(result.text(), "Error: Stored attachment failed.", "{code}");
        }
    }

    #[tokio::test]
    async fn passes_a_store_defect_a_classified_harness_error_and_an_abort_error_through_unchanged()
    {
        let (defect, _) = failing_save(|| "unrelated infrastructure failure".into(), None).await;
        assert!(defect.is_error());
        assert_eq!(error_info(&defect), None);
        assert_eq!(defect.text(), "Error: unrelated infrastructure failure");

        let (fs_failure, _) = failing_save(
            || {
                FsError::new(
                    "cannot read \"x\": permission denied",
                    FsErrorCode::PermissionDenied,
                )
                .into()
            },
            None,
        )
        .await;
        assert_eq!(
            error_info(&fs_failure),
            named("FsError", "FS_PERMISSION_DENIED")
        );
        assert_eq!(
            fs_failure.text(),
            "Error: cannot read \"x\": permission denied"
        );

        let (abort, _) = failing_save(|| AbortError::new("store stopped").into(), None).await;
        assert!(abort.text().contains("store stopped"));
        assert!(!abort.text().contains("cannot read"));
    }

    #[tokio::test]
    async fn lets_the_registry_classify_an_abort_error_thrown_after_caller_cancellation() {
        let controller = Arc::new(AbortController::new());
        let inner = controller.clone();
        let (result, _) = failing_save(
            move || {
                inner.abort();
                AbortError::new("save cancelled").into()
            },
            Some(controller),
        )
        .await;
        assert!(result.is_error());
        assert_eq!(error_info(&result), named("AbortError", "ABORTED"));
        assert!(!result.text().contains("cannot read"));
    }

    #[tokio::test]
    async fn keeps_a_save_failure_that_is_not_cancellation_even_after_the_caller_aborts() {
        let controller = Arc::new(AbortController::new());
        let inner = controller.clone();
        let (result, _) = failing_save(
            move || {
                inner.abort();
                AttachmentError::new(
                    "Unable to persist attachment.",
                    AttachmentErrorCode::AttachmentWriteFailed,
                )
                .into()
            },
            Some(controller),
        )
        .await;
        assert_eq!(
            error_info(&result),
            named("AttachmentError", "ATTACHMENT_WRITE_FAILED")
        );
    }

    #[tokio::test]
    async fn reports_a_missing_image_file_and_a_directory_target_through_the_fs_vocabulary() {
        let i = standard().await;
        let t = &i.t;
        std::fs::create_dir(t.path("folder.png")).unwrap();
        let missing = read_image(
            t,
            json!({ "file_path": "absent.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(missing.is_error());
        assert!(missing.text().contains("not found"));
        assert_eq!(code(&missing), named("FsError", "FS_NOT_FOUND"));
        let observed: Vec<(String, bool)> = support::lock(&t.observations)
            .iter()
            .map(|(path, observation)| (path.clone(), matches!(observation, FsObservation::Absent)))
            .collect();
        assert_eq!(observed, vec![(t.path_text("absent.png"), true)]);

        let directory = read_image(
            t,
            json!({ "file_path": "folder.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(directory.is_error());
        assert!(directory.text().contains("not a regular file"));
        assert_eq!(code(&directory), named("FsError", "FS_NOT_REGULAR_FILE"));

        // `..` is a directory with no extension, not a file with a "." extension.
        for dotted in ["..", "folder.png/.."] {
            let parent = read_image(
                t,
                json!({ "file_path": dotted }),
                Some(agent_on(t, Some("vision-model"))),
            )
            .await;
            assert!(parent.is_error(), "{dotted}");
            assert_eq!(
                code(&parent),
                named("FsError", "FS_NOT_REGULAR_FILE"),
                "{dotted}: {}",
                parent.text()
            );
        }
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn omits_the_display_name_when_the_store_returns_a_reference_without_one() {
        let i = image(SetupOptions {
            store: None,
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        mount_store_instance(
            t,
            Stub::new(&[ImageMediaType::Png])
                .on_save(|input| {
                    Ok(ImageAttachmentRef {
                        attachment_id: AttachmentId::from("sha256:feed"),
                        media_type: input.media_type,
                        bytes: input.data.len() as u64,
                        width: 1,
                        height: 1,
                        name: None,
                        original_dimensions: None,
                    })
                })
                .arc(),
        )
        .await;
        write(t, "red.png", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(image_ref(&result).name, None);
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn names_the_on_disk_dimensions_and_coordinate_multiplier_when_storage_downscales() {
        let i = image(SetupOptions {
            store: None,
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        mount_store_instance(
            t,
            Stub::new(&[ImageMediaType::Png])
                .on_save(|input| {
                    Ok(ImageAttachmentRef {
                        attachment_id: AttachmentId::from("sha256:feed"),
                        media_type: input.media_type,
                        bytes: 7,
                        width: 2,
                        height: 1,
                        name: None,
                        original_dimensions: Some(ProjectedDimensions {
                            width: 4,
                            height: 2,
                        }),
                    })
                })
                .arc(),
        )
        .await;
        write(t, "red.png", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert!(result.text().contains(
            "image/png image, 2x1 px, 7 bytes (downscaled from 4x2 px; multiply coordinates by 2.00 to locate features in the original file)"
        ));
        t.kernel.shutdown().await;
    }

    #[test]
    fn names_per_axis_multipliers_when_integer_rounding_makes_the_ratios_differ() {
        let envelope = format_image_read_output(
            "/img/photo.jpg",
            &ImageAttachmentRef {
                attachment_id: AttachmentId::from("sha256:feed"),
                media_type: ImageMediaType::Jpeg,
                bytes: 9,
                width: 2,
                height: 1,
                name: None,
                original_dimensions: Some(ProjectedDimensions {
                    width: 5,
                    height: 2,
                }),
            },
        );
        assert!(envelope.contains(
            "downscaled from 5x2 px; multiply x coordinates by 2.50 and y coordinates by 2.00 to locate features in the original file"
        ));
    }
}

mod tie_rounding {
    use super::*;

    fn advice(from: (u64, u64), to: (u64, u64)) -> String {
        format_image_read_output(
            "/img/photo.png",
            &ImageAttachmentRef {
                attachment_id: AttachmentId::from("sha256:feed"),
                media_type: ImageMediaType::Png,
                bytes: 9,
                width: to.0,
                height: to.1,
                name: None,
                original_dimensions: Some(ProjectedDimensions {
                    width: from.0,
                    height: from.1,
                }),
            },
        )
    }

    /// `toFixed(2)` rounds an exact tie up: 9/8 = 1.125 is 1.13, not the even 1.12.
    #[test]
    fn rounds_an_exact_binary_tie_up_like_to_fixed() {
        assert!(advice((9, 9), (8, 8)).contains("multiply coordinates by 1.13 "));
        assert!(advice((2250, 1125), (2000, 1000)).contains("multiply coordinates by 1.13 "));
        // Not a tie: unchanged.
        assert!(advice((4, 2), (2, 1)).contains("by 2.00 "));
    }
}

mod registration_surface {
    use super::*;

    #[tokio::test]
    async fn registers_only_read_image_without_text_guidance() {
        let i = image(SetupOptions {
            tool_fs: json!({ "read": false, "write": false, "edit": false }),
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        assert_eq!(names(t), vec!["read_image".to_owned()]);
        let prompt = t.ctx.get::<hoh_system_prompt::SystemPromptKey>().unwrap();
        let assembly = prompt
            .assemble(hoh_system_prompt::AssembleContext::default())
            .await
            .unwrap();
        assert!(
            !assembly
                .sections
                .iter()
                .any(|section| section.name.starts_with("tool:"))
        );
        write(t, "red.png", &png_1x1());
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert!(result.content().iter().any(|block| block.r#type == "image"));
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn omits_read_image_while_an_attachment_store_is_mounted_when_read_image_is_false() {
        let i = image(SetupOptions {
            tool_fs: json!({ "readImage": false }),
            ..SetupOptions::default()
        })
        .await;
        let t = &i.t;
        assert_eq!(
            names(t),
            vec!["edit".to_owned(), "read".to_owned(), "write".to_owned()]
        );
        assert!(t.env.tools.get("read_image", None, None).is_none());
        let result = read_image(
            t,
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(result.is_error());
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn withdraws_and_restores_image_only_registration_with_the_attachment_store() {
        let mut t = setup(PatchOptions {
            without_apply_patch: true,
            ..PatchOptions::default()
        })
        .await;
        remount_tool_fs(
            &mut t,
            json!({ "read": false, "write": false, "edit": false }),
        )
        .await;
        let home = tempfile::tempdir().unwrap();
        let attachment = mount_local_store(&t, home.path(), &json!({})).await;
        assert_eq!(names(&t), vec!["read_image".to_owned()]);
        attachment.dispose().await;
        assert_eq!(names(&t), Vec::<String>::new());
        mount_local_store(&t, home.path(), &json!({})).await;
        assert_eq!(names(&t), vec!["read_image".to_owned()]);
        t.tool_fs.dispose().await;
        assert_eq!(names(&t), Vec::<String>::new());
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn withdraws_read_image_when_the_tool_fs_fiber_or_the_attachment_store_is_disposed() {
        let t = setup(PatchOptions {
            without_apply_patch: true,
            ..PatchOptions::default()
        })
        .await;
        let home = tempfile::tempdir().unwrap();
        let attachments = mount_local_store(&t, home.path(), &json!({})).await;
        let all = || {
            vec![
                "edit".to_owned(),
                "read".to_owned(),
                "read_image".to_owned(),
                "write".to_owned(),
            ]
        };
        assert_eq!(names(&t), all());

        // Disposing only the attachment store tears down the scoped inject fiber: read_image
        // withdraws while the unconditional tools stay registered.
        attachments.dispose().await;
        assert_eq!(
            names(&t),
            vec!["edit".to_owned(), "read".to_owned(), "write".to_owned()]
        );

        // Remounting the store restores the conditional registration.
        let remounted = mount_local_store(&t, home.path(), &json!({})).await;
        assert_eq!(names(&t), all());

        // Disposing the whole plugin withdraws every tool, read_image included.
        t.tool_fs.dispose().await;
        assert_eq!(names(&t), Vec::<String>::new());
        remounted.dispose().await;
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn declares_read_image_parallel_safe_and_presents_a_read_family_card() {
        let i = standard().await;
        let t = &i.t;
        let pipeline = hoh_tools::ToolExecutionPipeline::new(t.ctx.clone()).unwrap();
        let mode = pipeline
            .execution_mode(&hoh_tools::ToolExecutionInput::new(
                "img-parallel".into(),
                "read_image",
                json!({ "file_path": "a.png" }),
                support::live_signal(),
            ))
            .unwrap()
            .kind();
        assert_eq!(mode, "parallel");
        let definition = t.env.tools.get("read_image", None, None).unwrap();
        let present = definition.present_call.as_ref().unwrap();
        assert_eq!(
            serde_json::to_value(present(&json!({ "file_path": "shot.png" }))).unwrap(),
            json!({
                "card": "generic",
                "title": "Read image shot.png",
                "kind": "read",
                "locations": [{ "path": "shot.png" }],
            })
        );
        t.kernel.shutdown().await;
    }
}

mod read_keeps_its_text_only_contract {
    use super::*;

    #[tokio::test]
    async fn still_refuses_a_png_as_a_binary_file_and_line_numbers_text() {
        let i = standard().await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        write(t, "note.txt", b"hello\nworld");
        let png = support::call(
            &t.ctx,
            "read",
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(png.is_error());
        assert!(png.text().contains("binary file"));
        let txt = support::call(
            &t.ctx,
            "read",
            json!({ "file_path": "note.txt" }),
            Some(agent_on(t, Some("text-model"))),
        )
        .await;
        assert!(!txt.is_error(), "{}", txt.text());
        assert!(txt.text().contains("1: hello"));
        assert!(txt.text().contains("<type>file</type>"));
        t.kernel.shutdown().await;
    }
}

mod image_result_presentation {
    use super::*;

    #[tokio::test]
    async fn persists_the_path_only_leaving_the_reference_to_the_result_content() {
        // The settled content already carries the image block with the complete reference, so
        // copying it into meta would keep two records of one fact and a post-execute content
        // replacement would strand the stale copy.
        let i = standard().await;
        let t = &i.t;
        let reference = json!({
            "attachmentId": format!("sha256:{}", "a".repeat(64)),
            "mediaType": "image/png",
            "bytes": 24_588,
            "width": 1496,
            "height": 260,
            "name": "card.png",
        });
        let value = json!({ "path": "/w/app/shots/card.png", "image": reference });
        let definition = t.env.tools.get("read_image", None, None).unwrap();
        let meta = definition.output.presentation_meta.as_ref().unwrap();
        assert_eq!(
            meta(&json!({ "file_path": "shots/card.png" }), &value).unwrap(),
            json!({ "path": "/w/app/shots/card.png" })
        );
        t.kernel.shutdown().await;
    }

    #[tokio::test]
    async fn carries_the_committed_reference_in_the_result_content_not_in_meta() {
        // Proves the single source of truth on the path a live call actually takes.
        let i = standard().await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let result = support::call(
            &t.ctx,
            "read_image",
            json!({ "file_path": "red.png" }),
            Some(agent_on(t, Some("vision-model"))),
        )
        .await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(
            result.meta(),
            Some(&json!({ "path": t.path_text("red.png") }))
        );
        let block = result
            .content()
            .iter()
            .find(|block| block.r#type == "image")
            .unwrap();
        assert_eq!(block.data["attachment"]["width"], 1);
        assert_eq!(block.data["attachment"]["height"], 1);
        assert!(
            block.data["attachment"]["attachmentId"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        t.kernel.shutdown().await;
    }
}

mod agent_options_fallback {
    use super::*;

    /// read-image.spec.ts «falls back to agent options when no request header exists yet»: the route
    /// comes from `agent.options` of the registered agent, the Session has no `request/header`.
    #[tokio::test]
    async fn falls_back_to_agent_options_when_no_request_header_exists_yet() {
        let i = standard().await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let agent = TestAgent::in_dir(
            &t.ctx,
            "image-agent-options",
            &t.workspace.to_string_lossy(),
        );
        agent.set_route("visual", "vision-model");
        assert!(agent.session.request_header().is_none());
        let result = read_image(t, json!({ "file_path": "red.png" }), Some(agent)).await;
        assert!(!result.is_error(), "{}", result.text());
        assert_eq!(image_ref(&result).media_type, ImageMediaType::Png);
        t.kernel.shutdown().await;
    }

    /// The request header outranks the agent options (`routed?.model ?? options.model`).
    #[tokio::test]
    async fn prefers_the_request_header_over_the_agent_options() {
        let i = standard().await;
        let t = &i.t;
        write(t, "red.png", &png_1x1());
        let agent = TestAgent::in_dir(
            &t.ctx,
            "image-agent-options-header",
            &t.workspace.to_string_lossy(),
        );
        agent.set_route("visual", "text-model");
        agent
            .session
            .append(
                "request/header",
                json!({
                    "header": { "config": { "provider": "visual", "model": "vision-model" } },
                    "reason": "initial",
                }),
                None,
            )
            .unwrap();
        let result = read_image(t, json!({ "file_path": "red.png" }), Some(agent)).await;
        assert!(!result.is_error(), "{}", result.text());
        t.kernel.shutdown().await;
    }
}
