mod support;

use hoh_http_proxy::policy::{proxy_for_url, resolve_proxy_policy};
use hoh_http_proxy::{DispatcherOptions, route_client_builder};
use support::{Server, env, lookup};
use url::Url;

async fn parity(no_proxy: &str, path: &str, bypassed: bool) {
    let proxy = Server::start("VIA-PROXY").await;
    let origin = Server::start("DIRECT").await;
    let proxy_url = proxy.url();
    let values = env(&[
        ("HTTP_PROXY", &proxy_url),
        ("HTTPS_PROXY", &proxy_url),
        ("NO_PROXY", no_proxy),
    ]);
    let policy = resolve_proxy_policy(&lookup(&values)).policy;
    let url = Url::parse(&format!("http://probe.invalid{path}")).unwrap();
    let client = route_client_builder(policy.clone(), DispatcherOptions::default())
        .unwrap()
        .resolve("probe.invalid", origin.address)
        .build()
        .unwrap();
    let body = client
        .get(url.clone())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        (
            proxy_for_url(&policy, &url).is_some(),
            !proxy.requests().is_empty()
        ),
        (!bypassed, !bypassed)
    );
    assert_eq!(body, if bypassed { "DIRECT" } else { "VIA-PROXY" });
    assert_eq!(origin.requests().len(), usize::from(bypassed));
}

macro_rules! parity_case {
    ($name:ident, $list:literal, $path:literal, $bypassed:expr) => {
        #[tokio::test]
        async fn $name() {
            parity($list, $path, $bypassed).await;
        }
    };
}
parity_case!(agrees_on_empty_for_plain, "", "/plain", false);
parity_case!(
    agrees_on_exact_host_for_exact,
    "probe.invalid",
    "/exact",
    true
);
parity_case!(
    agrees_on_dot_suffix_for_dot_suffix,
    ".probe.invalid",
    "/dot-suffix",
    true
);
parity_case!(
    agrees_on_star_suffix_for_star_suffix,
    "*.probe.invalid",
    "/star-suffix",
    true
);
parity_case!(
    agrees_on_other_host_for_miss,
    "other.invalid",
    "/miss",
    false
);
parity_case!(agrees_on_star_for_all, "*", "/all", true);
parity_case!(
    agrees_on_default_port_for_with_default_port,
    "probe.invalid:80",
    "/with-default-port",
    true
);
parity_case!(
    agrees_on_wrong_port_for_wrong_port,
    "probe.invalid:8443",
    "/wrong-port",
    false
);
parity_case!(
    agrees_on_comma_list_for_comma_list,
    "a.invalid, probe.invalid",
    "/comma-list",
    true
);
