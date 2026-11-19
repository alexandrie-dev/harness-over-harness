use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_web::{WebFetchRequest, WebFuture};
use hoh_web_fetch_http::{
    HttpFetchLimits, HttpFetchProvider, provider::HttpFetchTransport, transport_failure::*,
};
use serde_json::json;
use std::{error::Error, fmt, sync::Arc};
use url::Url;
struct Fails(SharedError);
impl HttpFetchTransport for Fails {
    fn get<'a>(&'a self, _: &'a Url, _: &'a str) -> WebFuture<'a, reqwest::Response> {
        Box::pin(async { Err(self.0.clone().into()) })
    }
}
async fn rejection_for(error: impl Into<BoxError>) -> TransportFailure {
    let provider = HttpFetchProvider::with_transport(
        HttpFetchLimits::default(),
        Arc::new(Fails(SharedError::new(error))),
    );
    let error = provider
        .fetch(
            WebFetchRequest {
                url: "http://origin.test/".into(),
            },
            None,
        )
        .await
        .unwrap_err();
    let error = HarnessError::of(error.as_ref()).unwrap();
    TransportFailure {
        code: error.code().into(),
        message: error.message().into(),
    }
}
fn system(message: &str, code: &str) -> SharedError {
    SharedError::new(TransportError::new(message, Some(code)))
}
fn attempts(values: Vec<SharedError>) -> TransportError {
    TransportError::aggregate("", values)
}

#[tokio::test(start_paused = true)]
async fn classifies_system_transport_failures() {
    for (detail, system_code, code, summary) in [
        (
            "getaddrinfo ENOTFOUND origin.test",
            "ENOTFOUND",
            "WEB_FETCH_DNS_FAILED",
            "could not resolve the host name",
        ),
        (
            "getaddrinfo EAI_AGAIN origin.test",
            "EAI_AGAIN",
            "WEB_FETCH_DNS_FAILED",
            "could not resolve the host name",
        ),
        (
            "connect ECONNREFUSED 127.0.0.1:8080",
            "ECONNREFUSED",
            "WEB_FETCH_CONNECTION_REFUSED",
            "the server refused the connection",
        ),
        (
            "Connect Timeout Error",
            "UND_ERR_CONNECT_TIMEOUT",
            "WEB_FETCH_CONNECTION_TIMEOUT",
            "the connection timed out",
        ),
        (
            "read ETIMEDOUT",
            "ETIMEDOUT",
            "WEB_FETCH_CONNECTION_TIMEOUT",
            "the connection timed out",
        ),
        (
            "Hostname mismatch",
            "HOSTNAME_MISMATCH",
            "WEB_FETCH_CERTIFICATE_INVALID",
            "the server certificate could not be verified",
        ),
        (
            "self-signed certificate",
            "DEPTH_ZERO_SELF_SIGNED_CERT",
            "WEB_FETCH_CERTIFICATE_INVALID",
            "the server certificate could not be verified",
        ),
        (
            "certificate has expired",
            "CERT_HAS_EXPIRED",
            "WEB_FETCH_CERTIFICATE_INVALID",
            "the server certificate could not be verified",
        ),
        (
            "Hostname/IP does not match certificate's altnames",
            "ERR_TLS_CERT_ALTNAME_INVALID",
            "WEB_FETCH_CERTIFICATE_INVALID",
            "the server certificate could not be verified",
        ),
        (
            "read ECONNRESET",
            "ECONNRESET",
            "WEB_FETCH_CONNECTION_INTERRUPTED",
            "the connection was interrupted",
        ),
        (
            "other side closed",
            "UND_ERR_SOCKET",
            "WEB_FETCH_CONNECTION_INTERRUPTED",
            "the connection was interrupted",
        ),
        (
            "connect ENETUNREACH 10.0.0.1:443",
            "ENETUNREACH",
            "WEB_FETCH_NETWORK_UNREACHABLE",
            "the network or host is unreachable",
        ),
        (
            "connect EHOSTUNREACH 10.0.0.1:443",
            "EHOSTUNREACH",
            "WEB_FETCH_NETWORK_UNREACHABLE",
            "the network or host is unreachable",
        ),
    ] {
        assert_eq!(
            rejection_for(TransportError::fetch_failed(system(detail, system_code))).await,
            TransportFailure {
                code: code.into(),
                message: format!("web fetch failed: {summary} ({system_code}): {detail}")
            }
        );
    }
}
#[tokio::test(start_paused = true)]
async fn reports_an_unrecognized_system_code_as_a_generic_network_failure_that_keeps_the_code() {
    assert_eq!(
        rejection_for(TransportError::fetch_failed(system(
            "write EPROTO",
            "EPROTO"
        )))
        .await,
        TransportFailure {
            code: "WEB_FETCH_NETWORK_ERROR".into(),
            message: "web fetch failed: the network request failed (EPROTO): write EPROTO".into()
        }
    );
}
#[tokio::test(start_paused = true)]
async fn ignores_a_non_string_code() {
    let cause = TransportError::new("socket hang up", None).with_code(json!(-54));
    assert_eq!(
        rejection_for(TransportError::fetch_failed(cause)).await,
        TransportFailure {
            code: "WEB_FETCH_NETWORK_ERROR".into(),
            message: "web fetch failed: the network request failed: socket hang up".into()
        }
    );
}
#[tokio::test(start_paused = true)]
async fn reports_a_failure_without_a_system_code_as_a_generic_network_failure() {
    assert_eq!(
        rejection_for(TransportError::fetch_failed(TransportError::new(
            "bad port", None
        )))
        .await,
        TransportFailure {
            code: "WEB_FETCH_NETWORK_ERROR".into(),
            message: "web fetch failed: the network request failed: bad port".into()
        }
    );
}
#[tokio::test(start_paused = true)]
async fn classifies_attempts_that_share_one_class_and_names_each_address() {
    let e = attempts(vec![
        system("connect ECONNREFUSED ::1:8080", "ECONNREFUSED"),
        system("connect ECONNREFUSED 127.0.0.1:8080", "ECONNREFUSED"),
    ]);
    assert_eq!(rejection_for(TransportError::fetch_failed(e)).await,TransportFailure {code:"WEB_FETCH_CONNECTION_REFUSED".into(),message:"web fetch failed: the server refused the connection (ECONNREFUSED): connect ECONNREFUSED ::1:8080; connect ECONNREFUSED 127.0.0.1:8080".into()});
}
#[tokio::test(start_paused = true)]
async fn classifies_an_unreachable_address_family_attempt_by_the_other_attempts() {
    assert_eq!(rejection_for(TransportError::fetch_failed(attempts(vec![system("connect ENETUNREACH 2001:db8::1:443","ENETUNREACH"),system("connect ECONNREFUSED 192.0.2.1:443","ECONNREFUSED")]))).await,TransportFailure {code:"WEB_FETCH_CONNECTION_REFUSED".into(),message:"web fetch failed: the server refused the connection: connect ENETUNREACH 2001:db8::1:443; connect ECONNREFUSED 192.0.2.1:443".into()});
}
#[tokio::test(start_paused = true)]
async fn does_not_classify_attempts_of_two_other_classes() {
    assert_eq!(rejection_for(TransportError::fetch_failed(attempts(vec![system("connect ETIMEDOUT 2001:db8::1:443","ETIMEDOUT"),system("connect ECONNREFUSED 192.0.2.1:443","ECONNREFUSED")]))).await,TransportFailure {code:"WEB_FETCH_NETWORK_ERROR".into(),message:"web fetch failed: the network request failed: connect ETIMEDOUT 2001:db8::1:443; connect ECONNREFUSED 192.0.2.1:443".into()});
}
#[tokio::test(start_paused = true)]
async fn names_exactly_four_attempts_without_a_count() {
    let e = attempts(
        (1..=4)
            .map(|n| SharedError::new(TransportError::new(format!("attempt {n}"), None)))
            .collect(),
    );
    assert_eq!(
        rejection_for(TransportError::fetch_failed(e)).await.message,
        "web fetch failed: the network request failed: attempt 1; attempt 2; attempt 3; attempt 4"
    );
}
#[tokio::test(start_paused = true)]
async fn names_four_of_five_attempts_and_counts_the_rest() {
    let e = attempts(
        (1..=5)
            .map(|n| SharedError::new(TransportError::new(format!("attempt {n}"), None)))
            .collect(),
    );
    assert_eq!(
        rejection_for(TransportError::fetch_failed(e)).await.message,
        "web fetch failed: the network request failed: attempt 1; attempt 2; attempt 3; attempt 4; 1 more"
    );
}
#[tokio::test(start_paused = true)]
async fn does_not_expand_an_aggregate_error_nested_in_an_attempt() {
    let inner = TransportError::aggregate(
        "inner attempts",
        vec![SharedError::new(TransportError::new("deep", None))],
    );
    assert_eq!(
        rejection_for(attempts(vec![SharedError::new(inner)]))
            .await
            .message,
        "web fetch failed: the network request failed: inner attempts"
    );
}
#[tokio::test(start_paused = true)]
async fn stringifies_a_non_error_rejection() {
    assert_eq!(
        rejection_for(TransportValue::Thrown(json!("socket closed")))
            .await
            .message,
        "web fetch failed: the network request failed: socket closed"
    );
    assert_eq!(
        rejection_for(TransportValue::Undefined).await.message,
        "web fetch failed: the network request failed: undefined"
    );
    for (value, text) in [
        (json!(null), "null"),
        (json!(true), "true"),
        (json!({"x": 1}), "[object Object]"),
        (json!([1, null, "a"]), "1,,a"),
    ] {
        assert_eq!(
            rejection_for(TransportValue::Thrown(value)).await.message,
            format!("web fetch failed: the network request failed: {text}")
        );
    }
}
#[tokio::test(start_paused = true)]
async fn stringifies_a_non_error_innermost_cause() {
    assert_eq!(
        rejection_for(TransportError::fetch_failed(TransportValue::Thrown(json!(
            "socket closed"
        ))))
        .await
        .message,
        "web fetch failed: the network request failed: socket closed"
    );
}
#[derive(Debug)]
struct Cycle(bool);
static FIRST: Cycle = Cycle(false);
static SECOND: Cycle = Cycle(true);
impl fmt::Display for Cycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0 { "second" } else { "first" })
    }
}
impl Error for Cycle {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(if self.0 { &FIRST } else { &SECOND })
    }
}
#[tokio::test(start_paused = true)]
async fn cuts_a_cyclic_cause_chain() {
    assert_eq!(
        rejection_for(Cycle(false)).await.message,
        "web fetch failed: the network request failed: first"
    );
}
#[tokio::test(start_paused = true)]
async fn follows_four_cause_hops() {
    let mut chain = TransportError::new("hop 0", None);
    for hop in 1..=5 {
        chain = TransportError::new(format!("hop {hop}"), None).with_cause(chain);
    }
    assert_eq!(
        rejection_for(chain).await.message,
        "web fetch failed: the network request failed: hop 1"
    );
}
#[tokio::test(start_paused = true)]
async fn falls_back_to_the_rejection_when_the_innermost_message_is_empty() {
    assert_eq!(
        rejection_for(TransportError::fetch_failed(attempts(vec![])))
            .await
            .message,
        "web fetch failed: the network request failed: TypeError: fetch failed"
    );
}
#[tokio::test(start_paused = true)]
async fn keeps_the_full_summary_and_500_characters_of_the_detail() {
    let prefix = "web fetch failed: the server certificate could not be verified (ERR_TLS_CERT_ALTNAME_INVALID): ";
    for length in [500, 501] {
        let e = system(&"x".repeat(length), "ERR_TLS_CERT_ALTNAME_INVALID");
        assert_eq!(
            rejection_for(TransportError::fetch_failed(e)).await.message,
            format!(
                "{prefix}{}{}",
                "x".repeat(500),
                if length == 501 { "…" } else { "" }
            )
        );
    }
}
