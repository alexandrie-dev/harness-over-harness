use hoh_kernel::{Plugin, PluginDef};
use hoh_web_fetch_http::{Config, DEFAULT_USER_AGENT, WebFetchHttpPlugin, policy::*};
use serde_json::json;
use url::Url;

#[test]
fn private_loopback_link_local_and_metadata_addresses_are_allowed_without_resolution() {
    for host in [
        "127.0.0.1",
        "localhost",
        "0.0.0.0",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.0.1",
        "169.254.169.254",
        "[::1]",
        "[fc00::1]",
        "[fe80::1]",
        "unresolved.invalid",
    ] {
        let input = format!("http://{host}/");
        assert!(validate_fetch_url(&input).is_ok(), "{input}");
    }
}
#[test]
fn origins_use_canonical_default_ports_but_reject_changed_ports_schemes_and_hosts() {
    let a = Url::parse("http://EXAMPLE.com:80/a").unwrap();
    assert!(is_same_origin(
        &a,
        &Url::parse("http://example.com/b").unwrap()
    ));
    for target in [
        "http://example.com:81/b",
        "https://example.com/b",
        "http://elsewhere.test/b",
    ] {
        assert!(!is_same_origin(&a, &Url::parse(target).unwrap()));
    }
    for input in ["http://user@example.com", "http://:pass@example.com"] {
        assert_eq!(
            validate_fetch_url(input).unwrap_err().code(),
            "WEB_BLOCKED_URL"
        );
    }
    assert!(validate_fetch_url("http://:@example.com/").is_ok());
    assert_eq!(
        validate_fetch_url("data:text/plain,hello")
            .unwrap_err()
            .message(),
        "unsupported URL scheme \"data:\" (only http and https are allowed)"
    );
}
#[test]
fn url_length_uses_utf16_before_percent_encoding_and_checks_length_before_scheme() {
    let prefix = "http://example.com/";
    let payload = "😀".repeat((2048 - prefix.len()) / 2);
    let mut exact = format!("{prefix}{payload}");
    if exact.encode_utf16().count() < 2048 {
        exact.push('x');
    }
    assert_eq!(exact.encode_utf16().count(), 2048);
    assert!(validate_fetch_url(&exact).unwrap().as_str().len() > 2048);
    assert_eq!(
        validate_fetch_url(&(exact + "x")).unwrap_err().message(),
        "URL exceeds the maximum length of 2048"
    );
    assert_eq!(
        validate_fetch_url(&format!("ftp:{}", "x".repeat(2048)))
            .unwrap_err()
            .message(),
        "URL exceeds the maximum length of 2048"
    );
}
#[test]
fn content_classification_and_charset_matching_use_javascript_whitespace_and_ascii_case() {
    assert_eq!(
        classify_content_type(Some("\u{feff}TEXT/HTML \u{feff}; charset=utf-8")),
        Some(FetchableKind::Html)
    );
    assert_eq!(classify_content_type(Some("\u{85}text/html")), None);
    for mime in [
        "application/xml",
        "application/ld+json",
        "something+xml",
        "text/anything",
    ] {
        assert_eq!(classify_content_type(Some(mime)), Some(FetchableKind::Text));
    }
    assert_eq!(
        parse_charset(Some(
            "text/plain;\u{feff}CHARSET\u{feff}=\u{feff} UTF-8\u{feff}"
        )),
        Some("utf-8".into())
    );
    assert_eq!(parse_charset(Some("text/plain; charſet=utf-8")), None);
    assert_eq!(parse_charset(Some("text/plain;\u{85}charset=utf-8")), None);
}
#[test]
fn decoders_strip_only_their_bom_and_recover_only_incomplete_utf8_at_a_real_cap() {
    for unsupported in [
        "replacement",
        "iso-2022-cn",
        "x-user-defined",
        "iso-8859-16",
        "latin10",
    ] {
        assert_eq!(
            decoder_for_charset(Some(unsupported)).unwrap_err().code(),
            "WEB_UNSUPPORTED_CONTENT_TYPE"
        );
    }
    let utf8 = decoder_for_charset(None).unwrap();
    assert_eq!(utf8.decode(&[0xef, 0xbb, 0xbf, b'x'], false).unwrap(), "x");
    let utf16 = decoder_for_charset(Some("utf-16le")).unwrap();
    assert_eq!(utf16.decode(&[0xff, 0xfe, b'x', 0], false).unwrap(), "x");
    let strict = decoder_for_charset(Some("gbk,utf-8")).unwrap();
    assert!(strict.decode(&[0xe4, 0xb8], false).is_err());
    assert_eq!(strict.decode(&[0xe4, 0xb8], true).unwrap(), "");
    assert!(strict.decode(&[0x81, 0xe4], true).is_err());
    assert_eq!(
        decoder_for_charset(Some("utf-8\u{feff},gbk"))
            .unwrap()
            .encoding(),
        "utf-8"
    );
}
#[test]
fn config_schema_defaults_and_validation_order_match_the_source() {
    let c: Config = serde_json::from_value(json!({})).unwrap();
    assert_eq!(c.max_response_bytes, 5_000_000.0);
    assert_eq!(c.max_body_chars, 100_000.0);
    assert_eq!(c.timeout_ms, 30_000.0);
    assert_eq!(c.max_redirects, 5.0);
    assert_eq!(c.user_agent, DEFAULT_USER_AGENT);
    let schema = PluginDef::of::<WebFetchHttpPlugin>().schema().clone();
    assert_eq!(schema["default"], json!({}));
    assert_eq!(<WebFetchHttpPlugin as Plugin>::NAME, "web-fetch-http");
    assert_eq!(
        schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "maxResponseBytes",
            "maxBodyChars",
            "timeoutMs",
            "maxRedirects",
            "userAgent"
        ]
    );
    for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
        assert_eq!(
            Config {
                max_response_bytes: n,
                ..Default::default()
            }
            .validate()
            .unwrap_err()
            .to_string(),
            "web-fetch-http: maxResponseBytes must be a positive finite number"
        );
        assert_eq!(
            Config {
                max_body_chars: n,
                ..Default::default()
            }
            .validate()
            .unwrap_err()
            .to_string(),
            "web-fetch-http: maxBodyChars must be a positive finite number"
        );
        assert_eq!(
            Config {
                timeout_ms: n,
                ..Default::default()
            }
            .validate()
            .unwrap_err()
            .to_string(),
            "web-fetch-http: timeoutMs must be a positive finite number"
        );
    }
    assert_eq!(
        Config {
            max_body_chars: -1.0,
            timeout_ms: 0.0,
            max_redirects: -1.0,
            ..Default::default()
        }
        .validate()
        .unwrap_err()
        .to_string(),
        "web-fetch-http: maxBodyChars must be a positive finite number"
    );
    assert_eq!(
        Config {
            max_response_bytes: 0.5,
            max_body_chars: 2.9,
            ..Default::default()
        }
        .limits()
        .unwrap()
        .max_response_bytes,
        0
    );
    assert_eq!(
        Config {
            max_response_bytes: 0.5,
            max_body_chars: 2.9,
            ..Default::default()
        }
        .limits()
        .unwrap()
        .max_body_chars,
        2
    );
}
