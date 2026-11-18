use hoh_llm::*;
const FORK: AppIdentity<'static> = AppIdentity {
    product: "fork-agent",
    version: "9.9.9",
    url: "https://example.com/fork-agent",
    suffix: None,
};
#[test]
fn sources_the_version_from_the_package_manifest_never_a_hand_copied_constant() {
    assert_eq!(APP_IDENTITY.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(APP_IDENTITY.version, "0.2.2-rc.3");
}
#[test]
fn carries_only_static_public_product_facts() {
    assert_eq!(APP_IDENTITY.product, "deepseek-harness");
    assert_eq!(
        APP_IDENTITY.url,
        "https://github.com/deepseek-ai/deepseek-harness"
    );
    assert_eq!(APP_IDENTITY.suffix, None);
}
#[test]
fn renders_product_version_with_the_url_comment() {
    assert_eq!(
        user_agent(Some(&APP_IDENTITY)).unwrap(),
        "deepseek-harness/0.2.2-rc.3 (+https://github.com/deepseek-ai/deepseek-harness)"
    );
}
#[test]
fn renders_a_custom_identity() {
    assert_eq!(
        user_agent(Some(&FORK)).unwrap(),
        "fork-agent/9.9.9 (+https://example.com/fork-agent)"
    );
}
#[test]
fn appends_the_identity_suffix_after_the_comment() {
    assert_eq!(
        user_agent(Some(&AppIdentity {
            suffix: Some("dsh-profile/web"),
            ..FORK
        }))
        .unwrap(),
        "fork-agent/9.9.9 (+https://example.com/fork-agent) dsh-profile/web"
    );
}
#[test]
fn appends_the_launch_suffix_to_the_default_user_agent_when_read() {
    assert_eq!(
        launch_suffix_value(Some("dsh-profile/my%20web")).unwrap(),
        Some("dsh-profile/my%20web")
    );
}
#[test]
fn omits_the_suffix_when_empty_or_unset() {
    for v in [Some(""), None] {
        assert_eq!(launch_suffix_value(v).unwrap(), None);
    }
}
#[test]
fn rejects_invalid_suffixes() {
    for v in [
        "dsh-profile/my web",
        "dsh-profile/web\r\nx-injected: 1",
        "dsh-profile/(web)",
        "dsh-profile/a/b",
    ] {
        assert!(
            launch_suffix_value(Some(v))
                .unwrap_err()
                .to_string()
                .starts_with("DSH_USER_AGENT_SUFFIX must be an RFC 9110 product token")
        );
    }
}
#[test]
fn defaults_to_the_provider_neutral_baseline_user_agent_and_nothing_else() {
    let headers = attribution_headers(None).unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers["user-agent"], user_agent(None).unwrap());
}
#[test]
fn maps_a_custom_identity_onto_the_user_agent_header_only() {
    let headers = attribution_headers(Some(&FORK)).unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(
        headers["user-agent"],
        "fork-agent/9.9.9 (+https://example.com/fork-agent)"
    );
}
