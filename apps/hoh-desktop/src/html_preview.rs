//! Short-lived, conversation-bound HTML snapshots for sandboxed frames.
use std::{
    collections::VecDeque,
    io::Read,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{
    Manager, Runtime, UriSchemeContext, Url,
    http::{Method, Request, Response, StatusCode},
};

const SCHEME: &str = "hoh-preview";
const LIMIT: usize = 12;
const TTL: Duration = Duration::from_secs(15 * 60);
const CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";

struct Document {
    token: String,
    body: String,
    inserted: Instant,
}

#[derive(Default)]
pub struct HtmlPreviews(Mutex<VecDeque<Document>>);

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

pub fn respond<R: Runtime>(
    context: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let document = if context.webview_label() == "main" && request.method() == Method::GET {
        Url::parse(&request.uri().to_string()).ok().and_then(|url| {
            let token = token(&url)?;
            context.app_handle().try_state::<HtmlPreviews>()?.get(token)
        })
    } else {
        None
    };
    let (status, body) = document.map_or_else(
        || {
            (
                StatusCode::NOT_FOUND,
                b"Preview unavailable. Reload the preview.".to_vec(),
            )
        },
        |body| (StatusCode::OK, body),
    );
    Response::builder()
        .status(status)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Security-Policy", CSP)
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .header("Cache-Control", "no-store")
        .header(
            "Permissions-Policy",
            "clipboard-read=(), clipboard-write=(), camera=(), microphone=(), geolocation=()",
        )
        .body(body)
        .expect("static HTML preview response headers")
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
