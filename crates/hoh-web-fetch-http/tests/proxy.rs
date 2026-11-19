mod support;
use hoh_http_proxy::{DispatcherOptions, policy::resolve_proxy_policy, route_client_builder};
use hoh_web_fetch_http::HttpFetchProvider;
use support::*;
fn proxy_provider(proxy: &str) -> HttpFetchProvider {
    let policy = resolve_proxy_policy(&|name: &str| {
        matches!(name, "HTTP_PROXY" | "HTTPS_PROXY").then(|| proxy.to_owned())
    })
    .policy;
    let builder = route_client_builder(policy, DispatcherOptions::default()).unwrap();
    HttpFetchProvider::with_client_builder(limits(), builder).unwrap()
}
const TARGET: &str = "http://origin.test/page";
#[tokio::test(start_paused = true)]
async fn tunnels_the_request_through_the_installed_proxy() {
    let proxy = Server::new(|_| Reply::text("via-proxy")).await;
    assert_eq!(
        content(&fetch(&proxy_provider(&proxy.base), TARGET).await.unwrap()),
        "via-proxy"
    );
    assert_eq!(
        proxy
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.target.as_str())
            .collect::<Vec<_>>(),
        vec![TARGET]
    );
}
#[tokio::test(start_paused = true)]
async fn connects_directly_to_a_host_the_policy_does_not_proxy() {
    let proxy = Server::new(|_| Reply::text("via-proxy")).await;
    let origin = Server::new(|_| Reply::text("direct")).await;
    assert_eq!(
        content(
            &fetch(
                &proxy_provider(&proxy.base),
                &format!("{}/page", origin.base)
            )
            .await
            .unwrap()
        ),
        "direct"
    );
    assert_eq!(proxy.count(), 0);
}
#[tokio::test(start_paused = true)]
async fn still_refuses_a_cross_origin_redirect_on_the_proxied_path() {
    let proxy = Server::new(|_| Reply::redirect(302, Some("http://elsewhere.example/next"))).await;
    failure(
        fetch(&proxy_provider(&proxy.base), TARGET)
            .await
            .unwrap_err(),
        "WEB_REDIRECT_BLOCKED",
        None,
    );
    assert_eq!(proxy.count(), 1);
}
#[tokio::test(start_paused = true)]
async fn still_refuses_a_url_the_transport_policy_rejects_before_any_hop() {
    let proxy = Server::new(|_| Reply::text("via-proxy")).await;
    failure(
        fetch(&proxy_provider(&proxy.base), "ftp://example.com/x")
            .await
            .unwrap_err(),
        "WEB_INVALID_URL",
        None,
    );
    assert_eq!(proxy.count(), 0);
}
#[tokio::test(start_paused = true)]
async fn names_a_proxy_transport_failure_without_the_proxy_credentials() {
    let proxy = Server::new(|_| Reply::closed()).await;
    let url = proxy
        .base
        .replace("http://", "http://proxy-user:proxy-secret@");
    failure(
        fetch(&proxy_provider(&url), TARGET).await.unwrap_err(),
        "WEB_FETCH_CONNECTION_INTERRUPTED",
        Some(
            "web fetch failed: the connection was interrupted (UND_ERR_SOCKET): other side closed",
        ),
    );
}
