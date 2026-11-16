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
        let builder = route_client_builder(policy, DispatcherOptions::default()).unwrap();
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
    assert_eq!(per_request.diagnostics().len(), 1);
    assert_eq!(
        per_request.diagnostics()[0].kind,
        hoh_http_proxy::policy::ProxyDiagnosticKind::Invalid
    );
    assert_eq!(per_request.diagnostics()[0].origin, name);
    assert_eq!(per_request.diagnostics()[0].message, reported[0]);
    assert!(matches!(
        per_request.route_for(&Url::parse("http://origin.test/").unwrap()),
        hoh_http_proxy::ProxyRoute::Direct { .. }
    ));
}

#[test]
fn rejects_authority_characters_accepted_by_url_but_not_http_uri() {
    for character in ['"', '`', '{', '}'] {
        let proxy = format!("http://proxy{character}name.test:3128/");
        let parsed = Url::parse(&proxy).unwrap();
        assert!(parsed.as_str().parse::<http::Uri>().is_err());
        let values = env(&[("HTTPS_PROXY", &proxy), ("ALL_PROXY", AUTHENTICATED)]);
        assert_eq!(
            resolve_proxy_policy(&lookup(&values))
                .policy
                .https_proxy
                .as_deref(),
            Some(proxy.as_str())
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
            "https_proxy"
        } else {
            "HTTPS_PROXY"
        };
        assert_eq!(
            reported,
            [format!("{name} is not a valid URL; connecting directly")]
        );
        assert_eq!(installed.policy().https_proxy, None);
        assert_eq!(
            installed.policy().http_proxy.as_deref(),
            Some(AUTHENTICATED)
        );
        assert!(matches!(
            proxy_route_for(
                Some(&installed),
                &Url::parse("https://origin.test/").unwrap()
            ),
            hoh_http_proxy::ProxyRoute::Direct { .. }
        ));
        let dispatcher =
            hoh_http_proxy::create_route_dispatcher(&lookup(&values), DispatcherOptions::default())
                .unwrap();
        assert_eq!(dispatcher.diagnostics().len(), 1);
        assert_eq!(dispatcher.diagnostics()[0].message, reported[0]);
        assert!(matches!(
            dispatcher.route_for(&Url::parse("https://origin.test/").unwrap()),
            hoh_http_proxy::ProxyRoute::Direct { .. }
        ));
    }
}

#[test]
fn policy_client_builder_refuses_unusable_urls_before_building_a_client() {
    for proxy in [
        format!("http://proxy.test/{}", "a".repeat(65_535)),
        "http://proxy{bad}.test/".to_owned(),
    ] {
        let policy = resolve_proxy_policy(&lookup(&env(&[("HTTP_PROXY", &proxy)]))).policy;
        let error = route_client_builder(policy, DispatcherOptions::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "http_proxy is not a valid URL; connecting directly"
        );
    }
}

#[test]
fn transport_validation_preserves_normalization_credentials_and_uri_length_boundary() {
    let prefix = "http://proxy.test/";
    let maximum = format!("{prefix}{}", "a".repeat(65_534 - prefix.len()));
    let mut accepted = vec![maximum];
    accepted.extend(
        [
            " HTTP://USER:p%40ss@PROXY.test:80/path?query=1#fragment ",
            "https://user:password@[::1]:3128/",
            "http://例子.test/",
            "http://proxy_name.test/",
            "http://proxy.test/a|b^c?query=`{}",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    for proxy in accepted {
        let values = env(&[("HTTP_PROXY", &proxy)]);
        let policy = resolve_proxy_policy(&lookup(&values)).policy;
        let mut reported = Vec::new();
        let installed = ProxyInstallation::with_inherited_environment(
            &lookup(&values),
            Default::default(),
            |message| reported.push(message.to_owned()),
            DispatcherOptions::default(),
        )
        .unwrap();
        assert!(reported.is_empty());
        assert_eq!(installed.policy(), &policy);
        route_client_builder(policy, DispatcherOptions::default())
            .unwrap()
            .build()
            .unwrap();
    }
}
