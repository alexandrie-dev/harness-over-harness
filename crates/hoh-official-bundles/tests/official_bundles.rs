use hoh_official_bundles::*;
use hoh_package_manifest::LocalizedText;
use hoh_util_values::js_trim;
use serde_json::{json, to_string, to_value};

#[test]
fn offers_exactly_the_two_native_subagent_integrations_with_offline_localized_metadata() {
    assert_eq!(
        ON_DEMAND_BUNDLES,
        [
            "@deepseek-ai/dsh-subagent-claude-code",
            "@deepseek-ai/dsh-subagent-codex",
        ]
    );
    assert_eq!(
        OFFICIAL_ON_DEMAND_CATALOG
            .iter()
            .map(|entry| entry.package_name.as_str())
            .collect::<Vec<_>>(),
        ON_DEMAND_BUNDLES,
    );
    for entry in OFFICIAL_ON_DEMAND_CATALOG.iter() {
        assert!(entry.meta.icon.starts_with("data:image/svg+xml;base64,"));
        for text in [&entry.meta.title, &entry.meta.description] {
            let locales = match text {
                LocalizedText::Text(_) => {
                    panic!("Official metadata requires both shipped languages")
                }
                LocalizedText::Translations(locales) => locales,
            };
            assert!(!js_trim(locales.get("en").unwrap()).is_empty());
            assert!(!js_trim(locales.get("zh").unwrap()).is_empty());
        }
        assert!(to_value(entry).unwrap().get("version").is_none());
    }
}

#[test]
fn stack_each_mode_bundle_on_base_except_the_standalone_minimal_sdk() {
    assert_eq!(
        to_value(&*PROFILE_TEMPLATES).unwrap(),
        json!({
            "acp": { "bundles": ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-acp-app"] },
            "web": { "bundles": ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"] },
            "headless": { "bundles": ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-headless"] },
            "sdk": { "bundles": ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-sdk-app"] },
            "sdk-minimal": { "bundles": ["@deepseek-ai/dsh-sdk-minimal"] },
        })
    );
}

#[test]
fn preserves_every_catalog_text_icon_field_and_locale_in_source_order() {
    let expected = include_str!("fixtures/catalog.json")
        .strip_suffix('\n')
        .unwrap();
    assert_eq!(to_string(&*OFFICIAL_ON_DEMAND_CATALOG).unwrap(), expected);
}

#[test]
fn preserves_all_bundle_lists_and_set_iteration_in_source_order() {
    let actual = json!({
        "PROFILE_TEMPLATES": &*PROFILE_TEMPLATES,
        "INSTALLATION_OWNED_PROFILE_TUPLES": &*INSTALLATION_OWNED_PROFILE_TUPLES,
        "RETIRED_BUNDLES": &*RETIRED_BUNDLES,
        "DEFAULT_PROFILE_BUNDLES": DEFAULT_PROFILE_BUNDLES,
        "OPTIONAL_BUNDLES": OPTIONAL_BUNDLES,
        "ON_DEMAND_BUNDLES": ON_DEMAND_BUNDLES,
        "PROTECTED_PLUGINS": &*PROTECTED_PLUGINS,
        "HIDDEN_BUNDLES": &*HIDDEN_BUNDLES,
        "EXTENSION_BUNDLES": EXTENSION_BUNDLES,
        "MORE_BUNDLES": MORE_BUNDLES,
        "EXPERIMENTAL_BUNDLES": &*EXPERIMENTAL_BUNDLES,
    });
    let expected = include_str!("fixtures/bundle_lists.json")
        .strip_suffix('\n')
        .unwrap();
    assert_eq!(to_string(&actual).unwrap(), expected);
}
