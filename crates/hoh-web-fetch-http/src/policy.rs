//! The provider's network-independent URL and representation policy.
use encoding_rs::{Encoding, UTF_8};
use hoh_error::HarnessError;
use hoh_util_values::{JS_WHITE_SPACE_SET, is_js_white_space, js_trim};
use hoh_web::WebError;
use url::Url;

#[derive(Debug, thiserror::Error)]
enum DecoderFailure {
    #[error("The encoded data was not valid for encoding utf-8")]
    InvalidUtf8,
    #[error("The \"{0}\" encoding is not supported")]
    UnsupportedEncoding(String),
}
pub(crate) fn invalid_url_cause() -> crate::transport_failure::TransportError {
    crate::transport_failure::TransportError::new("Invalid URL", Some("ERR_INVALID_URL"))
        .with_name("TypeError")
}

pub const WEB_FETCH_MAX_URL_LENGTH: u64 = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FetchableKind {
    Html,
    Text,
}

pub fn parse_fetch_url(input: &str) -> Result<Url, HarnessError> {
    let url = Url::parse(input).map_err(|_| {
        WebError::new(format!("invalid URL: {input}"), "WEB_INVALID_URL")
            .with_cause(invalid_url_cause())
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(WebError::new(
            format!(
                "unsupported URL scheme \"{}:\" (only http and https are allowed)",
                url.scheme()
            ),
            "WEB_INVALID_URL",
        ));
    }
    if !url.username().is_empty() || url.password().is_some_and(|p| !p.is_empty()) {
        return Err(WebError::new(
            "credentials in URLs are not allowed",
            "WEB_BLOCKED_URL",
        ));
    }
    Ok(url)
}

pub fn validate_fetch_url(input: &str) -> Result<Url, HarnessError> {
    if input.encode_utf16().count() as u64 > WEB_FETCH_MAX_URL_LENGTH {
        return Err(WebError::new(
            format!("URL exceeds the maximum length of {WEB_FETCH_MAX_URL_LENGTH}"),
            "WEB_INVALID_URL",
        ));
    }
    parse_fetch_url(input)
}

pub fn is_same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme() && a.host_str() == b.host_str() && a.port() == b.port()
}

pub fn classify_content_type(content_type: Option<&str>) -> Option<FetchableKind> {
    let mime = js_trim(
        content_type
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default(),
    )
    .to_lowercase();
    if mime == "text/html" || mime == "application/xhtml+xml" {
        return Some(FetchableKind::Html);
    }
    if mime.starts_with("text/")
        || mime == "application/json"
        || mime == "application/xml"
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
    {
        return Some(FetchableKind::Text);
    }
    None
}

pub fn parse_charset(content_type: Option<&str>) -> Option<String> {
    let pattern = format!(
        r#";[{ws}]*(?i-u:charset)[{ws}]*=[{ws}]*"?([^";]+)"?"#,
        ws = JS_WHITE_SPACE_SET
    );
    // Fixed source expression, with the JavaScript whitespace class substituted.
    let regex = regex::Regex::new(&pattern).expect("source charset expression is valid");
    regex
        .captures(content_type.unwrap_or_default())
        .and_then(|m| m.get(1))
        .map(|m| js_trim(m.as_str()).to_lowercase())
}

/// TextDecoder state fixed by the declaration; decode is performed once after the byte cap.
#[derive(Clone, Copy, Debug)]
pub struct CharsetDecoder {
    encoding: &'static Encoding,
    pub fatal: bool,
}
impl CharsetDecoder {
    pub fn encoding(&self) -> String {
        self.encoding.name().to_ascii_lowercase()
    }
    pub fn decode(&self, bytes: &[u8], truncated_by_bytes: bool) -> Result<String, HarnessError> {
        if self.fatal {
            let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
            return match std::str::from_utf8(bytes) {
                Ok(text) => Ok(text.into()),
                Err(error) if error.error_len().is_none() && truncated_by_bytes => {
                    // valid_up_to is a UTF-8 boundary proved by the decoder.
                    Ok(std::str::from_utf8(&bytes[..error.valid_up_to()])
                        .expect("validated UTF-8 prefix")
                        .into())
                }
                Err(_) => Err(WebError::new(
                    "response body is not valid UTF-8 for the malformed charset declaration",
                    "WEB_UNSUPPORTED_CONTENT_TYPE",
                )
                .with_cause(DecoderFailure::InvalidUtf8)),
            };
        }
        let mut decoder = self.encoding.new_decoder_with_bom_removal();
        // The documented maximum accounts for replacement sequences and pending decoder state.
        let capacity = decoder
            .max_utf8_buffer_length(bytes.len())
            .expect("bounded response fits decoder output size");
        let mut output = String::with_capacity(capacity);
        let (_, read, _) = decoder.decode_to_string(bytes, &mut output, true);
        // capacity is the decoder's own upper bound, so every byte is consumed.
        debug_assert_eq!(read, bytes.len());
        Ok(output)
    }
}

pub fn decoder_for_charset(charset: Option<&str>) -> Result<CharsetDecoder, HarnessError> {
    let declared = charset.unwrap_or("utf-8");
    if let Some(encoding) = Encoding::for_label(declared.as_bytes()).filter(|encoding| {
        *encoding != encoding_rs::REPLACEMENT
            && *encoding != encoding_rs::X_USER_DEFINED
            && *encoding != encoding_rs::ISO_8859_16
    }) {
        return Ok(CharsetDecoder {
            encoding,
            fatal: false,
        });
    }
    if declared
        .split(|c| is_js_white_space(c) || c == ',')
        .any(|label| Encoding::for_label(label.as_bytes()) == Some(UTF_8))
    {
        return Ok(CharsetDecoder {
            encoding: UTF_8,
            fatal: true,
        });
    }
    Err(WebError::new(
        format!("unsupported charset \"{declared}\""),
        "WEB_UNSUPPORTED_CONTENT_TYPE",
    )
    .with_cause(DecoderFailure::UnsupportedEncoding(declared.into())))
}
