//! DeepSeek Files API transport.

use crate::file_id::DeepSeekFileId;
use crate::messages_api::{MESSAGES_FILES_BETA, messages_api_root};
use hoh_attachment::ImageMediaType;
use hoh_error::{HarnessError, SharedError};
use hoh_llm::{BuiltinLlmFailureCode as Code, LlmError, LlmErrorOptions, attribution_headers};
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MIN_FILE_EXPIRY_SECONDS: u64 = 3_600;
pub const MAX_FILE_EXPIRY_SECONDS: u64 = 2_592_000;
pub const MAX_FILE_UPLOAD_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_STORED_FILE_COUNT: u64 = 10_000;
pub const MAX_STORED_FILE_BYTES: u64 = 25 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekFileObject {
    pub id: DeepSeekFileId,
    pub bytes: u64,
    pub created_at: u64,
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekFilePage {
    pub data: Vec<DeepSeekFileObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_id: Option<DeepSeekFileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_id: Option<DeepSeekFileId>,
    pub has_more: bool,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct DeepSeekFilesError {
    pub message: String,
    pub status: u16,
    pub detail: String,
}
pub fn is_files_quota_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        let files_error = error.downcast_ref::<DeepSeekFilesError>().or_else(|| {
            error
                .downcast_ref::<SharedError>()
                .and_then(SharedError::downcast_ref::<DeepSeekFilesError>)
        });
        if let Some(error) = files_error {
            let detail = error.detail.to_ascii_lowercase();
            return [
                "quota",
                "storage",
                "stored files",
                "file count",
                "too many files",
            ]
            .iter()
            .any(|text| detail.contains(text));
        }
        current = error.source();
    }
    false
}

#[derive(Clone)]
pub struct DeepSeekFilesClient {
    base_url: String,
    headers: IndexMap<String, String>,
    client: reqwest::Client,
}

impl DeepSeekFilesClient {
    pub fn new(base_url: &str, headers: IndexMap<String, String>) -> Result<Self, HarnessError> {
        let base_url =
            messages_api_root(base_url).map_err(|error| invalid("base URL", Some(error)))?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| invalid("HTTP client", Some(error)))?;
        Ok(Self {
            base_url,
            headers,
            client,
        })
    }
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.base_url, path))
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", MESSAGES_FILES_BETA);
        for (name, value) in attribution_headers(None)
            .unwrap_or_default()
            .iter()
            .chain(self.headers.iter())
        {
            request = request.header(name, value);
        }
        request
    }
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        signal: Option<&AbortSignal>,
    ) -> Result<reqwest::Response, HarnessError> {
        let operation = async {
            request.send().await.map_err(|error| {
                hoh_llm::LlmError::with_options(
                    format!("DeepSeek Files API request to {} failed", self.base_url),
                    Code::Transport,
                    LlmErrorOptions {
                        cause: Some(SharedError::new(error)),
                        ..Default::default()
                    },
                )
                .expect("files transport diagnostic is valid")
            })
        };
        let response = if let Some(signal) = signal {
            tokio::select! { biased; reason = signal.cancelled() => return Err(hoh_llm::LlmError::with_options("DeepSeek Files API request was aborted", Code::Aborted, LlmErrorOptions { cause: Some(reason), ..Default::default() }).expect("abort diagnostic is valid")), result = operation => result? }
        } else {
            operation.await?
        };
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let parsed: Value = match response_text(response, signal).await {
            Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Null),
            Err(error) if error.code() == Code::Aborted.as_str() => return Err(error),
            Err(_) => Value::Null,
        };
        let error = parsed.get("error").and_then(Value::as_object);
        let message = error
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .map_or_else(
                || format!("DeepSeek Files API error (HTTP {status})"),
                str::to_owned,
            );
        let detail = ["code", "type", "message"]
            .iter()
            .filter_map(|key| {
                error
                    .and_then(|value| value.get(*key))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join(" ");
        let source = DeepSeekFilesError {
            message: message.clone(),
            status,
            detail,
        };
        let code = if matches!(status, 401 | 403) {
            Code::Auth
        } else if status == 429 {
            Code::RateLimit
        } else if status >= 500 {
            Code::Server
        } else {
            Code::FilesApi
        };
        Err(LlmError::with_options(
            message,
            code,
            LlmErrorOptions {
                status: Some(status),
                cause: Some(SharedError::new(source)),
                ..Default::default()
            },
        )
        .expect("files failure is valid"))
    }
    async fn json(
        &self,
        response: reqwest::Response,
        operation: &str,
        signal: Option<&AbortSignal>,
    ) -> Result<Value, HarnessError> {
        let status = response.status().as_u16();
        let text = response_text(response, signal).await?;
        serde_json::from_str(&text).map_err(|error| {
            LlmError::with_options(
                format!(
                    "DeepSeek Files API returned invalid JSON for {operation} (HTTP {status})."
                ),
                Code::InvalidResponse,
                LlmErrorOptions {
                    status: Some(status),
                    cause: Some(SharedError::new(error)),
                    ..Default::default()
                },
            )
            .expect("files JSON failure is valid")
        })
    }
    pub async fn upload(
        &self,
        data: &[u8],
        media_type: ImageMediaType,
        filename: &str,
        expires_after_seconds: u64,
        signal: Option<&AbortSignal>,
    ) -> Result<DeepSeekFileObject, HarnessError> {
        validate_upload_options(data.len() as u64, expires_after_seconds)?;
        let boundary = "----deepseek-harness-files-boundary";
        let mut body = Vec::new();
        for (name, value) in [
            ("expires_after[anchor]", "created_at".to_owned()),
            ("expires_after[seconds]", expires_after_seconds.to_string()),
        ] {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        }
        let safe_name = filename.replace(['\r', '\n', '"'], "_");
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe_name}\"\r\nContent-Type: {media_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let request = self
            .request(reqwest::Method::POST, "/files")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body);
        let response = self.send(request, signal).await?;
        let value = self.json(response, "upload", signal).await?;
        let mut file = parse_file(&value, "upload")?;
        file.expires_at = Some(file.created_at + expires_after_seconds);
        Ok(file)
    }
    pub async fn list(
        &self,
        after: Option<&DeepSeekFileId>,
        limit: Option<u64>,
        signal: Option<&AbortSignal>,
    ) -> Result<DeepSeekFilePage, HarnessError> {
        let path = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            if let Some(after) = after {
                query.append_pair("after_id", after.as_str());
            }
            if let Some(limit) = limit {
                query.append_pair("limit", &limit.to_string());
            }
            format!("/files?{}", query.finish())
        };
        let response = self
            .send(self.request(reqwest::Method::GET, &path), signal)
            .await?;
        let value = self.json(response, "list", signal).await?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("list response.", None::<std::io::Error>))?;
        let data = object
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("list response.", None::<std::io::Error>))?
            .iter()
            .map(|value| parse_file(value, "list"))
            .collect::<Result<_, _>>()?;
        let id = |name| {
            object
                .get(name)
                .filter(|value| !value.is_null())
                .and_then(Value::as_str)
                .and_then(|value| DeepSeekFileId::new(value.to_owned()))
        };
        let has_more = object
            .get("has_more")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("list response.", None::<std::io::Error>))?;
        Ok(DeepSeekFilePage {
            data,
            first_id: id("first_id"),
            last_id: id("last_id"),
            has_more,
        })
    }
    pub async fn retrieve(
        &self,
        id: &DeepSeekFileId,
        signal: Option<&AbortSignal>,
    ) -> Result<DeepSeekFileObject, HarnessError> {
        let path = format!(
            "/files/{}",
            url::form_urlencoded::byte_serialize(id.as_str().as_bytes()).collect::<String>()
        );
        let response = self
            .send(self.request(reqwest::Method::GET, &path), signal)
            .await?;
        let value = self.json(response, "retrieve", signal).await?;
        parse_file(&value, "retrieve")
    }
    pub async fn delete(
        &self,
        id: &DeepSeekFileId,
        signal: Option<&AbortSignal>,
    ) -> Result<(), HarnessError> {
        let path = format!(
            "/files/{}",
            url::form_urlencoded::byte_serialize(id.as_str().as_bytes()).collect::<String>()
        );
        let response = self
            .send(self.request(reqwest::Method::DELETE, &path), signal)
            .await?;
        let value = self.json(response, "delete", signal).await?;
        if value.get("id").and_then(Value::as_str) != Some(id.as_str())
            || value.get("type").and_then(Value::as_str) != Some("file_deleted")
        {
            return Err(invalid("delete response.", None::<std::io::Error>));
        }
        Ok(())
    }
}

async fn response_text(
    response: reqwest::Response,
    signal: Option<&AbortSignal>,
) -> Result<String, HarnessError> {
    let read = response.text();
    let result = if let Some(signal) = signal {
        tokio::select! {
            biased;
            reason = signal.cancelled() => {
                return Err(LlmError::with_options(
                    "DeepSeek Files API request was aborted",
                    Code::Aborted,
                    LlmErrorOptions {
                        cause: Some(reason),
                        ..Default::default()
                    },
                )
                .expect("abort diagnostic is valid"));
            }
            result = read => result,
        }
    } else {
        read.await
    };
    result.map_err(|error| {
        LlmError::with_options(
            "DeepSeek Files API response body failed",
            Code::Transport,
            LlmErrorOptions {
                cause: Some(SharedError::new(error)),
                ..Default::default()
            },
        )
        .expect("files body transport diagnostic is valid")
    })
}

fn invalid(operation: &str, cause: Option<impl Into<hoh_error::BoxError>>) -> HarnessError {
    let error = LlmError::new(
        format!("DeepSeek Files API returned an invalid {operation} response."),
        Code::InvalidResponse,
    )
    .expect("invalid response diagnostic is valid");
    match cause {
        Some(cause) => error.with_cause(cause),
        None => error,
    }
}

fn validate_upload_options(bytes: u64, expires_after_seconds: u64) -> Result<(), HarnessError> {
    if bytes > MAX_FILE_UPLOAD_BYTES {
        return Err(LlmError::new(
            "DeepSeek Files API upload exceeds 128 MiB.",
            Code::InvalidRequest,
        )
        .expect("upload limit diagnostic is valid"));
    }
    if !(MIN_FILE_EXPIRY_SECONDS..=MAX_FILE_EXPIRY_SECONDS).contains(&expires_after_seconds) {
        return Err(LlmError::new(
            "DeepSeek file expiry must be between 3600 and 2592000 seconds.",
            Code::InvalidRequest,
        )
        .expect("expiry diagnostic is valid"));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use futures_util::stream;
    use hoh_timeout::AbortController;
    use serde_json::json;
    use std::time::Duration;

    #[tokio::test]
    async fn response_body_read_observes_abort_after_headers() {
        let response: reqwest::Response = http::Response::builder()
            .status(200)
            .body(reqwest::Body::wrap_stream(stream::pending::<
                Result<&'static [u8], std::io::Error>,
            >()))
            .unwrap()
            .into();
        let client = DeepSeekFilesClient::new("https://example.test", IndexMap::new()).unwrap();
        let controller = AbortController::new();
        let signal = controller.signal();
        let read =
            tokio::spawn(async move { client.json(response, "retrieve", Some(&signal)).await });
        tokio::task::yield_now().await;
        controller.abort();
        let error = tokio::time::timeout(Duration::from_secs(1), read)
            .await
            .expect("body read stops promptly")
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code(), "ABORTED");
    }

    #[tokio::test]
    async fn response_body_transport_failure_is_not_reclassified_as_invalid_json() {
        let response: reqwest::Response = http::Response::builder()
            .status(200)
            .body(reqwest::Body::wrap_stream(stream::once(async {
                Err::<&'static [u8], _>(std::io::Error::other("body transport failed"))
            })))
            .unwrap()
            .into();
        let client = DeepSeekFilesClient::new("https://example.test", IndexMap::new()).unwrap();
        let error = client.json(response, "retrieve", None).await.unwrap_err();
        assert_eq!(error.code(), "TRANSPORT");
        assert!(error.cause().is_some_and(|cause| {
            SharedError::of(cause)
                .is_some_and(|cause| cause.downcast_ref::<reqwest::Error>().is_some())
        }));
    }

    #[test]
    fn upload_limits_preserve_source_codes_and_messages() {
        let error = validate_upload_options(MAX_FILE_UPLOAD_BYTES + 1, MIN_FILE_EXPIRY_SECONDS)
            .unwrap_err();
        assert_eq!(error.code(), "INVALID_REQUEST");
        assert_eq!(
            error.message(),
            "DeepSeek Files API upload exceeds 128 MiB."
        );
        let error = validate_upload_options(0, MIN_FILE_EXPIRY_SECONDS - 1).unwrap_err();
        assert_eq!(error.code(), "INVALID_REQUEST");
        assert_eq!(
            error.message(),
            "DeepSeek file expiry must be between 3600 and 2592000 seconds."
        );
    }

    #[test]
    fn file_objects_require_every_messages_field() {
        let valid = json!({
            "id":"file-one",
            "type":"file",
            "mime_type":"image/png",
            "size_bytes":3,
            "created_at":"2023-11-14T22:13:20Z",
            "filename":"image.png"
        });
        let parsed = parse_file(&valid, "retrieve").unwrap();
        assert_eq!(parsed.id.as_str(), "file-one");
        assert_eq!(parsed.bytes, 3);
        assert_eq!(parsed.created_at, 1_700_000_000);

        for field in [
            "id",
            "type",
            "mime_type",
            "size_bytes",
            "created_at",
            "filename",
        ] {
            let mut invalid = valid.clone();
            invalid.as_object_mut().unwrap().remove(field);
            let error = parse_file(&invalid, "retrieve").unwrap_err();
            assert_eq!(error.code(), "INVALID_RESPONSE", "{field}");
            assert_eq!(
                error.message(),
                "DeepSeek Files API returned an invalid retrieve response."
            );
        }
    }

    #[test]
    fn quota_detection_requires_a_typed_files_error() {
        let quota = DeepSeekFilesError {
            message: "full".into(),
            status: 400,
            detail: "file_quota user storage quota exceeded".into(),
        };
        assert!(is_files_quota_error(&quota));
        assert!(!is_files_quota_error(&std::io::Error::other(
            "storage quota"
        )));
        let wrapped = SharedError::new(quota);
        assert!(is_files_quota_error(&wrapped));
    }
}

fn parse_file(value: &Value, operation: &str) -> Result<DeepSeekFileObject, HarnessError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid(operation, None::<std::io::Error>))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .and_then(|value| DeepSeekFileId::new(value.to_owned()));
    let bytes = object.get("size_bytes").and_then(Value::as_u64);
    let filename = object
        .get("filename")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let created_at = object
        .get("created_at")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<jiff::Timestamp>().ok())
        .and_then(|value| u64::try_from(value.as_second()).ok());
    if object.get("type").and_then(Value::as_str) != Some("file")
        || object.get("mime_type").and_then(Value::as_str).is_none()
    {
        return Err(invalid(operation, None::<std::io::Error>));
    }
    Ok(DeepSeekFileObject {
        id: id.ok_or_else(|| invalid(operation, None::<std::io::Error>))?,
        bytes: bytes.ok_or_else(|| invalid(operation, None::<std::io::Error>))?,
        created_at: created_at.ok_or_else(|| invalid(operation, None::<std::io::Error>))?,
        filename: filename
            .ok_or_else(|| invalid(operation, None::<std::io::Error>))?
            .into(),
        expires_at: None,
    })
}
