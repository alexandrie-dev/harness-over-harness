//! Bounded retrieval. Address selection is the transport's normal resolver policy.
use crate::{DEFAULT_USER_AGENT, policy::*, transport_failure::*};
use async_compression::tokio::bufread::{
    BrotliDecoder, DeflateDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder,
};
use futures_util::TryStreamExt;
use hoh_error::{BoxError, HarnessError, node_error_code};
use hoh_timeout::{AbortError, AbortSignal, deadline, timeout_of};
use hoh_web::{
    WebError, WebFetchBody, WebFetchProvider, WebFetchRequest, WebFetchResult, WebFuture,
    WebProviderInfo,
};
use reqwest::{Client, ClientBuilder, Response};
use serde::{Deserialize, Serialize};
use std::{error::Error, pin::Pin, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio_util::io::StreamReader;
use url::Url;

pub const LOCAL_FETCH_PROVIDER_ID: &str = "http";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpFetchLimits {
    pub max_response_bytes: u64,
    pub max_body_chars: u64,
    pub timeout_ms: f64,
    pub max_redirects: u64,
    pub user_agent: String,
}
impl Default for HttpFetchLimits {
    fn default() -> Self {
        Self {
            max_response_bytes: 5_000_000,
            max_body_chars: 100_000,
            timeout_ms: 30_000.0,
            max_redirects: 5,
            user_agent: DEFAULT_USER_AGENT.into(),
        }
    }
}

/// Object-safe request boundary, also used to replay Node transport failures without DNS/network.
pub trait HttpFetchTransport: Send + Sync {
    fn get<'a>(&'a self, url: &'a Url, user_agent: &'a str) -> WebFuture<'a, Response>;
}
struct ReqwestTransport(Client);
impl HttpFetchTransport for ReqwestTransport {
    fn get<'a>(&'a self, url: &'a Url, user_agent: &'a str) -> WebFuture<'a, Response> {
        Box::pin(async move {
            Ok(self
                .0
                .get(url.clone())
                .header("user-agent", user_agent_value(user_agent)?)
                .header("accept-language", "*")
                .header("sec-fetch-mode", "cors")
                .header(
                    "accept-encoding",
                    if url.scheme() == "https" {
                        "br, gzip, deflate, zstd"
                    } else {
                        "gzip, deflate"
                    },
                )
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,text/*;q=0.9,application/json;q=0.8",
                )
                .send()
                .await?)
        })
    }
}
fn user_agent_value(value: &str) -> Result<reqwest::header::HeaderValue, BoxError> {
    let mut bytes = Vec::new();
    for (index, unit) in value.encode_utf16().enumerate() {
        if unit > 255 {
            return Err(TransportError::new(format!("Cannot convert argument to a ByteString because the character at index {index} has a value of {unit} which is greater than 255."), None).with_name("TypeError").into());
        }
        bytes.push(unit as u8);
    }
    let is_http_space = |byte: &u8| matches!(*byte, b'\t' | b'\n' | b'\r' | b' ');
    let start = bytes
        .iter()
        .position(|byte| !is_http_space(byte))
        .unwrap_or(bytes.len());
    let bytes = &bytes[start..];
    let end = bytes
        .iter()
        .rposition(|byte| !is_http_space(byte))
        .map_or(0, |index| index + 1);
    let bytes = &bytes[..end];
    Ok(reqwest::header::HeaderValue::from_bytes(bytes)?)
}

#[derive(Clone)]
pub struct HttpFetchProvider {
    limits: HttpFetchLimits,
    transport: Arc<dyn HttpFetchTransport>,
    byte_limit_text: String,
    redirect_limit_text: String,
}
impl HttpFetchProvider {
    pub fn new(limits: HttpFetchLimits) -> Result<Self, BoxError> {
        Self::with_client_builder(limits, Client::builder().no_proxy())
    }
    pub fn with_client_builder(
        limits: HttpFetchLimits,
        builder: ClientBuilder,
    ) -> Result<Self, BoxError> {
        let client = builder
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self::with_transport(
            limits,
            Arc::new(ReqwestTransport(client)),
        ))
    }
    pub fn with_transport(limits: HttpFetchLimits, transport: Arc<dyn HttpFetchTransport>) -> Self {
        let byte_limit_text = limits.max_response_bytes.to_string();
        let redirect_limit_text = limits.max_redirects.to_string();
        Self {
            limits,
            transport,
            byte_limit_text,
            redirect_limit_text,
        }
    }
    pub(crate) fn with_limit_diagnostics(mut self, bytes: String, redirects: String) -> Self {
        self.byte_limit_text = bytes;
        self.redirect_limit_text = redirects;
        self
    }
    pub async fn fetch(
        &self,
        request: WebFetchRequest,
        signal: Option<&AbortSignal>,
    ) -> Result<WebFetchResult, BoxError> {
        if signal.is_some_and(AbortSignal::aborted) {
            return Err(WebError::new("web fetch aborted", "WEB_ABORTED").into());
        }
        let deadline = deadline(signal, self.limits.timeout_ms, "WEB_FETCH_TIMEOUT")?;
        self.follow_and_read(&request.url, &deadline.signal).await
    }
    async fn follow_and_read(
        &self,
        initial_url: &str,
        signal: &AbortSignal,
    ) -> Result<WebFetchResult, BoxError> {
        let mut current = validate_fetch_url(initial_url)?;
        let mut followed = 0_u64;
        loop {
            let response = tokio::select! {
                biased;
                reason=signal.cancelled()=>return Err(abort_failure(signal,reason.into()).into()),
                result=self.transport.get(&current,&self.limits.user_agent)=>result.map_err(|e|translate_failure(e,&current,signal))?,
            };
            let status = response.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                if followed >= self.limits.max_redirects {
                    return Err(WebError::new(
                        format!(
                            "exceeded the maximum of {} redirects",
                            self.redirect_limit_text
                        ),
                        "WEB_REDIRECT_BLOCKED",
                    )
                    .into());
                }
                let Some(location) = header(&response, "location") else {
                    return Err(WebError::new(
                        format!("redirect response (HTTP {status}) without a Location header"),
                        "WEB_PROVIDER_ERROR",
                    )
                    .into());
                };
                let target = current.join(&location).map_err(|_| {
                    WebError::new(
                        format!("invalid redirect Location \"{location}\""),
                        "WEB_PROVIDER_ERROR",
                    )
                    .with_cause(invalid_url_cause())
                })?;
                let target = validate_fetch_url(target.as_str())?;
                if !is_same_origin(&target, &current) {
                    return Err(WebError::new(format!("cross-origin redirect to {} is not followed automatically; retry against that URL directly",target.origin().ascii_serialization()),"WEB_REDIRECT_BLOCKED").into());
                }
                drop(response); // Dropping an unfinished reqwest body cancels that response stream.
                current = target;
                followed += 1;
                continue;
            }
            return self.read_body(response, &current, signal).await;
        }
    }
    async fn read_body(
        &self,
        response: Response,
        url: &Url,
        signal: &AbortSignal,
    ) -> Result<WebFetchResult, BoxError> {
        let content_type = header(&response, "content-type");
        let kind = classify_content_type(content_type.as_deref()).ok_or_else(|| {
            WebError::new(
                format!(
                    "unsupported content type \"{}\"",
                    content_type.as_deref().unwrap_or("unknown")
                ),
                "WEB_UNSUPPORTED_CONTENT_TYPE",
            )
        })?;
        let decoder = decoder_for_charset(parse_charset(content_type.as_deref()).as_deref())?;
        let status_code = u64::from(response.status().as_u16());
        let (bytes, truncated_by_bytes) = self.read_capped(response, url, signal).await?;
        let decoded = decoder.decode(&bytes, truncated_by_bytes)?;
        let truncated_by_chars = decoded.encode_utf16().count() as u64 > self.limits.max_body_chars;
        let content = if truncated_by_chars {
            truncate_chars(&decoded, self.limits.max_body_chars)
        } else {
            decoded
        };
        let body = match kind {
            FetchableKind::Html => WebFetchBody::Html { content },
            FetchableKind::Text => WebFetchBody::Text { content },
        };
        Ok(WebFetchResult {
            url: url.to_string(),
            status_code,
            body,
            truncated: truncated_by_bytes || truncated_by_chars,
        })
    }
    async fn read_capped(
        &self,
        response: Response,
        url: &Url,
        signal: &AbortSignal,
    ) -> Result<(Vec<u8>, bool), BoxError> {
        if let Some(declared) = header(&response, "content-length") {
            let length = declared.parse::<f64>().ok();
            if length.is_some_and(|n| n.is_finite() && n > self.limits.max_response_bytes as f64) {
                return Err(WebError::new(
                    format!(
                        "response exceeds the maximum of {} bytes",
                        self.byte_limit_text
                    ),
                    "WEB_FETCH_TOO_LARGE",
                )
                .into());
            }
        }
        // Fetch exposes a null body for these status codes (after the declared-size check).
        if matches!(response.status().as_u16(), 204 | 205 | 304) {
            return Ok((Vec::new(), false));
        }
        let encoding = header(&response, "content-encoding")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let codings: Vec<_> = encoding.split(',').map(hoh_util_values::js_trim).collect();
        let decode = codings
            .iter()
            .all(|coding| matches!(*coding, "gzip" | "x-gzip" | "deflate" | "br" | "zstd"));
        let stream = response.bytes_stream().map_err(std::io::Error::other);
        let mut reader: Pin<Box<dyn AsyncRead + Send>> = Box::pin(StreamReader::new(stream));
        if decode {
            for coding in codings.into_iter().rev() {
                let mut buffered = BufReader::new(reader);
                reader = match coding {
                    "gzip" | "x-gzip" => {
                        let mut decoder = GzipDecoder::new(buffered);
                        decoder.multiple_members(true);
                        Box::pin(FetchGzip(decoder))
                    }
                    "br" => Box::pin(BrotliDecoder::new(buffered)),
                    "zstd" => {
                        let mut decoder = ZstdDecoder::new(buffered);
                        decoder.multiple_members(true);
                        Box::pin(decoder)
                    }
                    "deflate" => {
                        let first = tokio::select! {
                            biased;
                            _ = signal.cancelled() => return Err(abort_failure(signal, AbortError::new("The operation was aborted.").into()).into()),
                            bytes = buffered.fill_buf() => bytes.map_err(|e|translate_read_failure(e,url,signal))?.first().copied(),
                        };
                        // Node's Inflate stream accepts a zlib wrapper or raw deflate, sniffing CMF.
                        if first.is_none() {
                            Box::pin(buffered)
                        } else if first.is_some_and(|byte| byte & 0x0f == 8) {
                            Box::pin(ZlibDecoder::new(buffered))
                        } else {
                            Box::pin(DeflateDecoder::new(buffered))
                        }
                    }
                    // Every coding was checked before constructing the decoder chain.
                    _ => unreachable!("all content codings were checked above"),
                };
            }
        }
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let read = tokio::select! {
                biased;
                _reason=signal.cancelled()=>return Err(abort_failure(signal,AbortError::new("The operation was aborted.").into()).into()),
                count=reader.read(&mut chunk)=>count.map_err(|e|translate_read_failure(e,url,signal))?,
            };
            if read == 0 {
                return Ok((bytes, false));
            }
            let chunk = &chunk[..read];
            let remaining = self.limits.max_response_bytes - bytes.len() as u64;
            if chunk.len() as u64 > remaining {
                // The comparison proves remaining fits usize and is within this chunk.
                bytes.extend_from_slice(&chunk[..remaining as usize]);
                return Ok((bytes, true));
            }
            bytes.extend_from_slice(chunk);
        }
    }
}
// Fetch's gunzip uses Z_SYNC_FLUSH at EOF, accepting a missing trailer or unfinished member.
struct FetchGzip(GzipDecoder<BufReader<Pin<Box<dyn AsyncRead + Send>>>>);
impl AsyncRead for FetchGzip {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match Pin::new(&mut self.get_mut().0).poll_read(cx, buf) {
            std::task::Poll::Ready(Err(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof
                    && error.get_ref().is_none() =>
            {
                std::task::Poll::Ready(Ok(()))
            }
            result => result,
        }
    }
}
fn translate_read_failure(error: std::io::Error, url: &Url, signal: &AbortSignal) -> HarnessError {
    let error: BoxError = if error.get_ref().is_some() {
        // get_ref proves the custom payload exists, retaining the original reqwest error.
        error.into_inner().expect("custom I/O error payload")
    } else {
        Box::new(error)
    };
    translate_failure(error, url, signal)
}
impl WebFetchProvider for HttpFetchProvider {
    fn info(&self) -> WebProviderInfo {
        // Ordered source translations, retained without introducing a dependency on their owner.
        let description=serde_json::from_value(serde_json::json!({
            "en":"Free, and can reach intranet and local addresses. Does not support pages that need JavaScript or sign-in, or PDFs.",
            "zh":"免费，可访问内网和本机。不支持需要 JavaScript 或登录的页面，也不支持 PDF。",
            "ja":"無料で、イントラネットやローカルのアドレスにもアクセスできます。JavaScript やログインが必要なページ、PDF には対応しません。",
            "es":"Gratis y puede acceder a direcciones de la intranet y locales. No admite páginas que requieran JavaScript o inicio de sesión, ni PDF.",
            "fr":"Gratuit, et accède aux adresses intranet et locales. Ne prend pas en charge les pages nécessitant JavaScript ou une connexion, ni les PDF.",
            "de":"Kostenlos und erreicht auch Intranet- und lokale Adressen. Keine Seiten, die JavaScript oder eine Anmeldung erfordern, und keine PDFs.",
            "ru":"Бесплатно, доступны адреса интрасети и локальные адреса. Не поддерживает страницы, требующие JavaScript или входа, а также PDF.",
            "pt":"Gratuito e acessa endereços da intranet e locais. Não oferece suporte a páginas que exigem JavaScript ou login, nem a PDFs."
        })).expect("source picker descriptions are string translations");
        WebProviderInfo {
            id: LOCAL_FETCH_PROVIDER_ID.into(),
            title: None,
            description: Some(description),
        }
    }
    fn available(&self) -> Result<bool, BoxError> {
        Ok(true)
    }
    fn fetch<'a>(
        &'a self,
        request: WebFetchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebFetchResult> {
        Box::pin(HttpFetchProvider::fetch(self, request, signal))
    }
}
fn header(response: &Response, name: &str) -> Option<String> {
    let values: Vec<String> = response
        .headers()
        .get_all(name)
        .iter()
        // Fetch Headers exposes HTTP field values as ByteString (Latin-1 code units).
        .map(|v| v.as_bytes().iter().map(|byte| char::from(*byte)).collect())
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}
fn truncate_chars(text: &str, limit: u64) -> String {
    let mut units = 0;
    let mut out = String::new();
    for ch in text.chars() {
        if units + ch.len_utf16() as u64 > limit {
            break;
        }
        units += ch.len_utf16() as u64;
        out.push(ch);
    }
    out
}
fn abort_failure(signal: &AbortSignal, error: BoxError) -> HarnessError {
    let reason = signal.reason();
    if timeout_of(reason.as_ref(), Some("WEB_FETCH_TIMEOUT")).is_some() {
        // Matching timeout_of proves this is the shared reason carried by the deadline.
        return WebError::new("web fetch timed out", "WEB_FETCH_TIMEOUT")
            .with_cause(reason.expect("matching timeout reason"));
    }
    WebError::new("web fetch aborted", "WEB_ABORTED").with_cause(error)
}
fn translate_failure(error: BoxError, url: &Url, signal: &AbortSignal) -> HarnessError {
    if signal.aborted() {
        return abort_failure(signal, error);
    }
    let failure = match error.downcast_ref::<reqwest::Error>() {
        Some(error) => classify_transport_failure(&node_transport_error(error, url)),
        None => classify_transport_failure(error.as_ref()),
    };
    WebError::new(
        format!("web fetch failed: {}", failure.message),
        failure.code,
    )
    .with_cause(error)
}
fn node_transport_error(error: &reqwest::Error, url: &Url) -> TransportError {
    let mut source: Option<&(dyn Error + 'static)> = Some(error);
    let mut messages = Vec::new();
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>()
            && let Some(code) = node_error_code(io)
        {
            let address = match url.host() {
                Some(url::Host::Ipv6(ip)) => {
                    format!("{ip}:{}", url.port_or_known_default().unwrap_or(0))
                }
                _ => format!(
                    "{}:{}",
                    url.host_str().unwrap_or_default(),
                    url.port_or_known_default().unwrap_or(0)
                ),
            };
            let message = if error.is_connect() {
                format!("connect {code} {address}")
            } else if code == "EPIPE" {
                format!("write {code}")
            } else {
                format!("read {code}")
            };
            return TransportError::new(message, Some(code));
        }
        messages.push(current.to_string());
        source = current.source();
    }
    let detail = messages.join(": ");
    if detail.contains("invalid peer certificate") || detail.contains("invalid certificate") {
        let (code, message) = if detail.contains("expired") || detail.contains("Expired") {
            ("CERT_HAS_EXPIRED", "certificate has expired")
        } else if detail.contains("not valid yet") || detail.contains("NotValidYet") {
            ("CERT_NOT_YET_VALID", "certificate is not yet valid")
        } else if detail.contains("not valid for name") || detail.contains("NotValidForName") {
            (
                "ERR_TLS_CERT_ALTNAME_INVALID",
                "Hostname/IP does not match certificate's altnames",
            )
        } else if detail.contains("Revoked") {
            ("CERT_REVOKED", "certificate revoked")
        } else if detail.contains("UnknownIssuer") {
            (
                "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
                "unable to get local issuer certificate",
            )
        } else if detail.contains("BadSignature") {
            ("CERT_SIGNATURE_FAILURE", "certificate signature failure")
        } else {
            ("CERT_UNTRUSTED", "certificate is not trusted")
        };
        return TransportError::new(message, Some(code));
    }
    if error.is_timeout() {
        return if error.is_connect() {
            TransportError::new("Connect Timeout Error", Some("UND_ERR_CONNECT_TIMEOUT"))
        } else if error.is_body() || error.is_decode() {
            TransportError::new("Body Timeout Error", Some("UND_ERR_BODY_TIMEOUT"))
        } else {
            TransportError::new("Headers Timeout Error", Some("UND_ERR_HEADERS_TIMEOUT"))
        };
    }
    if detail.contains("dns error") || detail.contains("failed to lookup address") {
        let code = if detail.contains("Temporary failure") {
            "EAI_AGAIN"
        } else {
            "ENOTFOUND"
        };
        return TransportError::new(
            format!("getaddrinfo {code} {}", url.host_str().unwrap_or_default()),
            Some(code),
        );
    }
    if detail.contains("Invalid chunk size line: missing size digit")
        || detail.contains("Invalid chunk size line: Invalid Size")
    {
        return TransportError::new(
            "Response does not match the HTTP/1.1 protocol (Invalid character in chunk size)",
            Some("HPE_INVALID_CHUNK_SIZE"),
        );
    }
    if detail.contains("connection closed")
        || detail.contains("incomplete message")
        || detail.contains("end of file before message length reached")
        || detail.contains("unexpected EOF")
    {
        return TransportError::new("other side closed", Some("UND_ERR_SOCKET"));
    }
    TransportError::new(
        messages
            .last()
            .cloned()
            .unwrap_or_else(|| "fetch failed".into()),
        None,
    )
}
