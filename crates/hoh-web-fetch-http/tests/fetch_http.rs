mod support;
use hoh_error::{HarnessError, SharedError};
use hoh_kernel::Kernel;
use hoh_timeout::{AbortController, AbortError, TimeoutReason};
use hoh_web::{Web, WebFetchBody, WebFetchProvider, WebFetchRequest, WebFuture, WebRuntime};
use hoh_web_fetch_http::{
    HttpFetchProvider, LOCAL_FETCH_PROVIDER_ID, WebFetchHttpPlugin, policy::*,
    provider::HttpFetchTransport,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use support::*;
use tokio::sync::Notify;
use url::Url;

#[test]
fn validates_scheme_credentials_and_length() {
    assert_eq!(
        parse_fetch_url("https://example.com/preflight")
            .unwrap()
            .path(),
        "/preflight"
    );
    assert_eq!(
        validate_fetch_url("https://example.com/x")
            .unwrap()
            .host_str(),
        Some("example.com")
    );
    for input in ["ftp://example.com", "not a url"] {
        failure(
            validate_fetch_url(input).unwrap_err().into(),
            "WEB_INVALID_URL",
            None,
        );
    }
    failure(
        validate_fetch_url("https://user:pass@example.com")
            .unwrap_err()
            .into(),
        "WEB_BLOCKED_URL",
        Some("credentials in URLs are not allowed"),
    );
    let prefix = "https://example.com/";
    let exact = format!(
        "{prefix}{}",
        "a".repeat(WEB_FETCH_MAX_URL_LENGTH as usize - prefix.len())
    );
    assert_eq!(validate_fetch_url(&exact).unwrap().as_str(), exact);
    failure(
        validate_fetch_url(&(exact + "a")).unwrap_err().into(),
        "WEB_INVALID_URL",
        Some("URL exceeds the maximum length of 2048"),
    );
}
#[test]
fn classifies_content_types() {
    for (header, kind) in [
        ("text/html; charset=utf-8", FetchableKind::Html),
        ("application/xhtml+xml", FetchableKind::Html),
        ("text/plain", FetchableKind::Text),
        ("application/json", FetchableKind::Text),
    ] {
        assert_eq!(classify_content_type(Some(header)), Some(kind));
    }
    assert_eq!(classify_content_type(Some("image/png")), None);
    assert_eq!(classify_content_type(None), None);
}
#[test]
fn compares_origins() {
    let a = Url::parse("https://a.com/x").unwrap();
    assert!(is_same_origin(&a, &Url::parse("https://a.com/y").unwrap()));
    assert!(!is_same_origin(&a, &Url::parse("https://b.com").unwrap()));
    assert!(!is_same_origin(&a, &Url::parse("http://a.com").unwrap()));
}
#[test]
fn parses_the_charset_parameter() {
    assert_eq!(
        parse_charset(Some("text/html; charset=UTF-8")).as_deref(),
        Some("utf-8")
    );
    assert_eq!(
        parse_charset(Some("text/plain; charset=\"iso-8859-1\"")).as_deref(),
        Some("iso-8859-1")
    );
    assert_eq!(parse_charset(Some("text/plain")), None);
    assert_eq!(parse_charset(None), None);
}
#[test]
fn builds_a_decoder_for_a_charset_and_defaults_to_utf8() {
    assert_eq!(decoder_for_charset(None).unwrap().encoding(), "utf-8");
    assert_eq!(
        decoder_for_charset(Some("iso-8859-1")).unwrap().encoding(),
        "windows-1252"
    );
    assert_eq!(
        decoder_for_charset(Some("gb2312")).unwrap().encoding(),
        "gbk"
    );
    assert!(!decoder_for_charset(Some("utf-8")).unwrap().fatal);
    failure(
        decoder_for_charset(Some("not-a-charset"))
            .unwrap_err()
            .into(),
        "WEB_UNSUPPORTED_CONTENT_TYPE",
        Some("unsupported charset \"not-a-charset\""),
    );
}
#[test]
fn requires_strict_utf8_for_malformed_declarations() {
    for charset in [
        "utf-8,gbk",
        "gbk,utf8",
        "not-a-charset, utf-8",
        "utf-8 utf8",
    ] {
        let decoder = decoder_for_charset(Some(charset)).unwrap();
        assert_eq!(decoder.encoding(), "utf-8");
        assert!(decoder.fatal);
    }
}
#[test]
fn rejects_a_malformed_declaration_without_utf8() {
    for charset in ["gb2312,gbk", "windows-1252,gbk", ","] {
        failure(
            decoder_for_charset(Some(charset)).unwrap_err().into(),
            "WEB_UNSUPPORTED_CONTENT_TYPE",
            None,
        );
    }
}

#[tokio::test(start_paused = true)]
async fn fetches_a_text_body() {
    let server = Server::new(|_| Reply::text("hello world")).await;
    let p = provider();
    let result = fetch(&p, &server.base).await.unwrap();
    assert!(p.available().unwrap());
    assert_eq!(result.status_code, 200);
    assert_eq!(
        result.body,
        WebFetchBody::Text {
            content: "hello world".into()
        }
    );
    assert!(!result.truncated);
}
#[tokio::test(start_paused = true)]
async fn fetches_an_html_body_and_classifies_it_as_html() {
    let server =
        Server::new(|_| Reply::text("<h1>hi</h1>").header("content-type", "text/html")).await;
    assert_eq!(
        fetch(&provider(), &server.base).await.unwrap().body,
        WebFetchBody::Html {
            content: "<h1>hi</h1>".into()
        }
    );
}
#[tokio::test(start_paused = true)]
async fn sends_the_configured_user_agent() {
    let server = Server::new(|_| Reply::text("ok")).await;
    fetch(&provider(), &server.base).await.unwrap();
    let headers = server.requests.lock().unwrap()[0]
        .headers
        .to_ascii_lowercase();
    assert!(headers.contains("user-agent: test-agent/1.0\r\n"));
    assert!(headers.contains(
        "accept: text/html,application/xhtml+xml,text/*;q=0.9,application/json;q=0.8\r\n"
    ));
    assert!(!headers.contains("authorization:"));
    assert!(!headers.contains("cookie:"));
    assert!(headers.contains("accept-language: *\r\n"));
    assert!(headers.contains("sec-fetch-mode: cors\r\n"));
}
#[tokio::test(start_paused = true)]
async fn returns_a_non_2xx_response_as_a_result_not_an_error() {
    let server = Server::new(|_| {
        let mut r = Reply::text("nope");
        r.status = 404;
        r
    })
    .await;
    let result = fetch(&provider(), &server.base).await.unwrap();
    assert_eq!(result.status_code, 404);
    assert_eq!(content(&result), "nope");
}
macro_rules! retrieval_case {
    ($name:ident,$reply:expr,$change:expr,$expected:expr,$truncated:expr) => {
        #[tokio::test(start_paused = true)]
        async fn $name() {
            let server = Server::new(|_| $reply).await;
            let mut caps = limits();
            ($change)(&mut caps);
            let result = fetch(&HttpFetchProvider::new(caps).unwrap(), &server.base)
                .await
                .unwrap();
            assert_eq!(content(&result), $expected);
            assert_eq!(result.truncated, $truncated);
        }
    };
}
retrieval_case!(
    truncates_a_stream_that_grows_past_the_byte_cap,
    Reply::text("abcdefghij"),
    |l: &mut hoh_web_fetch_http::HttpFetchLimits| l.max_response_bytes = 4,
    "abcd",
    true
);
retrieval_case!(
    does_not_flag_a_body_that_exactly_fills_the_byte_cap_as_truncated,
    Reply::text("abcd"),
    |l: &mut hoh_web_fetch_http::HttpFetchLimits| l.max_response_bytes = 4,
    "abcd",
    false
);
retrieval_case!(
    truncates_a_decoded_body_past_the_character_cap,
    Reply::text("abcdefghij"),
    |l: &mut hoh_web_fetch_http::HttpFetchLimits| l.max_body_chars = 3,
    "abc",
    true
);
retrieval_case!(
    accepts_a_declared_content_length_within_the_cap,
    Reply::text("sized").header("content-length", "5"),
    |_: &mut hoh_web_fetch_http::HttpFetchLimits| {},
    "sized",
    false
);
retrieval_case!(
    decodes_a_non_utf8_declared_charset,
    Reply::text([0x63, 0x61, 0x66, 0xe9]).header("content-type", "text/plain; charset=iso-8859-1"),
    |_: &mut hoh_web_fetch_http::HttpFetchLimits| {},
    "café",
    false
);
retrieval_case!(
    keeps_single_label_gbk_decoding,
    Reply::text([0xd6, 0xd0, 0xce, 0xc4]).header("content-type", "text/plain; charset=gbk"),
    |_: &mut hoh_web_fetch_http::HttpFetchLimits| {},
    "中文",
    false
);
retrieval_case!(
    keeps_replacement_decoding_for_a_single_declared_charset,
    Reply::text([0xe4, 0xb8]).header("content-type", "text/plain; charset=utf-8"),
    |_: &mut hoh_web_fetch_http::HttpFetchLimits| {},
    "\u{fffd}",
    false
);
retrieval_case!(
    uses_utf8_even_when_a_long_body_also_forms_valid_gbk,
    Reply::text("©".repeat(1000)).header("content-type", "text/plain; charset=gbk,utf-8"),
    |_: &mut hoh_web_fetch_http::HttpFetchLimits| {},
    "©".repeat(1000),
    false
);

#[tokio::test(start_paused = true)]
async fn rejects_an_over_cap_content_length_with_web_fetch_too_large() {
    let server = Server::new(|_| {
        Reply::text("x")
            .header("content-length", "999999")
            .unfinished()
    })
    .await;
    let mut caps = limits();
    caps.max_response_bytes = 10;
    failure(
        fetch(&HttpFetchProvider::new(caps).unwrap(), &server.base)
            .await
            .unwrap_err(),
        "WEB_FETCH_TOO_LARGE",
        Some("response exceeds the maximum of 10 bytes"),
    );
    server.wait_closed(1).await;
}
#[tokio::test(start_paused = true)]
async fn rejects_an_unsupported_content_type() {
    let server = Server::new(|_| Reply::text("binary").header("content-type", "image/png")).await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_UNSUPPORTED_CONTENT_TYPE",
        Some("unsupported content type \"image/png\""),
    );
}
#[tokio::test(start_paused = true)]
async fn rejects_a_response_with_no_content_type_at_all() {
    let server = Server::new(|_| Reply::text("no type").without_type()).await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_UNSUPPORTED_CONTENT_TYPE",
        Some("unsupported content type \"unknown\""),
    );
}
#[tokio::test(start_paused = true)]
async fn rejects_an_unsupported_declared_charset() {
    let server = Server::new(|_| {
        Reply::text("x").header("content-type", "text/plain; charset=not-a-charset")
    })
    .await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_UNSUPPORTED_CONTENT_TYPE",
        Some("unsupported charset \"not-a-charset\""),
    );
}
#[tokio::test(start_paused = true)]
async fn decodes_utf8_html_declared_as_a_compound_charset() {
    for charset in ["utf-8,gbk", "gbk,utf8", "not-a-charset, utf-8"] {
        let ct = format!("text/html; charset={charset}");
        let server = Server::new(move |_| {
            Reply::text("<meta charset=\"UTF-8\"><p>你好</p>").header("content-type", &ct)
        })
        .await;
        let result = fetch(&provider(), &server.base).await.unwrap();
        assert_eq!(
            result.body,
            WebFetchBody::Html {
                content: "<meta charset=\"UTF-8\"><p>你好</p>".into()
            }
        );
        assert!(!result.truncated);
    }
}
#[tokio::test(start_paused = true)]
async fn rejects_non_utf8_bytes_for_compound_charsets() {
    for (charset, bytes) in [
        ("gbk,utf-8", vec![0xe4, 0xb8]),
        ("gbk,utf-8", vec![0xe4]),
        ("gbk,utf-8", vec![0x81, 0x20]),
        ("gbk,utf-8", vec![0xd6, 0xd0, 0xce, 0xc4]),
        ("not-a-charset,utf-8", vec![0xd6, 0xd0, 0xce, 0xc4]),
        ("utf8,utf-8", vec![0x81, 0x20]),
    ] {
        let ct = format!("text/plain; charset={charset}");
        let server = Server::new(move |_| Reply::text(&bytes).header("content-type", &ct)).await;
        failure(
            fetch(&provider(), &server.base).await.unwrap_err(),
            "WEB_UNSUPPORTED_CONTENT_TYPE",
            Some("response body is not valid UTF-8 for the malformed charset declaration"),
        );
    }
}
#[tokio::test(start_paused = true)]
async fn rejects_a_legacy_only_compound_declaration() {
    for charset in ["gb2312,gbk", "windows-1252,gbk"] {
        let ct = format!("text/plain; charset={charset}");
        let server =
            Server::new(move |_| Reply::text([0xd6, 0xd0, 0xce, 0xc4]).header("content-type", &ct))
                .await;
        failure(
            fetch(&provider(), &server.base).await.unwrap_err(),
            "WEB_UNSUPPORTED_CONTENT_TYPE",
            Some(&format!("unsupported charset \"{charset}\"")),
        );
    }
}
#[tokio::test(start_paused = true)]
async fn returns_complete_utf8_characters_at_the_byte_cap() {
    for (body, cap, truncated) in [("中文", 5, true), ("中文", 3, true), ("中", 3, false)] {
        let server = Server::new(move |_| {
            Reply::text(body).header("content-type", "text/plain; charset=gbk,utf-8")
        })
        .await;
        let mut limits = limits();
        limits.max_response_bytes = cap;
        let result = fetch(&HttpFetchProvider::new(limits).unwrap(), &server.base)
            .await
            .unwrap();
        assert_eq!(content(&result), "中");
        assert_eq!(result.truncated, truncated);
    }
}
#[tokio::test(start_paused = true)]
async fn validates_eof_when_the_response_exactly_fills_the_byte_cap() {
    let server = Server::new(|_| {
        Reply::text([0xe4, 0xb8]).header("content-type", "text/plain; charset=gbk,utf-8")
    })
    .await;
    let mut limits = limits();
    limits.max_response_bytes = 2;
    failure(
        fetch(&HttpFetchProvider::new(limits).unwrap(), &server.base)
            .await
            .unwrap_err(),
        "WEB_UNSUPPORTED_CONTENT_TYPE",
        Some("response body is not valid UTF-8 for the malformed charset declaration"),
    );
}
#[tokio::test(start_paused = true)]
async fn rejects_invalid_utf8_before_the_truncated_suffix() {
    let server = Server::new(|_| {
        Reply::text([0x81, 0x20, 0x61]).header("content-type", "text/plain; charset=gbk,utf-8")
    })
    .await;
    let mut limits = limits();
    limits.max_response_bytes = 2;
    failure(
        fetch(&HttpFetchProvider::new(limits).unwrap(), &server.base)
            .await
            .unwrap_err(),
        "WEB_UNSUPPORTED_CONTENT_TYPE",
        Some("response body is not valid UTF-8 for the malformed charset declaration"),
    );
}

#[tokio::test(start_paused = true)]
async fn follows_a_same_origin_redirect_and_reports_the_final_url() {
    let server = Server::new(|r| {
        if r.target == "/start" {
            Reply::redirect(302, Some("/end"))
        } else {
            Reply::text("arrived")
        }
    })
    .await;
    let result = fetch(&provider(), &format!("{}/start", server.base))
        .await
        .unwrap();
    assert_eq!(content(&result), "arrived");
    assert_eq!(result.url, format!("{}/end", server.base));
}
#[tokio::test(start_paused = true)]
async fn blocks_a_cross_origin_redirect_with_web_redirect_blocked() {
    let server = Server::new(|_| Reply::redirect(302, Some("https://example.com/"))).await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_REDIRECT_BLOCKED",
        Some(
            "cross-origin redirect to https://example.com is not followed automatically; retry against that URL directly",
        ),
    );
    assert_eq!(server.count(), 1);
}
#[tokio::test(start_paused = true)]
async fn revalidates_a_redirect_target_rejecting_same_origin_credentials_in_the_location() {
    let server = Server::new(|r| {
        let host = r
            .headers
            .lines()
            .find_map(|l| l.strip_prefix("host: "))
            .unwrap();
        Reply::redirect(302, Some(&format!("http://user:pass@{host}/")))
    })
    .await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_BLOCKED_URL",
        Some("credentials in URLs are not allowed"),
    );
    assert_eq!(server.count(), 1);
}
fn redirect_number(request: &Request) -> u64 {
    request
        .target
        .split("n=")
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap()
}
#[tokio::test(start_paused = true)]
async fn rejects_exceeding_the_redirect_hop_cap() {
    let server =
        Server::new(|r| Reply::redirect(302, Some(&format!("/?n={}", redirect_number(r) + 1))))
            .await;
    let mut caps = limits();
    caps.max_redirects = 2;
    failure(
        fetch(
            &HttpFetchProvider::new(caps).unwrap(),
            &format!("{}/?n=0", server.base),
        )
        .await
        .unwrap_err(),
        "WEB_REDIRECT_BLOCKED",
        None,
    );
}
#[tokio::test(start_paused = true)]
async fn follows_exactly_max_redirects_hops_a_chain_landing_on_the_nth_redirect_succeeds() {
    let server = Server::new(|r| {
        if redirect_number(r) >= 2 {
            Reply::text("landed")
        } else {
            Reply::redirect(302, Some(&format!("/?n={}", redirect_number(r) + 1)))
        }
    })
    .await;
    let mut caps = limits();
    caps.max_redirects = 2;
    assert_eq!(
        content(
            &fetch(
                &HttpFetchProvider::new(caps).unwrap(),
                &format!("{}/?n=0", server.base)
            )
            .await
            .unwrap()
        ),
        "landed"
    );
    assert_eq!(server.count(), 3);
}
#[tokio::test(start_paused = true)]
async fn makes_exactly_max_redirects_plus_one_requests_before_blocking_an_over_long_chain() {
    let server =
        Server::new(|r| Reply::redirect(302, Some(&format!("/?n={}", redirect_number(r) + 1))))
            .await;
    let mut caps = limits();
    caps.max_redirects = 2;
    failure(
        fetch(
            &HttpFetchProvider::new(caps).unwrap(),
            &format!("{}/?n=0", server.base),
        )
        .await
        .unwrap_err(),
        "WEB_REDIRECT_BLOCKED",
        Some("exceeded the maximum of 2 redirects"),
    );
    assert_eq!(server.count(), 3);
}
#[tokio::test(start_paused = true)]
async fn reports_an_over_limit_redirect_as_exceeded_not_cross_origin_even_for_a_cross_origin_target()
 {
    let server = Server::new(|r| {
        Reply::redirect(
            302,
            Some(if redirect_number(r) == 0 {
                "/?n=1"
            } else {
                "https://example.com/"
            }),
        )
    })
    .await;
    let mut caps = limits();
    caps.max_redirects = 1;
    failure(
        fetch(
            &HttpFetchProvider::new(caps).unwrap(),
            &format!("{}/?n=0", server.base),
        )
        .await
        .unwrap_err(),
        "WEB_REDIRECT_BLOCKED",
        Some("exceeded the maximum of 1 redirects"),
    );
}
#[tokio::test(start_paused = true)]
async fn max_redirects_zero_follows_no_redirect_but_still_fetches_a_direct_200() {
    let server = Server::new(|r| {
        if r.target == "/r" {
            Reply::redirect(302, Some("/done"))
        } else {
            Reply::text("direct")
        }
    })
    .await;
    let mut caps = limits();
    caps.max_redirects = 0;
    let p = HttpFetchProvider::new(caps).unwrap();
    failure(
        fetch(&p, &format!("{}/r", server.base)).await.unwrap_err(),
        "WEB_REDIRECT_BLOCKED",
        None,
    );
    assert_eq!(
        content(&fetch(&p, &format!("{}/done", server.base)).await.unwrap()),
        "direct"
    );
}
#[tokio::test(start_paused = true)]
async fn treats_a_redirect_without_a_location_header_as_a_provider_error() {
    let server = Server::new(|_| Reply::redirect(302, None)).await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_PROVIDER_ERROR",
        Some("redirect response (HTTP 302) without a Location header"),
    );
}
#[tokio::test(start_paused = true)]
async fn follows_a_relative_same_origin_redirect() {
    let server = Server::new(|r| {
        if r.target == "/a" {
            Reply::redirect(301, Some("b"))
        } else {
            Reply::text("landed")
        }
    })
    .await;
    assert_eq!(
        content(
            &fetch(&provider(), &format!("{}/a", server.base))
                .await
                .unwrap()
        ),
        "landed"
    );
}
#[tokio::test(start_paused = true)]
async fn fetches_a_non_public_destination() {
    let server = Server::new(|_| Reply::text("default")).await;
    for base in [
        server.base.clone(),
        server.base.replace("127.0.0.1", "localhost"),
    ] {
        assert_eq!(
            fetch(&provider(), &base).await.unwrap().body,
            WebFetchBody::Text {
                content: "default".into()
            }
        );
    }
}
#[tokio::test(start_paused = true)]
async fn rejects_a_non_http_scheme_before_any_network_access() {
    failure(
        fetch(&provider(), "ftp://example.com").await.unwrap_err(),
        "WEB_INVALID_URL",
        None,
    );
}
#[tokio::test(start_paused = true)]
async fn rejects_credentials_in_the_url() {
    failure(
        fetch(&provider(), "http://user:pass@127.0.0.1/")
            .await
            .unwrap_err(),
        "WEB_BLOCKED_URL",
        None,
    );
}
#[tokio::test(start_paused = true)]
async fn honors_a_pre_aborted_signal() {
    let controller = AbortController::new();
    controller.abort();
    let error = provider()
        .fetch(
            WebFetchRequest {
                url: "http://127.0.0.1/".into(),
            },
            Some(&controller.signal()),
        )
        .await
        .unwrap_err();
    failure(error, "WEB_ABORTED", Some("web fetch aborted"));
}
#[tokio::test(start_paused = true)]
async fn aborts_an_in_flight_fetch_via_the_signal() {
    let server = Server::new(|_| Reply::closed().unfinished()).await;
    let controller = AbortController::new();
    let signal = controller.signal();
    let url = server.base.clone();
    let running = tokio::spawn(async move {
        provider()
            .fetch(WebFetchRequest { url }, Some(&signal))
            .await
    });
    server.arrived.notified().await;
    controller.abort();
    let error = running.await.unwrap().unwrap_err();
    let harness = HarnessError::of(error.as_ref()).unwrap();
    assert!(
        harness
            .cause()
            .unwrap()
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&controller.signal().reason().unwrap())
    );
    failure(error, "WEB_ABORTED", Some("web fetch aborted"));
}
#[tokio::test(start_paused = true)]
async fn times_out_a_slow_response_with_web_fetch_timeout() {
    let server = Server::new(|_| {
        let mut r = Reply::text("").unfinished();
        r.silent = true;
        r
    })
    .await;
    // A dedicated silent transport keeps the socket open instead of the fixture's close-only reply.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (accepted, seen) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = accepted.send(());
        std::future::pending::<()>().await;
        drop(stream);
    });
    let mut caps = limits();
    caps.timeout_ms = 50.0;
    let p = HttpFetchProvider::new(caps).unwrap();
    let running = tokio::spawn(async move { fetch(&p, &url).await });
    seen.await.unwrap();
    tokio::time::advance(Duration::from_millis(50)).await;
    failure(
        running.await.unwrap().unwrap_err(),
        "WEB_FETCH_TIMEOUT",
        Some("web fetch timed out"),
    );
    task.abort();
    drop(server);
}
struct ObserveHeaders {
    client: reqwest::Client,
    observed: Arc<Notify>,
}
impl HttpFetchTransport for ObserveHeaders {
    fn get<'a>(&'a self, url: &'a Url, agent: &'a str) -> WebFuture<'a, reqwest::Response> {
        Box::pin(async move {
            let response = self
                .client
                .get(url.clone())
                .header("user-agent", agent)
                .send()
                .await?;
            self.observed.notify_one();
            Ok(response)
        })
    }
}
#[tokio::test(start_paused = true)]
async fn classifies_a_timeout_during_the_body_read_as_web_fetch_timeout_not_web_aborted() {
    let server = Server::new(|_| {
        Reply::text("partial")
            .header("content-length", "100")
            .unfinished()
    })
    .await;
    let observed = Arc::new(Notify::new());
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut caps = limits();
    caps.timeout_ms = 80.0;
    let p = HttpFetchProvider::with_transport(
        caps,
        Arc::new(ObserveHeaders {
            client,
            observed: observed.clone(),
        }),
    );
    let url = server.base.clone();
    let running = tokio::spawn(async move { fetch(&p, &url).await });
    observed.notified().await;
    tokio::time::advance(Duration::from_millis(80)).await;
    failure(
        running.await.unwrap().unwrap_err(),
        "WEB_FETCH_TIMEOUT",
        Some("web fetch timed out"),
    );
    server.wait_closed(1).await;
}
#[tokio::test(start_paused = true)]
async fn classifies_a_closed_connection_and_names_the_transport_error() {
    let server = Server::new(|_| Reply::closed()).await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_FETCH_CONNECTION_INTERRUPTED",
        Some(
            "web fetch failed: the connection was interrupted (UND_ERR_SOCKET): other side closed",
        ),
    );
}
#[tokio::test(start_paused = true)]
async fn cancels_the_body_when_a_cross_origin_redirect_is_blocked() {
    let server = Server::new(|_| {
        Reply::redirect(302, Some("https://elsewhere.test/"))
            .header("content-length", "10000")
            .unfinished()
    })
    .await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_REDIRECT_BLOCKED",
        None,
    );
    server.wait_closed(1).await;
}
#[tokio::test(start_paused = true)]
async fn cancels_the_body_when_charset_is_rejected() {
    for charset in ["not-a-charset", "gb2312,gbk", "windows-1252,gbk"] {
        let ct = format!("text/plain; charset={charset}");
        let server = Server::new(move |_| {
            Reply::text("unfinished")
                .header("content-type", &ct)
                .header("content-length", "10000")
                .unfinished()
        })
        .await;
        failure(
            fetch(&provider(), &server.base).await.unwrap_err(),
            "WEB_UNSUPPORTED_CONTENT_TYPE",
            None,
        );
        server.wait_closed(1).await;
    }
}
#[tokio::test(start_paused = true)]
async fn cancels_the_body_when_a_redirect_has_no_location_header() {
    let server = Server::new(|_| {
        Reply::redirect(302, None)
            .header("content-length", "10000")
            .unfinished()
    })
    .await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_PROVIDER_ERROR",
        None,
    );
    server.wait_closed(1).await;
}
async fn plugin_config(raw: serde_json::Value) -> Result<(), String> {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let web = ctx
        .plugin::<WebRuntime>(json!({"fetchProvider":"http"}))
        .unwrap();
    web.settled().await.unwrap();
    let fiber = ctx.plugin::<WebFetchHttpPlugin>(raw).unwrap();
    let result = fiber.settled().await.map_err(|e| e.to_string());
    kernel.shutdown().await;
    result
}
#[tokio::test(start_paused = true)]
async fn registers_the_provider_into_ctx_web_hmr_safe() {
    let server = Server::new(|_| Reply::text("default")).await;
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<WebRuntime>(json!({"fetchProvider":LOCAL_FETCH_PROVIDER_ID}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = ctx.plugin::<WebFetchHttpPlugin>(json!({})).unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(
        ctx.get::<Web>()
            .unwrap()
            .fetch(
                WebFetchRequest {
                    url: server.base.clone()
                },
                None
            )
            .await
            .unwrap()
            .status_code,
        200
    );
    fiber.dispose().await;
    failure(
        ctx.get::<Web>()
            .unwrap()
            .fetch(
                WebFetchRequest {
                    url: server.base.clone(),
                },
                None,
            )
            .await
            .unwrap_err(),
        "WEB_PROVIDER_CONFIGURED_MISSING",
        None,
    );
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_non_positive_resource_limit_at_construction() {
    assert!(
        plugin_config(json!({"maxResponseBytes":-1}))
            .await
            .unwrap_err()
            .contains("maxResponseBytes must be a positive finite number")
    );
}
#[tokio::test(start_paused = true)]
async fn rejects_a_zero_timeout_at_construction() {
    assert!(
        plugin_config(json!({"timeoutMs":0}))
            .await
            .unwrap_err()
            .contains("timeoutMs must be a positive finite number")
    );
}
#[tokio::test(start_paused = true)]
async fn rejects_a_timeout_beyond_node_timer_range_at_construction() {
    assert!(
        plugin_config(json!({"timeoutMs":2147483648_u64}))
            .await
            .unwrap_err()
            .contains("timeoutMs must be no greater than 2147483647")
    );
}
#[tokio::test(start_paused = true)]
async fn rejects_a_fractional_redirect_cap_at_construction() {
    assert!(
        plugin_config(json!({"maxRedirects":1.5}))
            .await
            .unwrap_err()
            .contains("maxRedirects must be a non-negative integer")
    );
}
#[tokio::test(start_paused = true)]
async fn rejects_a_negative_redirect_cap_at_construction() {
    assert!(
        plugin_config(json!({"maxRedirects":-1}))
            .await
            .unwrap_err()
            .contains("maxRedirects must be a non-negative integer")
    );
}
#[tokio::test(start_paused = true)]
async fn accepts_max_redirects_zero_follow_no_redirects_as_valid_config() {
    let server = Server::new(|_| Reply::text("default")).await;
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<WebRuntime>(json!({"fetchProvider":"http"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = ctx
        .plugin::<WebFetchHttpPlugin>(json!({"maxRedirects":0}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(
        ctx.get::<Web>()
            .unwrap()
            .fetch(
                WebFetchRequest {
                    url: server.base.clone()
                },
                None
            )
            .await
            .unwrap()
            .status_code,
        200
    );
    fiber.dispose().await;
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn checks_the_original_compressed_content_length_before_reading_the_decoded_body() {
    // gzip("hello"): 25 wire bytes, five decoded bytes. The source rejects the declared
    // size before reading even though the decoded body would fit the ten-byte cap.
    let gzip = [
        0x1f, 0x8b, 0x08, 0, 0, 0, 0, 0, 0, 0x03, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0, 0x86,
        0xa6, 0x10, 0x36, 0x05, 0, 0, 0,
    ];
    let server = Server::new(move |_| {
        Reply::text(gzip)
            .header("content-encoding", "gzip")
            .header("content-length", "25")
    })
    .await;
    let mut caps = limits();
    caps.max_response_bytes = 10;
    failure(
        fetch(&HttpFetchProvider::new(caps).unwrap(), &server.base)
            .await
            .unwrap_err(),
        "WEB_FETCH_TOO_LARGE",
        Some("response exceeds the maximum of 10 bytes"),
    );
}

#[tokio::test(start_paused = true)]
async fn a_foreign_outer_timeout_is_an_abort_and_body_read_has_an_abort_error_cause() {
    let server = Server::new(|_| {
        Reply::text("partial")
            .header("content-length", "100")
            .unfinished()
    })
    .await;
    let observed = Arc::new(Notify::new());
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let p = HttpFetchProvider::with_transport(
        limits(),
        Arc::new(ObserveHeaders {
            client,
            observed: observed.clone(),
        }),
    );
    let url = server.base.clone();
    let controller = AbortController::new();
    let signal = controller.signal();
    let running =
        tokio::spawn(async move { p.fetch(WebFetchRequest { url }, Some(&signal)).await });
    observed.notified().await;
    let reason = SharedError::new(TimeoutReason::new("TOOL_TIMEOUT", 80.0));
    controller.abort_with(reason.clone());
    let error = running.await.unwrap().unwrap_err();
    let error = HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "WEB_ABORTED");
    assert_eq!(
        error
            .cause()
            .unwrap()
            .downcast_ref::<AbortError>()
            .unwrap()
            .to_string(),
        "The operation was aborted."
    );
    server.wait_closed(1).await;
}

#[tokio::test(start_paused = true)]
async fn a_body_read_transport_failure_is_classified_and_empty_bodies_stay_empty() {
    let server = Server::new(|_| Reply::text("partial").header("content-length", "100")).await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_FETCH_CONNECTION_INTERRUPTED",
        Some(
            "web fetch failed: the connection was interrupted (UND_ERR_SOCKET): other side closed",
        ),
    );
    for status in [204, 205, 304] {
        let server = Server::new(move |_| {
            let mut r = Reply::text("");
            r.status = status;
            r
        })
        .await;
        let result = fetch(&provider(), &server.base).await.unwrap();
        assert_eq!(result.status_code, u64::from(status));
        assert_eq!(content(&result), "");
        assert!(!result.truncated);
    }
}

#[tokio::test(start_paused = true)]
async fn a_malformed_chunk_size_remains_a_generic_network_failure() {
    let server = Server::new(|_| {
        Reply::text(b"zz\r\nbody\r\n0\r\n\r\n").header("transfer-encoding", "chunked")
    })
    .await;
    failure(
        fetch(&provider(), &server.base).await.unwrap_err(),
        "WEB_FETCH_NETWORK_ERROR",
        Some(
            "web fetch failed: the network request failed (HPE_INVALID_CHUNK_SIZE): Response does not match the HTTP/1.1 protocol (Invalid character in chunk size)",
        ),
    );
}

#[tokio::test(start_paused = true)]
async fn redirect_targets_reapply_scheme_and_serialized_length_policy_before_following() {
    for (location, message) in [
        (
            "ftp://example.com/page".to_string(),
            "unsupported URL scheme \"ftp:\" (only http and https are allowed)",
        ),
        (
            format!("/{}", "a".repeat(2048)),
            "URL exceeds the maximum length of 2048",
        ),
    ] {
        let server = Server::new(move |_| Reply::redirect(302, Some(&location))).await;
        failure(
            fetch(&provider(), &server.base).await.unwrap_err(),
            "WEB_INVALID_URL",
            Some(message),
        );
        assert_eq!(server.count(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn every_source_redirect_status_keeps_get_and_non_redirect_statuses_do_not_follow_location() {
    for status in [301, 302, 303, 307, 308] {
        let server = Server::new(move |r| {
            if r.target == "/start" {
                Reply::redirect(status, Some("/done"))
            } else {
                Reply::text("done")
            }
        })
        .await;
        assert_eq!(
            content(
                &fetch(&provider(), &format!("{}/start", server.base))
                    .await
                    .unwrap()
            ),
            "done"
        );
        assert_eq!(server.count(), 2);
        assert!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|r| r.headers.starts_with("GET "))
        );
    }
    for status in [300, 304, 305, 306, 309] {
        let server = Server::new(move |_| {
            let mut r = Reply::text("").header("location", "https://elsewhere.test/");
            r.status = status;
            r
        })
        .await;
        assert_eq!(
            fetch(&provider(), &server.base).await.unwrap().status_code,
            u64::from(status)
        );
        assert_eq!(server.count(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn a_port_change_is_cross_origin_even_between_two_allowed_loopback_destinations() {
    let destination = Server::new(|_| Reply::text("not followed")).await;
    let target = destination.base.clone();
    let origin = Server::new(move |_| Reply::redirect(302, Some(&target))).await;
    failure(
        fetch(&provider(), &origin.base).await.unwrap_err(),
        "WEB_REDIRECT_BLOCKED",
        None,
    );
    assert_eq!(origin.count(), 1);
    assert_eq!(destination.count(), 0);
}
