//! Merge plugin-contributed top-level request fields and commit after HTTP acceptance.

use hoh_deepseek_llm_api_extensions::{
    DeepSeekLlmApiExtensionRequest, PreparedDeepSeekLlmApiExtensions,
};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_llm::{BuiltinLlmFailureCode as Code, LlmError, LlmErrorOptions};
use hoh_util_values::js_json_stringify;
use serde_json::{Map, Value};

pub(crate) fn stringify_payload(value: &Value) -> String {
    js_json_stringify(value)
}

pub struct PreparedRequestExtensions {
    pub payload: String,
    extensions: Option<PreparedDeepSeekLlmApiExtensions>,
}
impl PreparedRequestExtensions {
    pub async fn accept(self) -> Result<(), SharedError> {
        match self.extensions {
            Some(value) => value.accept().await,
            None => Ok(()),
        }
    }
}

pub async fn prepare_request_extensions<F, Fut>(
    body: Map<String, Value>,
    request: DeepSeekLlmApiExtensionRequest,
    prepare: F,
    _on_omitted: impl FnMut(&[String], &serde_json::Error),
) -> Result<PreparedRequestExtensions, HarnessError>
where
    F: FnOnce(DeepSeekLlmApiExtensionRequest) -> Fut,
    Fut: std::future::Future<Output = Result<PreparedDeepSeekLlmApiExtensions, BoxError>>,
{
    let extensions = prepare(request).await.map_err(|error| {
        LlmError::with_options(
            format!("DeepSeek request extension preparation failed: {error}"),
            Code::RequestExtension,
            LlmErrorOptions {
                cause: Some(SharedError::new(error)),
                ..Default::default()
            },
        )
        .expect("extension diagnostic is valid")
    })?;
    let fields: Vec<_> = extensions.fields().keys().cloned().collect();
    for field in &fields {
        if body.contains_key(field) {
            return Err(LlmError::new(
                format!(
                    "DeepSeek request extension field {} collides with the base request",
                    serde_json::to_string(field).expect("field is JSON")
                ),
                Code::RequestExtension,
            )
            .expect("collision diagnostic is valid"));
        }
    }
    let mut merged = body.clone();
    merged.extend(extensions.fields().clone());
    let payload = stringify_payload(&Value::Object(merged));
    Ok(PreparedRequestExtensions {
        payload,
        extensions: Some(extensions),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_deepseek_llm_api_extensions::{
        DeepSeekLlmApiExtensionRegistry, PreparedDeepSeekLlmApiExtension,
    };
    use hoh_kernel::Kernel;
    use hoh_timeout::AbortController;
    use serde_json::json;

    #[test]
    fn base_request_payload_uses_json_stringify_number_format() {
        let body = json!({"temperature": 1.0});
        assert_eq!(stringify_payload(&body), r#"{"temperature":1}"#);
    }

    #[tokio::test]
    async fn extension_request_payload_uses_json_stringify_number_format() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let registry = DeepSeekLlmApiExtensionRegistry::new(ctx.clone());
        registry
            .register(&ctx, "extension", |_| async {
                Ok::<_, BoxError>(Some(PreparedDeepSeekLlmApiExtension {
                    value: json!(1.0),
                    accept: None,
                }))
            })
            .unwrap();
        let body = json!({"temperature": 1.0}).as_object().unwrap().clone();
        let request = DeepSeekLlmApiExtensionRequest {
            body: body.clone(),
            session_id: None,
            purpose: None,
            signal: AbortController::new().signal(),
        };
        let prepared = prepare_request_extensions(
            body,
            request,
            move |request| registry.prepare(request),
            |_, _| panic!("JSON values are representable"),
        )
        .await
        .unwrap();
        assert_eq!(prepared.payload, r#"{"temperature":1,"extension":1}"#);
        kernel.shutdown().await;
    }
}
