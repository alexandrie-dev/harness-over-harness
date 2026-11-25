//! Short-lived, conversation-bound HTML snapshots for sandboxed frames.
use std::{
    collections::VecDeque,
    io::Read,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::Url;

const SCHEME: &str = "hoh-preview";
const LIMIT: usize = 12;
const TTL: Duration = Duration::from_secs(15 * 60);
const CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";

struct Document {
    token: String,
    body: String,
    inserted: Instant,
}

#[derive(Clone, Default)]
pub struct HtmlPreviews(Arc<Mutex<VecDeque<Document>>>);

impl HtmlPreviews {
    pub fn store(&self, body: String) -> Result<String, String> {
        let mut bytes = [0; 24];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut bytes))
            .map_err(|_| "The preview could not be created.")?;
        let token = hoh_protocol::approval::hex(&bytes);
        let mut cache = self.0.lock().map_err(|_| "The preview is unavailable.")?;
        cache.retain(|doc| doc.inserted.elapsed() < TTL);
        while cache.len() >= LIMIT {
            cache.pop_front();
        }
        cache.push_back(Document {
            token: token.clone(),
            body,
            inserted: Instant::now(),
        });
        Ok(format!("{SCHEME}://localhost/{token}"))
    }

    fn get(&self, token: &str) -> Option<Vec<u8>> {
        let cache = self.0.lock().ok()?;
        cache
            .iter()
            .find(|doc| doc.token == token && doc.inserted.elapsed() < TTL)
            .map(|doc| doc.body.as_bytes().to_vec())
    }
}

fn token(url: &Url) -> Option<&str> {
    let path = url.path().strip_prefix('/')?;
    (url.scheme() == SCHEME
        && url.host_str() == Some("localhost")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && path.len() == 48
        && path
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    .then_some(path)
}

pub fn navigation_allowed(url: &Url) -> bool {
    token(url).is_some()
}

#[cfg(target_os = "linux")]
pub fn install(view: &webkit2gtk::WebView, previews: HtmlPreviews) {
    use gtk::gio::MemoryInputStream;
    use gtk::glib::{Bytes, prelude::*};
    use soup::{MessageHeaders, MessageHeadersType};
    use webkit2gtk::{
        PolicyDecisionExt, PolicyDecisionType, ResponsePolicyDecision, ResponsePolicyDecisionExt,
        SecurityManagerExt, URIRequestExt, URISchemeRequestExt, URISchemeResponse,
        URISchemeResponseExt, WebContextExt, WebViewExt,
    };

    let Some(context) = view.context() else {
        return;
    };
    let Some(security) = context.security_manager() else {
        return;
    };
    security.register_uri_scheme_as_secure(SCHEME);
    let owner = view.downgrade();
    // Register with WebKit, outside Tauri's local-origin command capability set.
    context.register_uri_scheme(SCHEME, move |request| {
        let document = if owner
            .upgrade()
            .is_some_and(|view| request.web_view() == Some(view))
            && request.http_method().as_deref() == Some("GET")
        {
            request.uri().and_then(|uri| {
                let url = Url::parse(&uri).ok()?;
                previews.get(token(&url)?)
            })
        } else {
            None
        };
        let (status, body) = document.map_or_else(
            || (404, b"Preview unavailable. Reload the preview.".to_vec()),
            |body| (200, body),
        );
        let length = body.len() as i64;
        let input = MemoryInputStream::from_bytes(&Bytes::from_owned(body));
        let response = URISchemeResponse::new(&input, length);
        response.set_status(status, None);
        response.set_content_type("text/html; charset=utf-8");
        let headers = MessageHeaders::new(MessageHeadersType::Response);
        headers.append("Content-Security-Policy", CSP);
        headers.append("X-Content-Type-Options", "nosniff");
        headers.append("Referrer-Policy", "no-referrer");
        headers.append("Cache-Control", "no-store");
        headers.append(
            "Permissions-Policy",
            "clipboard-read=(), clipboard-write=(), camera=(), microphone=(), geolocation=()",
        );
        response.set_http_headers(headers);
        request.finish_with_response(&response);
    });

    view.connect_decide_policy(|_, decision, kind| {
        if kind != PolicyDecisionType::Response {
            return false;
        }
        let Some(response) = decision.downcast_ref::<ResponsePolicyDecision>() else {
            return false;
        };
        if response.is_main_frame_main_resource()
            && response
                .request()
                .and_then(|request| request.uri())
                .is_some_and(|uri| uri.starts_with("hoh-preview:"))
        {
            decision.ignore();
            return true;
        }
        false
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_tokens_only_resolve_to_bounded_cache_entries() {
        let previews = HtmlPreviews::default();
        let first = Url::parse(&previews.store("first".into()).unwrap()).unwrap();
        let first_token = token(&first).unwrap();
        assert_eq!(previews.get(first_token), Some(b"first".to_vec()));
        assert!(!navigation_allowed(
            &Url::parse(&format!("https://localhost/{first_token}")).unwrap()
        ));
        assert!(!navigation_allowed(
            &Url::parse(&format!("{first}?different")).unwrap()
        ));
        for index in 0..LIMIT {
            previews.store(index.to_string()).unwrap();
        }
        assert_eq!(previews.get(first_token), None);
    }
}
