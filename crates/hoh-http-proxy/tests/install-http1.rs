mod support;

use hoh_http_proxy::policy::DIRECT_POLICY;
use hoh_http_proxy::{
    DispatcherOptions, ProxyInstallation, ProxyRoute, RouteDispatcher, create_route_dispatcher,
    proxy_route_for, route_client_builder,
};
use support::{env, install, lookup};
use url::Url;

fn check_built_pin(options: DispatcherOptions) {
    // Inspect the shared builder using the object's stored record, as well as its metadata.
    let builder = route_client_builder(DIRECT_POLICY, options).unwrap();
    assert_eq!(
        format!("{builder:?}").contains("http1_only: true"),
        options.allow_h2 == Some(false),
        "client builder must apply the stored transport options: {options:?}"
    );
}

fn check_dispatcher(dispatcher: &RouteDispatcher, allow_h2: Option<bool>) {
    assert_eq!(dispatcher.options(), DispatcherOptions { allow_h2 });
    assert_eq!(dispatcher.http1_only(), allow_h2 == Some(false));
    check_built_pin(dispatcher.options());
    for target in ["https://proxied.test/", "http://direct.test/"] {
        let route = dispatcher.route_for(&Url::parse(target).unwrap());
        assert_eq!(route.options(), DispatcherOptions { allow_h2 });
        assert_eq!(route.http1_only(), allow_h2 == Some(false));
        if let ProxyRoute::Proxied {
            dispatcher: carried,
            ..
        } = route
        {
            assert_eq!(carried.options(), dispatcher.options());
            assert_eq!(carried.http1_only(), dispatcher.http1_only());
        }
    }
}

fn check_installation(installed: &ProxyInstallation, allow_h2: Option<bool>) {
    assert_eq!(installed.options(), DispatcherOptions { allow_h2 });
    assert_eq!(installed.http1_only(), allow_h2 == Some(false));
    check_built_pin(installed.options());
    for target in ["https://proxied.test/", "http://direct.test/"] {
        let route = proxy_route_for(Some(installed), &Url::parse(target).unwrap());
        assert_eq!(route.options(), DispatcherOptions { allow_h2 });
        assert_eq!(route.http1_only(), allow_h2 == Some(false));
    }
}

#[test]
fn keeps_undicis_http2_default_when_installation_carries_no_override() {
    let (installed, reports) = install(&[("HTTP_PROXY", "http://127.0.0.1:9")]);
    check_installation(&installed, None);
    assert!(reports.is_empty());
}

#[test]
fn pins_an_installed_dispatcher_to_http1_when_the_caller_passes_allow_h2_false() {
    let values = env(&[("HTTPS_PROXY", "http://127.0.0.1:9")]);
    let installed = ProxyInstallation::with_inherited_environment(
        &lookup(&values),
        Default::default(),
        |_| {},
        DispatcherOptions {
            allow_h2: Some(false),
        },
    )
    .unwrap();
    check_installation(&installed, Some(false));
    assert!(matches!(
        proxy_route_for(
            Some(&installed),
            &Url::parse("https://proxied.test/").unwrap()
        ),
        ProxyRoute::Proxied { .. }
    ));
    assert!(matches!(
        proxy_route_for(
            Some(&installed),
            &Url::parse("http://direct.test/").unwrap()
        ),
        ProxyRoute::Direct { .. }
    ));
}

#[test]
fn builds_a_per_request_dispatcher_pinned_to_http1() {
    let values = env(&[("HTTPS_PROXY", "http://127.0.0.1:9")]);
    let dispatcher = create_route_dispatcher(
        &lookup(&values),
        DispatcherOptions {
            allow_h2: Some(false),
        },
    )
    .unwrap();
    check_dispatcher(&dispatcher, Some(false));
}

#[test]
fn keeps_http2_enabled_when_the_caller_opts_in() {
    for values in [vec![("HTTPS_PROXY", "http://127.0.0.1:9")], vec![]] {
        let dispatcher = create_route_dispatcher(
            &lookup(&env(&values)),
            DispatcherOptions {
                allow_h2: Some(true),
            },
        )
        .unwrap();
        check_dispatcher(&dispatcher, Some(true));
    }
}

#[test]
fn pins_a_direct_per_request_dispatcher_to_http1() {
    let dispatcher = create_route_dispatcher(
        &lookup(&env(&[])),
        DispatcherOptions {
            allow_h2: Some(false),
        },
    )
    .unwrap();
    check_dispatcher(&dispatcher, Some(false));
}

#[test]
fn keeps_undicis_http2_default_for_a_direct_per_request_dispatcher() {
    let dispatcher =
        create_route_dispatcher(&lookup(&env(&[])), DispatcherOptions::default()).unwrap();
    check_dispatcher(&dispatcher, None);
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
    check_installation(&inner, Some(false));
    check_installation(&outer, None);
    assert!(matches!(
        proxy_route_for(Some(&inner), &Url::parse("https://proxied.test/").unwrap()),
        ProxyRoute::Direct { .. }
    ));
}
