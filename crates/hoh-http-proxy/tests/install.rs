mod support;

use hoh_http_proxy::policy::{PROXY_ENV_NAMES, ProxySource};
use hoh_http_proxy::*;
use support::{Server, env, inherited, install, lookup};
use url::Url;

const TARGET: &str = "http://origin.test/probe";
const NESTED: &str = "http://127.0.0.1:9";
fn url(value: &str) -> Url {
    Url::parse(value).unwrap()
}
fn direct(route: ProxyRoute) {
    assert!(matches!(route, ProxyRoute::Direct { .. }));
}
fn value<'a>(env: &'a ProxyEnvironment, name: &str) -> Option<&'a str> {
    env.get(name).and_then(Option::as_deref)
}

#[tokio::test]
async fn routes_the_explicitly_shared_fetch_client_through_the_proxy() {
    let proxy = Server::start("VIA-PROXY").await;
    let (installation, _) = install(&[("HTTP_PROXY", &proxy.url()), ("HTTPS_PROXY", &proxy.url())]);
    assert_eq!(
        installation
            .client()
            .get(TARGET)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "VIA-PROXY"
    );
    assert_eq!(proxy.requests(), [format!("GET {TARGET} HTTP/1.1")]);
}

#[tokio::test]
async fn connects_directly_when_the_bypass_list_covers_the_target() {
    let proxy = Server::start("VIA-PROXY").await;
    let origin = Server::start("DIRECT").await;
    let (installation, _) = install(&[("HTTP_PROXY", &proxy.url()), ("NO_PROXY", "origin.test")]);
    let client = route_client_builder(installation.policy().clone(), DispatcherOptions::default())
        .unwrap()
        .resolve("origin.test", origin.address)
        .build()
        .unwrap();
    assert_eq!(
        client
            .get(TARGET)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "DIRECT"
    );
    assert!(proxy.requests().is_empty());
}

#[tokio::test]
async fn reports_a_value_it_cannot_use_and_installs_the_rest() {
    let proxy = Server::start("VIA-PROXY").await;
    let (installation, reported) = install(&[
        ("HTTP_PROXY", &proxy.url()),
        ("HTTPS_PROXY", "socks5://127.0.0.1:1080"),
    ]);
    assert_eq!(reported.len(), 1);
    assert!(reported[0].to_uppercase().contains("HTTPS_PROXY"));
    assert!(reported[0].contains("SOCKS"));
    assert!(!reported[0].contains("1080"));
    assert_eq!(
        installation
            .client()
            .get(TARGET)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "VIA-PROXY"
    );
}

#[test]
fn publishes_the_policy_through_the_proxy_environment_in_both_casings() {
    let (installation, _) = install(&[
        ("HTTP_PROXY", NESTED),
        ("HTTPS_PROXY", NESTED),
        ("NO_PROXY", "example.com"),
    ]);
    for name in ["http_proxy", "HTTP_PROXY"] {
        assert_eq!(value(installation.environment(), name), Some(NESTED));
    }
    for name in ["no_proxy", "NO_PROXY"] {
        assert!(
            value(installation.environment(), name)
                .unwrap()
                .contains("example.com")
        );
    }
}

#[test]
fn removes_an_environment_name_the_policy_leaves_unset() {
    // With no global state to restore, layering and disposal leave the outer installation unchanged.
    let original = inherited(&[("HTTPS_PROXY", "http://stale.example")]);
    let outer = ProxyInstallation::with_inherited_environment(
        &lookup(&env(&[
            ("HTTP_PROXY", NESTED),
            ("HTTPS_PROXY", "http://stale.example"),
        ])),
        original.clone(),
        |_| {},
        DispatcherOptions::default(),
    )
    .unwrap();
    let values = env(&[
        ("HTTP_PROXY", NESTED),
        ("HTTPS_PROXY", "socks5://127.0.0.1:1080"),
    ]);
    let installed = outer
        .layer(&lookup(&values), |_| {}, DispatcherOptions::default())
        .unwrap();
    assert_eq!(installed.environment().get("HTTPS_PROXY"), Some(&None));
    drop(installed);
    assert_eq!(
        value(outer.environment(), "HTTPS_PROXY"),
        Some("http://stale.example")
    );
    assert_eq!(
        outer.policy().https_proxy.as_deref(),
        Some("http://stale.example")
    );
    assert_eq!(
        value(&original, "HTTPS_PROXY"),
        Some("http://stale.example")
    );
}

#[tokio::test]
async fn restores_the_dispatcher_the_route_and_the_environment_on_disposal() {
    // Explicit layering does not mutate the outer installation; disposal exposes that same data.
    let proxy = Server::start("VIA-PROXY").await;
    let origin = Server::start("DIRECT").await;
    let (before, _) = install(&[]);
    let installed = before
        .layer(
            &lookup(&env(&[("HTTP_PROXY", &proxy.url())])),
            |_| {},
            DispatcherOptions::default(),
        )
        .unwrap();
    assert!(matches!(
        proxy_route_for(Some(&installed), &url(TARGET)),
        ProxyRoute::Proxied { .. }
    ));
    drop(installed);
    direct(proxy_route_for(Some(&before), &url(TARGET)));
    assert!(before.environment().is_empty());
    assert_eq!(
        before
            .client()
            .get(origin.url())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "DIRECT"
    );
}

#[test]
fn installs_a_direct_client_and_touches_no_environment_when_the_user_exported_none() {
    let original = inherited(&[("HTTP_PROXY", "http://untouched.example")]);
    let mut reported = Vec::new();
    let installation = ProxyInstallation::with_inherited_environment(
        &lookup(&env(&[])),
        original.clone(),
        |m| reported.push(m.to_owned()),
        DispatcherOptions::default(),
    )
    .unwrap();
    assert!(installation.environment().is_empty());
    assert!(reported.is_empty());
    assert_eq!(
        value(&original, "HTTP_PROXY"),
        Some("http://untouched.example")
    );
    direct(proxy_route_for(Some(&installation), &url(TARGET)));
}

#[tokio::test]
async fn keeps_a_scheme_direct_when_the_policy_refused_the_proxy_the_user_named_for_it() {
    let proxy = Server::start("VIA-PROXY").await;
    let origin = Server::start("not TLS").await;
    let (installation, _) = install(&[
        ("HTTP_PROXY", &proxy.url()),
        ("HTTPS_PROXY", "socks5://127.0.0.1:1080"),
    ]);
    // A local plaintext origin rejects the TLS handshake. DNS is overridden so no resolver is used.
    let client = route_client_builder(installation.policy().clone(), DispatcherOptions::default())
        .unwrap()
        .resolve("refused-scheme.invalid", origin.address)
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .unwrap();
    assert!(
        client
            .get("https://refused-scheme.invalid/")
            .send()
            .await
            .is_err()
    );
    assert!(proxy.requests().is_empty());
    assert_eq!(
        installation
            .client()
            .get(TARGET)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "VIA-PROXY"
    );
    assert_eq!(proxy.requests(), [format!("GET {TARGET} HTTP/1.1")]);
}

#[tokio::test]
async fn carries_the_dispatcher_already_routing_so_a_branch_and_its_request_agree() {
    let proxy = Server::keep_alive("VIA-PROXY").await;
    let (installation, _) = install(&[("HTTP_PROXY", &proxy.url())]);
    let ProxyRoute::Proxied {
        proxy: routed_proxy,
        dispatcher,
    } = proxy_route_for(Some(&installation), &url(TARGET))
    else {
        panic!("expected proxy")
    };
    assert_eq!(routed_proxy, proxy.url());
    for client in [installation.client(), dispatcher.client()] {
        let body = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            client
                .get(TARGET)
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(body, "VIA-PROXY");
    }
    assert_eq!(
        proxy.requests(),
        [
            format!("GET {TARGET} HTTP/1.1"),
            format!("GET {TARGET} HTTP/1.1")
        ]
    );
    assert_eq!(proxy.accepted_connections(), 1);
}

#[tokio::test]
async fn captured_route_finishes_in_flight_request_after_installation_is_dropped() {
    let proxy = Server::paused("VIA-PROXY").await;
    let (installation, _) = install(&[("HTTP_PROXY", &proxy.url())]);
    let ProxyRoute::Proxied {
        proxy: routed_proxy,
        dispatcher,
    } = proxy_route_for(Some(&installation), &url(TARGET))
    else {
        panic!("expected proxied route")
    };
    assert_eq!(routed_proxy, proxy.url());
    let request = tokio::spawn(async move { dispatcher.get(TARGET).send().await });
    proxy.wait_for_request().await;
    drop(installation);
    proxy.respond();
    assert_eq!(
        request.await.unwrap().unwrap().text().await.unwrap(),
        "VIA-PROXY"
    );
    assert_eq!(proxy.requests(), [format!("GET {TARGET} HTTP/1.1")]);
}

#[test]
fn is_direct_for_a_bypassed_url_and_direct_with_nothing_installed() {
    let (installation, _) = install(&[("HTTP_PROXY", NESTED), ("NO_PROXY", "origin.test")]);
    direct(proxy_route_for(Some(&installation), &url(TARGET)));
    direct(proxy_route_for(None, &url(TARGET)));
}

#[test]
fn is_direct_for_a_loopback_url_under_a_policy_that_proxies_everything() {
    let (installation, _) = install(&[("HTTP_PROXY", NESTED), ("HTTPS_PROXY", NESTED)]);
    direct(proxy_route_for(
        Some(&installation),
        &url("http://127.0.0.1:9000/probe"),
    ));
}

#[test]
fn is_empty_when_no_policy_is_installed() {
    assert!(proxy_environment_for_child(None).is_empty());
}

#[test]
fn is_empty_when_the_user_exported_none_so_a_child_sees_no_flag_it_cannot_use() {
    let (installation, _) = install(&[]);
    assert!(proxy_environment_for_child(Some(&installation)).is_empty());
}

#[test]
fn hands_a_child_the_values_the_user_exported_not_this_processes_normalization() {
    let values = env(&[
        ("HTTP_PROXY", NESTED),
        ("https_proxy", "socks5://127.0.0.1:1080"),
        ("NO_PROXY", "example.com"),
    ]);
    let installed = ProxyInstallation::with_inherited_environment(
        &lookup(&values),
        inherited(&[
            ("HTTP_PROXY", NESTED),
            ("https_proxy", "socks5://127.0.0.1:1080"),
        ]),
        |_| {},
        DispatcherOptions::default(),
    )
    .unwrap();
    let child = proxy_environment_for_child(Some(&installed));
    assert_eq!(
        value(&child, "https_proxy"),
        Some("socks5://127.0.0.1:1080")
    );
    assert_eq!(value(&child, "HTTPS_PROXY"), None);
    assert_eq!(value(&child, "HTTP_PROXY"), Some(NESTED));
    for name in ["no_proxy", "NO_PROXY"] {
        assert_eq!(
            value(&child, name),
            Some("example.com,localhost,127.0.0.1,::1,[::1]")
        );
    }
    assert!(!child.contains_key("NODE_USE_ENV_PROXY"));
}

#[test]
fn fills_a_scheme_the_user_named_in_neither_casing_so_a_child_node_is_not_left_direct() {
    let (installation, _) = install(&[("ALL_PROXY", NESTED)]);
    let child = proxy_environment_for_child(Some(&installation));
    for name in ["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY"] {
        assert_eq!(value(&child, name), Some(NESTED));
    }
    assert_eq!(value(&child, "NODE_USE_ENV_PROXY"), Some("1"));
}

fn withheld(refused: &str) {
    let values = env(&[("HTTP_PROXY", NESTED), ("HTTPS_PROXY", refused)]);
    let installation = ProxyInstallation::with_inherited_environment(
        &lookup(&values),
        inherited(&[("HTTP_PROXY", NESTED), ("HTTPS_PROXY", refused)]),
        |_| {},
        DispatcherOptions::default(),
    )
    .unwrap();
    let child = proxy_environment_for_child(Some(&installation));
    assert_eq!(value(&child, "HTTP_PROXY"), Some(NESTED));
    assert_eq!(value(&child, "HTTPS_PROXY"), Some(refused));
    assert!(!child.contains_key("NODE_USE_ENV_PROXY"));
}
#[test]
fn withholds_node_use_env_proxy_when_the_child_receives_socks4_so_a_child_node_still_starts() {
    withheld("socks4://127.0.0.1:1080");
}
#[test]
fn withholds_node_use_env_proxy_when_the_child_receives_ftp_so_a_child_node_still_starts() {
    withheld("ftp://p:1");
}
#[test]
fn withholds_node_use_env_proxy_when_the_child_receives_malformed_so_a_child_node_still_starts() {
    withheld("not a url");
}

#[test]
fn keeps_the_outermost_installs_record_of_what_the_user_exported_across_a_nested_one() {
    let values = env(&[
        ("HTTP_PROXY", "http://outer.test"),
        ("HTTPS_PROXY", "http://outer.test"),
        ("NO_PROXY", "example.com"),
    ]);
    let outer = ProxyInstallation::with_inherited_environment(
        &lookup(&values),
        inherited(&[("HTTP_PROXY", "http://outer.test")]),
        |_| {},
        DispatcherOptions::default(),
    )
    .unwrap();
    let inner = outer
        .layer(
            &lookup(&env(&[("HTTP_PROXY", NESTED), ("HTTPS_PROXY", NESTED)])),
            |_| {},
            DispatcherOptions::default(),
        )
        .unwrap();
    let child = proxy_environment_for_child(Some(&inner));
    for name in ["https_proxy", "HTTPS_PROXY"] {
        assert_eq!(value(&child, name), Some(NESTED));
    }
    assert_eq!(value(&child, "HTTP_PROXY"), Some("http://outer.test"));
    drop(inner);
    let child = proxy_environment_for_child(Some(&outer));
    assert_eq!(value(&child, "HTTP_PROXY"), Some("http://outer.test"));
    assert_eq!(value(&child, "https_proxy"), Some("http://outer.test"));
}

#[tokio::test]
async fn stops_proxying_when_the_mounted_policy_proxies_nothing() {
    let proxy = Server::start("VIA-PROXY").await;
    let origin = Server::start("DIRECT").await;
    let (outer, _) = install(&[("HTTP_PROXY", &proxy.url())]);
    assert_eq!(
        outer
            .client()
            .get(TARGET)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "VIA-PROXY"
    );
    let off = outer
        .layer(&lookup(&env(&[])), |_| {}, DispatcherOptions::default())
        .unwrap();
    assert_eq!(
        off.client()
            .get(origin.url())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "DIRECT"
    );
    direct(proxy_route_for(Some(&off), &url(TARGET)));
    drop(off);
    assert_eq!(
        outer
            .client()
            .get(TARGET)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "VIA-PROXY"
    );
    assert!(matches!(
        proxy_route_for(Some(&outer), &url(TARGET)),
        ProxyRoute::Proxied { .. }
    ));
}

#[test]
fn hands_a_child_the_users_own_values_and_the_outer_normalization_again_afterwards() {
    let original = inherited(&[
        ("HTTP_PROXY", NESTED),
        ("https_proxy", "socks5://127.0.0.1:1080"),
    ]);
    let outer = ProxyInstallation::with_inherited_environment(
        &lookup(&env(&[
            ("HTTP_PROXY", NESTED),
            ("https_proxy", "socks5://127.0.0.1:1080"),
        ])),
        original.clone(),
        |_| {},
        DispatcherOptions::default(),
    )
    .unwrap();
    assert_eq!(value(outer.environment(), "https_proxy"), None);
    assert_eq!(value(outer.environment(), "HTTPS_PROXY"), None);
    let off = outer
        .layer(&lookup(&env(&[])), |_| {}, DispatcherOptions::default())
        .unwrap();
    assert_eq!(off.environment(), &original);
    assert!(proxy_environment_for_child(Some(&off)).is_empty());
    drop(off);
    assert_eq!(value(outer.environment(), "https_proxy"), None);
    assert_eq!(value(outer.environment(), "HTTP_PROXY"), Some(NESTED));
}

#[test]
fn touches_no_environment_when_the_install_underneath_proxied_nothing() {
    let (outer, _) = install(&[]);
    let inner = outer
        .layer(&lookup(&env(&[])), |_| {}, DispatcherOptions::default())
        .unwrap();
    assert_eq!(outer.policy().source, ProxySource::None);
    assert!(outer.environment().is_empty());
    assert!(inner.environment().is_empty());
}

#[test]
fn restores_every_name_from_one_snapshot_taken_before_any_write() {
    let original = inherited(&[
        ("http_proxy", "http://before.example"),
        ("HTTP_PROXY", "http://before.example"),
    ]);
    let installed = ProxyInstallation::with_inherited_environment(
        &lookup(&env(&[("HTTP_PROXY", NESTED)])),
        original.clone(),
        |_| {},
        DispatcherOptions::default(),
    )
    .unwrap();
    assert_eq!(value(installed.environment(), "HTTP_PROXY"), Some(NESTED));
    let off = installed
        .layer(&lookup(&env(&[])), |_| {}, DispatcherOptions::default())
        .unwrap();
    assert_eq!(off.environment(), &original);
}

#[test]
fn names_every_proxy_variable_for_removal_so_a_replay_reaches_its_own_fixture_server() {
    let cleared = cleared_proxy_env();
    assert_eq!(
        cleared.keys().map(String::as_str).collect::<Vec<_>>(),
        PROXY_ENV_NAMES
    );
    assert!(cleared.values().all(Option::is_none));
}
