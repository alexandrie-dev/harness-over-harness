mod support;

use hoh_http_proxy::policy::resolve_proxy_policy;
use hoh_http_proxy::{
    DispatcherOptions, ProxyInstallation, create_route_dispatcher, route_client_builder,
};
use support::{Server, env, install, lookup};

fn assert_h2(values: &[(&str, &str)], allow_h2: Option<bool>) {
    let policy = resolve_proxy_policy(&lookup(&env(values))).policy;
    let builder = route_client_builder(policy, DispatcherOptions { allow_h2 });
    assert_eq!(
        format!("{builder:?}").contains("http1_only: true"),
        allow_h2 == Some(false)
    );
}

#[test]
fn keeps_undicis_http2_default_when_installation_carries_no_override() {
    let values = [("HTTP_PROXY", "http://127.0.0.1:9")];
    assert_h2(&values, None);
    let (_, reports) = install(&values);
    assert!(reports.is_empty());
}

#[tokio::test]
async fn pins_an_installed_dispatcher_to_http1_when_the_caller_passes_allow_h2_false() {
    let proxy = Server::start("VIA-PROXY").await;
    let origin = Server::start("DIRECT").await;
    let proxy_url = proxy.url();
    let values = env(&[("HTTP_PROXY", &proxy_url)]);
    let options = DispatcherOptions {
        allow_h2: Some(false),
    };
    assert_h2(&[("HTTP_PROXY", &proxy_url)], Some(false));
    let installed = ProxyInstallation::with_inherited_environment(
        &lookup(&values),
        Default::default(),
        |_| {},
        options,
    )
    .unwrap();
    for (target, body) in [
        ("http://proxied.test/".to_owned(), "VIA-PROXY"),
        (origin.url(), "DIRECT"),
    ] {
        let response = installed.client().get(target).send().await.unwrap();
        assert_eq!(response.version(), reqwest::Version::HTTP_11);
        assert_eq!(response.text().await.unwrap(), body);
    }
    assert_eq!(proxy.requests(), ["GET http://proxied.test/ HTTP/1.1"]);
}

#[tokio::test]
async fn builds_a_per_request_dispatcher_pinned_to_http1() {
    let proxy = Server::start("VIA-PROXY").await;
    let origin = Server::start("DIRECT").await;
    let proxy_url = proxy.url();
    let values = env(&[("HTTP_PROXY", &proxy_url)]);
    assert_h2(&[("HTTP_PROXY", &proxy_url)], Some(false));
    let dispatcher = create_route_dispatcher(
        &lookup(&values),
        DispatcherOptions {
            allow_h2: Some(false),
        },
    )
    .unwrap();
    for target in ["http://proxied.test/".to_owned(), origin.url()] {
        assert_eq!(
            dispatcher.get(target).send().await.unwrap().version(),
            reqwest::Version::HTTP_11
        );
    }
    assert_eq!(proxy.requests().len(), 1);
    assert_eq!(origin.requests().len(), 1);
}

#[test]
fn keeps_http2_enabled_when_the_caller_opts_in() {
    for values in [vec![("HTTPS_PROXY", "http://127.0.0.1:9")], vec![]] {
        assert_h2(&values, Some(true));
        create_route_dispatcher(
            &lookup(&env(&values)),
            DispatcherOptions {
                allow_h2: Some(true),
            },
        )
        .unwrap();
    }
}

#[test]
fn pins_a_direct_per_request_dispatcher_to_http1() {
    assert_h2(&[], Some(false));
    create_route_dispatcher(
        &lookup(&env(&[])),
        DispatcherOptions {
            allow_h2: Some(false),
        },
    )
    .unwrap();
}

#[test]
fn keeps_undicis_http2_default_for_a_direct_per_request_dispatcher() {
    assert_h2(&[], None);
    create_route_dispatcher(&lookup(&env(&[])), DispatcherOptions::default()).unwrap();
}

#[test]
fn pins_the_direct_dispatcher_of_a_policy_layered_over_an_installed_proxy() {
    let (outer, _) = install(&[("HTTP_PROXY", "http://127.0.0.1:9")]);
    let inner = outer
        .layer(
            &lookup(&env(&[])),
            |_| {},
            DispatcherOptions {
                allow_h2: Some(false),
            },
        )
        .unwrap();
    assert_h2(&[], Some(false));
    assert_eq!(
        inner.policy().source,
        hoh_http_proxy::policy::ProxySource::None
    );
}
