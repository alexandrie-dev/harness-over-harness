mod support;

use hoh_http_proxy::policy::{DIRECT_POLICY, resolve_proxy_policy};
use hoh_http_proxy::{
    DispatcherOptions, ProxyInstallation, proxy_environment_for_child, proxy_route_for,
    route_client_builder,
};
use support::{env, inherited, lookup};
use url::Url;

const AUTHENTICATED: &str = "http://user:private-password@proxy.test:3128";

fn authenticated_installation() -> ProxyInstallation {
    ProxyInstallation::with_inherited_environment(
        &lookup(&env(&[("HTTP_PROXY", AUTHENTICATED)])),
        inherited(&[
            ("HTTP_PROXY", AUTHENTICATED),
            ("HTTPS_PROXY", "http://other:inherited-secret@bad host"),
        ]),
        |_| {},
        DispatcherOptions::default(),
    )
    .unwrap()
}

#[test]
fn client_builder_uses_fetch_transport_defaults() {
    for policy in [
        DIRECT_POLICY,
        resolve_proxy_policy(&lookup(&env(&[("HTTP_PROXY", AUTHENTICATED)]))).policy,
    ] {
        let builder = route_client_builder(policy, DispatcherOptions::default());
        let settings = format!("{builder:?}");
        assert!(settings.contains("connect_timeout: 10s"));
        assert!(settings.contains("redirect_policy: Policy(Limit(20))"));
        assert!(!settings.contains("referer: true"));
        // reqwest's builder Debug omits read_timeout; its built client exposes it.
        assert!(format!("{:?}", builder.build().unwrap()).contains("read_timeout: 300s"));
    }
}

#[test]
fn policy_debug_redacts_password_without_changing_values() {
    let policy = resolve_proxy_policy(&lookup(&env(&[("HTTP_PROXY", AUTHENTICATED)]))).policy;
    let debug = format!("{policy:?}");
    assert!(!debug.contains("private-password"));
    assert!(debug.contains("http://user:***@proxy.test:3128"));
    assert_eq!(policy.http_proxy.as_deref(), Some(AUTHENTICATED));
}

#[test]
fn installation_debug_redacts_url_and_inherited_environment_passwords() {
    let installed = authenticated_installation();
    let debug = format!("{installed:?}");
    assert!(!debug.contains("private-password"));
    assert!(!debug.contains("inherited-secret"));
    assert!(debug.contains("http://user:***@proxy.test:3128"));
    let child = proxy_environment_for_child(Some(&installed));
    assert_eq!(child["HTTP_PROXY"].as_deref(), Some(AUTHENTICATED));
    assert_eq!(
        child["HTTPS_PROXY"].as_deref(),
        Some("http://other:inherited-secret@bad host")
    );
}

#[test]
fn route_debug_redacts_password_without_changing_proxy_value() {
    let installed = authenticated_installation();
    let route = proxy_route_for(
        Some(&installed),
        &Url::parse("https://origin.test/").unwrap(),
    );
    let debug = format!("{route:?}");
    assert!(!debug.contains("private-password"));
    assert!(debug.contains("http://user:***@proxy.test:3128"));
    let hoh_http_proxy::ProxyRoute::Proxied { proxy, .. } = route else {
        panic!("expected proxy")
    };
    assert_eq!(proxy, AUTHENTICATED);
}

#[test]
fn debug_redacts_password_in_malformed_inherited_url() {
    let inherited_value = "http://other:malformed-secret/segment@bad host";
    let installed = ProxyInstallation::with_inherited_environment(
        &lookup(&env(&[("ALL_PROXY", AUTHENTICATED)])),
        inherited(&[("HTTPS_PROXY", inherited_value)]),
        |_| {},
        DispatcherOptions::default(),
    )
    .unwrap();
    assert!(!format!("{installed:?}").contains("malformed-secret"));
    assert_eq!(
        proxy_environment_for_child(Some(&installed))["HTTPS_PROXY"].as_deref(),
        Some(inherited_value)
    );
}

#[test]
fn rejects_a_url_the_policy_accepts_but_reqwests_uri_matcher_would_drop() {
    let overlong = format!(
        "http://user:private-password@proxy.test/{}",
        "a".repeat(65_535)
    );
    let values = env(&[
        ("HTTP_PROXY", &overlong),
        ("ALL_PROXY", "http://fallback.test:3128"),
    ]);
    assert_eq!(
        resolve_proxy_policy(&lookup(&values))
            .policy
            .http_proxy
            .as_deref(),
        Some(overlong.as_str())
    );
    let mut reported = Vec::new();
    let installed = ProxyInstallation::with_inherited_environment(
        &lookup(&values),
        Default::default(),
        |message| reported.push(message.to_owned()),
        DispatcherOptions::default(),
    )
    .unwrap();
    let name = if cfg!(windows) {
        "http_proxy"
    } else {
        "HTTP_PROXY"
    };
    assert_eq!(
        reported,
        [format!("{name} is not a valid URL; connecting directly")]
    );
    assert_eq!(installed.policy().http_proxy, None);
    assert_eq!(
        installed.policy().https_proxy.as_deref(),
        Some("http://fallback.test:3128")
    );
    assert!(matches!(
        proxy_route_for(
            Some(&installed),
            &Url::parse("http://origin.test/").unwrap()
        ),
        hoh_http_proxy::ProxyRoute::Direct { .. }
    ));
    let per_request =
        hoh_http_proxy::create_route_dispatcher(&lookup(&values), DispatcherOptions::default())
            .unwrap();
    assert!(matches!(
        per_request.route_for(&Url::parse("http://origin.test/").unwrap()),
        hoh_http_proxy::ProxyRoute::Direct { .. }
    ));
}
