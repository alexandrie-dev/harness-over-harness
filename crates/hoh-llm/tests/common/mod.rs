#![allow(dead_code)]
use hoh_llm::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
pub fn decode<T: DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}
pub fn value(v: impl Serialize) -> Value {
    let value = serde_json::to_value(v).unwrap();
    hoh_util_values::js_json_parse(&hoh_util_values::js_json_stringify(&value)).unwrap()
}
pub fn chunk(v: Value) -> StreamChunk {
    decode(v)
}
pub fn block(v: Value) -> ContentBlock {
    decode(v)
}
pub fn text(s: &str) -> StreamChunk {
    StreamChunk::TextDelta {
        index: 0,
        text: s.into(),
        wire: Default::default(),
    }
}
pub fn script() -> Vec<StreamChunk> {
    vec![
        chunk(json!({"type":"block-start","index":0,"blockType":"text"})),
        text("hi"),
        chunk(json!({"type":"block-end","index":0,"block":{"type":"text","text":"hi"}})),
        StreamChunk::finish(FinishReason::new("stop")),
    ]
}
pub fn config() -> LlmCallConfig {
    LlmCallConfig {
        provider: "route".into(),
        model: "model".into(),
        ..Default::default()
    }
}
pub fn options() -> GenerateOptions {
    GenerateOptions {
        provider: "route".into(),
        model: "model".into(),
        ..Default::default()
    }
}
pub fn metadata() -> LlmResolvedModelInfo {
    decode(json!({"provider":"route","id":"model","name":"Model"}))
}
pub fn user(content: Vec<ContentBlock>) -> RequestMessage {
    create_user_message(NewUserMessage {
        content,
        source: MessageSource::new("test"),
    })
    .into()
}
pub fn developer(content: Vec<ContentBlock>) -> RequestMessage {
    create_developer_message(NewDeveloperMessage {
        content,
        source: MessageSource::new("test"),
    })
    .into()
}
pub fn tool(name: &str) -> ToolSchema {
    decode(json!({"name":name,"description":name,"parameters":{}}))
}
pub fn image(bytes: u64, offloaded: bool) -> ContentBlock {
    let mut v = json!({"type":"image","attachment":{"attachmentId":format!("sha256:{}","a".repeat(64)),"mediaType":"image/png","bytes":bytes,"width":1,"height":1}});
    if offloaded {
        v["offloaded"] = json!(true);
    }
    block(v)
}
pub fn file() -> ContentBlock {
    block(
        json!({"type":"file","attachment":{"attachmentId":format!("sha256:{}","ab".repeat(32)),"name":"notes.pdf","bytes":42}}),
    )
}
pub fn update(message: &RequestMessage, additions: Vec<ToolSchema>) -> ToolHistoryUpdate {
    ToolHistoryUpdate {
        message_id: message.id().unwrap().clone(),
        additions,
    }
}
pub async fn collect(stream: LlmStream) -> Result<Vec<StreamChunk>, hoh_error::BoxError> {
    use futures_util::TryStreamExt;
    stream.try_collect().await
}
#[derive(Default)]
pub struct RecordingAdapter {
    pub seen: Mutex<Vec<GenerateOptions>>,
    pub metadata: Mutex<Option<LlmResolvedModelInfo>>,
    pub models: Mutex<Vec<LlmModelInfo>>,
    pub policy: Mutex<Option<ResolvedRetryPolicy>>,
}
impl LlmAdapter for RecordingAdapter {
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, hoh_error::BoxError> {
        self.seen.lock().unwrap().push(options);
        Ok(LlmStream::chunks(script()))
    }
    fn list_models<'a>(&'a self, _p: &'a str) -> LlmFuture<'a, Vec<LlmModelInfo>> {
        let models = self.models.lock().unwrap().clone();
        Box::pin(async move { Ok(models) })
    }
    fn provider_retry_policy(
        &self,
        _p: &str,
    ) -> Result<Option<ResolvedRetryPolicy>, hoh_error::BoxError> {
        Ok(self.policy.lock().unwrap().clone())
    }
    fn resolve_model<'a>(
        &'a self,
        p: &'a str,
        m: &'a str,
        _s: Option<&'a hoh_timeout::AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        let info = self
            .metadata
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| decode(json!({"provider":p,"id":m,"name":m})));
        Box::pin(async move { Ok(info) })
    }
}
pub async fn setup() -> (
    hoh_kernel::Kernel,
    hoh_kernel::Context,
    LlmRuntime,
    Arc<RecordingAdapter>,
) {
    let kernel = hoh_kernel::Kernel::start();
    let ctx = kernel.root();
    let runtime = LlmRuntime::new();
    let adapter = Arc::new(RecordingAdapter::default());
    runtime
        .register_adapter(&ctx, vec!["route".into()], adapter.clone())
        .unwrap();
    (kernel, ctx, runtime, adapter)
}

pub struct TestAttachments {
    pub host_path: Result<Option<String>, hoh_error::HarnessError>,
    pub limits: hoh_attachment::ImageAttachmentLimits,
}
impl TestAttachments {
    pub fn new(host_path: Result<Option<String>, hoh_error::HarnessError>) -> Self {
        Self {
            host_path,
            limits: decode(
                json!({"maxImageBytes":100,"maxImagesPerMessage":10,"maxMessageImageBytes":1000,"maxImagePixels":100,"maxImageDimension":10,"mediaTypes":["image/png"]}),
            ),
        }
    }
}
impl hoh_attachment::AttachmentStore for TestAttachments {
    fn image_limits(&self) -> &hoh_attachment::ImageAttachmentLimits {
        &self.limits
    }
    fn validate_image<'a>(
        &'a self,
        _input: &'a hoh_attachment::SaveImageAttachment,
    ) -> LlmFuture<'a, ()> {
        Box::pin(async { panic!("unexpected image validation") })
    }
    fn save_image<'a>(
        &'a self,
        _input: &'a hoh_attachment::SaveImageAttachment,
    ) -> LlmFuture<'a, hoh_attachment::ImageAttachmentRef> {
        Box::pin(async { panic!("unexpected image write") })
    }
    fn read_image<'a>(
        &'a self,
        _reference: &'a hoh_attachment::ImageAttachmentRef,
        _signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> LlmFuture<'a, hoh_attachment::StoredImageAttachment> {
        Box::pin(async { panic!("unexpected image read") })
    }
    fn image_host_path(
        &self,
        _reference: &hoh_attachment::ImageAttachmentRef,
    ) -> Result<Option<String>, hoh_error::HarnessError> {
        self.host_path.clone()
    }
    fn file_host_path(
        &self,
        _reference: &hoh_attachment::FileAttachmentRef,
    ) -> Result<Option<String>, hoh_error::HarnessError> {
        self.host_path.clone()
    }
}
