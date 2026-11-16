mod support;

use hoh_http_proxy::policy::*;
use support::{env, lookup};
use url::Url;

const PROXY: &str = "http://127.0.0.1:7897";
const OTHER: &str = "http://127.0.0.1:8080";

fn resolve(values: &[(&str, &str)]) -> ProxyResolution {
    resolve_proxy_policy(&lookup(&env(values)))
}
fn url(value: &str) -> Url {
    Url::parse(value).unwrap()
}
fn proxied() -> ProxyPolicy {
    ProxyPolicy {
        http_proxy: Some(PROXY.into()),
        https_proxy: Some(PROXY.into()),
        no_proxy: String::new(),
        source: ProxySource::Env,
    }
}

macro_rules! loopback_case {
    ($name:ident, $host:literal, $expected:expr) => {
        #[test]
        fn $name() {
            assert_eq!(
                proxy_for_url(&proxied(), &url(concat!("http://", $host, ":8080/"))),
                $expected
            );
        }
    };
}
loopback_case!(never_routes_127_0_0_1_through_a_proxy, "127.0.0.1", None);
loopback_case!(never_routes_127_0_0_2_through_a_proxy, "127.0.0.2", None);
loopback_case!(never_routes_127_0_0_53_through_a_proxy, "127.0.0.53", None);
loopback_case!(
    never_routes_127_255_255_254_through_a_proxy,
    "127.255.255.254",
    None
);
loopback_case!(never_routes_localhost_through_a_proxy, "localhost", None);
loopback_case!(
    never_routes_app_localhost_through_a_proxy,
    "app.localhost",
    None
);
loopback_case!(never_routes_ipv6_loopback_through_a_proxy, "[::1]", None);
loopback_case!(
    never_routes_mapped_loopback_through_a_proxy,
    "[::ffff:127.0.0.1]",
    None
);
loopback_case!(never_routes_unspecified_through_a_proxy, "0.0.0.0", None);
loopback_case!(
    still_routes_128_0_0_1_which_is_not_this_machine,
    "128.0.0.1",
    Some(PROXY)
);
loopback_case!(
    still_routes_10_0_0_5_which_is_not_this_machine,
    "10.0.0.5",
    Some(PROXY)
);
loopback_case!(
    still_routes_mapped_private_which_is_not_this_machine,
    "[::ffff:10.0.0.1]",
    Some(PROXY)
);
loopback_case!(
    still_routes_notlocalhost_which_is_not_this_machine,
    "notlocalhost",
    Some(PROXY)
);
loopback_case!(
    still_routes_example_com_which_is_not_this_machine,
    "example.com",
    Some(PROXY)
);

#[test]
fn rejects_an_out_of_range_octet_rather_than_reading_it_as_loopback() {
    assert!(!is_loopback_host("127.999.1.1"));
    assert!(!is_loopback_host("1270.0.0.1"));
}
#[test]
fn reads_an_ipv4_mapped_address_in_either_spelling() {
    for (host, expected) in [
        ("::ffff:127.0.0.1", true),
        ("::ffff:7f00:1", true),
        ("::ffff:10.0.0.1", false),
        ("::ffff:a00:1", false),
    ] {
        assert_eq!(is_loopback_host(host), expected);
    }
}
#[test]
fn resolves_nothing_when_the_environment_carries_no_proxy() {
    assert_eq!(
        resolve(&[]),
        ProxyResolution {
            policy: DIRECT_POLICY,
            diagnostics: vec![]
        }
    );
}
#[test]
fn reads_both_schemes_and_merges_loopback_into_the_bypass_list() {
    let p = resolve(&[
        ("HTTP_PROXY", PROXY),
        ("HTTPS_PROXY", OTHER),
        ("NO_PROXY", "example.com"),
    ])
    .policy;
    assert_eq!(p.http_proxy.as_deref(), Some(PROXY));
    assert_eq!(p.https_proxy.as_deref(), Some(OTHER));
    assert_eq!(p.no_proxy, "example.com,localhost,127.0.0.1,::1,[::1]");
    assert_eq!(p.source, ProxySource::Env);
}
#[test]
fn prefers_the_lowercase_name_matching_undici() {
    assert_eq!(
        resolve(&[("http_proxy", PROXY), ("HTTP_PROXY", OTHER)])
            .policy
            .http_proxy
            .as_deref(),
        Some(if cfg!(windows) { OTHER } else { PROXY })
    );
}
#[test]
fn treats_a_blank_lowercase_value_as_unset_instead_of_letting_it_shadow_the_uppercase_one() {
    assert_eq!(
        resolve(&[("http_proxy", "   "), ("HTTP_PROXY", PROXY)])
            .policy
            .http_proxy
            .as_deref(),
        Some(PROXY)
    );
}
#[test]
fn leaves_http_direct_when_only_the_https_variable_is_set() {
    let p = resolve(&[("HTTPS_PROXY", PROXY)]).policy;
    assert_eq!(p.http_proxy, None);
    assert_eq!(p.https_proxy.as_deref(), Some(PROXY));
    assert_eq!(proxy_for_url(&p, &url("http://example.com/")), None);
    assert_eq!(proxy_for_url(&p, &url("https://example.com/")), Some(PROXY));
}
#[test]
fn backs_both_schemes_with_all_proxy_which_neither_node_nor_undici_reads() {
    let p = resolve(&[("ALL_PROXY", PROXY)]).policy;
    assert_eq!(p.http_proxy.as_deref(), Some(PROXY));
    assert_eq!(p.https_proxy.as_deref(), Some(PROXY));
}
#[test]
fn lets_a_scheme_specific_value_outrank_all_proxy() {
    let p = resolve(&[("ALL_PROXY", PROXY), ("HTTPS_PROXY", OTHER)]).policy;
    assert_eq!(p.http_proxy.as_deref(), Some(PROXY));
    assert_eq!(p.https_proxy.as_deref(), Some(OTHER));
}
#[test]
fn falls_https_back_to_the_http_proxy_last_so_the_dispatcher_and_proxy_for_url_agree() {
    assert_eq!(
        resolve(&[("HTTP_PROXY", PROXY)])
            .policy
            .https_proxy
            .as_deref(),
        Some(PROXY)
    );
}
#[test]
fn keeps_a_bypass_list_of_star_unchanged() {
    assert_eq!(
        resolve(&[("HTTP_PROXY", PROXY), ("NO_PROXY", "*")])
            .policy
            .no_proxy,
        "*"
    );
}
#[test]
fn does_not_repeat_a_loopback_entry_the_user_already_listed() {
    assert_eq!(
        resolve(&[("HTTP_PROXY", PROXY), ("NO_PROXY", "localhost, 127.0.0.1")])
            .policy
            .no_proxy,
        "localhost,127.0.0.1,::1,[::1]"
    );
}
#[test]
fn keeps_a_scheme_direct_when_its_own_value_was_refused_rather_than_falling_back() {
    let r = resolve(&[
        ("HTTPS_PROXY", "socks5://127.0.0.1:1080"),
        ("HTTP_PROXY", PROXY),
    ]);
    assert_eq!(r.policy.http_proxy.as_deref(), Some(PROXY));
    assert_eq!(r.policy.https_proxy, None);
    assert_eq!(proxy_for_url(&r.policy, &url("https://example.com/")), None);
    assert!(
        r.diagnostics[0]
            .message
            .contains("connecting directly for that scheme")
    );
}
#[test]
fn keeps_a_scheme_direct_when_its_own_value_was_malformed_past_all_proxy_too() {
    let p = resolve(&[("HTTPS_PROXY", "not a url"), ("ALL_PROXY", PROXY)]).policy;
    assert_eq!(p.http_proxy.as_deref(), Some(PROXY));
    assert_eq!(p.https_proxy, None);
}
#[test]
fn reports_a_socks_proxy_instead_of_silently_ignoring_it() {
    let r = resolve(&[("HTTP_PROXY", "socks5://127.0.0.1:7890")]);
    assert_eq!(r.policy, DIRECT_POLICY);
    assert_eq!(r.diagnostics.len(), 1);
    assert_eq!(r.diagnostics[0].kind, ProxyDiagnosticKind::Socks);
    assert!(
        r.diagnostics[0]
            .message
            .contains("SOCKS proxy, which is not supported")
    );
}
#[test]
fn reports_an_unparseable_proxy_url() {
    let r = resolve(&[("HTTP_PROXY", "not a url")]);
    assert_eq!(r.policy, DIRECT_POLICY);
    assert_eq!(r.diagnostics[0].kind, ProxyDiagnosticKind::Invalid);
    assert_eq!(
        r.diagnostics[0].origin,
        if cfg!(windows) {
            "http_proxy"
        } else {
            "HTTP_PROXY"
        }
    );
}
#[test]
fn reports_a_proxy_url_whose_scheme_is_neither_http_s_nor_socks() {
    assert!(
        resolve(&[("HTTP_PROXY", "ftp://proxy.example")]).diagnostics[0]
            .message
            .contains("unsupported ftp:// scheme")
    );
}

macro_rules! bypass_case {
    ($name:ident, $list:literal, $url:literal, $expected:expr) => {
        #[test]
        fn $name() {
            assert_eq!(bypasses_proxy($list, &url($url)), $expected);
        }
    };
}
bypass_case!(
    bypass_exact_host,
    "example.com",
    "http://example.com/a",
    true
);
bypass_case!(
    bypass_subdomain,
    "example.com",
    "http://sub.example.com/a",
    true
);
bypass_case!(
    bypass_does_not_match_partial_label,
    "example.com",
    "http://notexample.com/a",
    false
);
bypass_case!(
    bypass_dot_suffix,
    ".example.com",
    "http://sub.example.com/a",
    true
);
bypass_case!(
    bypass_star_suffix,
    "*.example.com",
    "http://sub.example.com/a",
    true
);
bypass_case!(
    bypass_trailing_dot,
    "example.com",
    "http://example.com./a",
    true
);
bypass_case!(bypass_everything, "*", "http://anything.example/a", true);
bypass_case!(
    bypass_matching_port,
    "example.com:8080",
    "http://example.com:8080/a",
    true
);
bypass_case!(
    bypass_wrong_port,
    "example.com:8080",
    "http://example.com:9090/a",
    false
);
bypass_case!(
    bypass_default_http_port,
    "example.com:80",
    "http://example.com/a",
    true
);
bypass_case!(
    bypass_default_https_port,
    "example.com:443",
    "https://example.com/a",
    true
);
bypass_case!(
    bypass_case_insensitive,
    "EXAMPLE.com",
    "http://example.COM/a",
    true
);
bypass_case!(
    bypass_localhost,
    "localhost",
    "http://localhost:3000/a",
    true
);
bypass_case!(
    bypass_ipv4_literal,
    "127.0.0.1",
    "http://127.0.0.1:7777/a",
    true
);
bypass_case!(
    bypasses_a_bare_ipv6_loopback_which_undici_reads_as_host_colon_port_1,
    "::1",
    "http://[::1]:3000/a",
    true
);
bypass_case!(
    bypasses_the_bracketed_ipv6_form_as_well,
    "[::1]",
    "http://[::1]/a",
    true
);
#[test]
fn honors_a_port_on_a_bracketed_ipv6_entry() {
    assert!(bypasses_proxy("[::1]:3000", &url("http://[::1]:3000/a")));
    assert!(!bypasses_proxy("[::1]:3000", &url("http://[::1]:4000/a")));
}
bypass_case!(
    does_not_match_cidr_notation_so_an_os_bypass_list_must_be_rewritten_as_suffixes,
    "10.0.0.0/8",
    "http://10.1.2.3/a",
    false
);
bypass_case!(
    skips_blank_and_bracket_only_entries,
    " , , [ , .",
    "http://example.com/a",
    false
);

fn routing_policy() -> ProxyPolicy {
    resolve(&[
        ("HTTP_PROXY", PROXY),
        ("HTTPS_PROXY", OTHER),
        ("NO_PROXY", "direct.example"),
    ])
    .policy
}
#[test]
fn routes_http_through_the_http_proxy() {
    assert_eq!(
        proxy_for_url(&routing_policy(), &url("http://example.com/")),
        Some(PROXY)
    );
}
#[test]
fn routes_https_through_the_https_proxy() {
    assert_eq!(
        proxy_for_url(&routing_policy(), &url("https://example.com/")),
        Some(OTHER)
    );
}
#[test]
fn returns_nothing_for_a_bypassed_host() {
    assert_eq!(
        proxy_for_url(&routing_policy(), &url("https://direct.example/")),
        None
    );
}
#[test]
fn returns_nothing_for_loopback_which_is_always_bypassed() {
    assert_eq!(
        proxy_for_url(&routing_policy(), &url("http://127.0.0.1:9000/")),
        None
    );
}
#[test]
fn returns_nothing_for_a_non_http_scheme() {
    assert_eq!(
        proxy_for_url(&routing_policy(), &url("ws://example.com/")),
        None
    );
}
#[test]
fn returns_nothing_under_a_direct_policy() {
    assert_eq!(
        proxy_for_url(&DIRECT_POLICY, &url("https://example.com/")),
        None
    );
}

#[test]
fn preserves_every_diagnostic_text_and_candidate_order_without_credentials() {
    let r = resolve(&[
        ("ALL_PROXY", "socks5://user:secret@proxy.test:1080"),
        ("HTTP_PROXY", "bad"),
        ("HTTPS_PROXY", "pac+http://proxy.test/config"),
    ]);
    let names = if cfg!(windows) {
        ["all_proxy", "http_proxy", "https_proxy"]
    } else {
        ["ALL_PROXY", "HTTP_PROXY", "HTTPS_PROXY"]
    };
    assert_eq!(
        r.diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>(),
        vec![
            format!(
                "{} names a SOCKS proxy, which is not supported; connecting directly for that scheme — set an http:// or https:// proxy URL instead",
                names[0]
            ),
            format!("{} is not a valid URL; connecting directly", names[1]),
            format!(
                "{} uses the unsupported pac+http:// scheme; connecting directly for that scheme — set an http:// or https:// proxy URL instead",
                names[2]
            ),
        ]
    );
    assert_eq!(r.policy, DIRECT_POLICY);
}

#[test]
fn keeps_source_ecmascript_whitespace_and_loopback_boundaries() {
    assert_eq!(
        resolve(&[
            ("HTTP_PROXY", "\u{feff}http://proxy.test\u{feff}"),
            ("NO_PROXY", "LOCALHOST\u{feff}example.com")
        ])
        .policy
        .no_proxy,
        "LOCALHOST,example.com,127.0.0.1,::1,[::1]"
    );
    for host in ["::", "0.0.0.0", "LOCALHOST.", "[::1]", "::ffff:7fff:ffff"] {
        assert!(is_loopback_host(host), "{host}");
    }
    for host in ["127.00.0.1", "127.256.0.1", "::ffff:8000:1", "localhost.."] {
        assert!(!is_loopback_host(host), "{host}");
    }
    for scheme in ["socks", "socks4", "socks4a", "socks5", "socks5h"] {
        assert_eq!(
            resolve(&[("HTTP_PROXY", &format!("{scheme}://proxy.test"))]).diagnostics[0].kind,
            ProxyDiagnosticKind::Socks
        );
    }
}
