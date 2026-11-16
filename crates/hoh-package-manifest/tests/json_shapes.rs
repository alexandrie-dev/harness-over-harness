use hoh_package_manifest::*;
use serde_json::{Value, json};

fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(value: Value) {
    let decoded: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn reads_and_writes_full_package_manifest_with_exact_field_names() {
    round_trip::<DshPackageManifest>(json!({
        "name":"example-dsh-plugin", "version":"1.0.0", "description":"summary",
        "icon":"./icon.svg", "private":false, "dependencies":{"z":"*","a":"~1"},
        "optionalDependencies":{"optional":"*"}, "peerDependencies":{"peer":"^2"},
        "engines":{"dsh":"0.1.5-alpha.1","node":">=24","npm":">=10","pnpm":"10"},
        "dsh":{"manifestVersion":1,"bundle":{"platforms":["darwin","linux"],
        "patch":["./first.yml","./second.yml"],"listedByDefault":false},
        "profile":{"bundles":["second","first"]},"client":{"platform":"web",
        "inject":["one"],"immediately":false,"external":["pkg/client"]}}
    }));
}

#[test]
fn omissions_stay_absent_and_no_reader_defaults_are_inferred() {
    round_trip::<DshPackageManifest>(json!({"name":"local","version":"0"}));
    round_trip::<DshManifest>(json!({}));
    round_trip::<DshBundleManifest>(json!({"patch":"./cordis.patch.yml"}));
    round_trip::<DshClientManifest>(json!({"platform":"web"}));
    round_trip::<DshProfileManifest>(json!({"bundles":[]}));
    round_trip::<DshEnginesManifest>(json!({}));
    let engines = DshEnginesManifest::default();
    assert_eq!(engines.dsh(), None);
    assert_eq!(engines.node(), None);
    assert_eq!(engines.npm(), None);
}

#[test]
fn localized_text_and_plugin_metadata_leave_fallback_validation_to_readers() {
    round_trip::<LocalizedText>(json!("Title"));
    round_trip::<PluginLocalizedMeta>(json!({"title":{"en":"Title","zh":"标题"},
        "description":"Introduction","icon":"data:image/png;base64,AA==","error":"unchanged"}));
    round_trip::<PluginLocalizedMeta>(json!({}));
    let input = r#"{"zh":"标题"}"#;
    let text: LocalizedText = serde_json::from_str(input).unwrap();
    assert_eq!(text.en(), None);
    assert_eq!(serde_json::to_string(&text).unwrap(), input);
    round_trip::<LocalizedText>(json!({}));
    assert_eq!(LocalizedText::Text("Title".into()).en(), None);
}

#[test]
fn required_fields_and_literal_version_follow_the_types() {
    assert!(serde_json::from_value::<DshPackageManifest>(json!({"name":"x"})).is_err());
    assert!(serde_json::from_value::<DshManifest>(json!({"manifestVersion":2})).is_err());
    assert!(serde_json::from_value::<DshBundleManifest>(json!({})).is_err());
    assert!(serde_json::from_value::<DshClientManifest>(json!({})).is_err());
    round_trip::<DshBundleManifest>(json!({"patch":[],"listedByDefault":true}));
}

#[test]
fn declaration_field_and_record_insertion_order_is_preserved() {
    let input = r#"{"name":"p","version":"1","dependencies":{"z":"1","a":"2"},"optionalDependencies":{},"peerDependencies":{},"dsh":{"manifestVersion":1}}"#;
    let value: DshPackageManifest = serde_json::from_str(input).unwrap();
    assert_eq!(serde_json::to_string(&value).unwrap(), input);
}

#[test]
fn localized_text_preserves_locale_order_with_english_after_another_locale() {
    let input = r#"{"zh":"x","en":"y"}"#;
    let text: LocalizedText = serde_json::from_str(input).unwrap();
    assert_eq!(serde_json::to_string(&text).unwrap(), input);
    assert_eq!(text.en(), Some("y"));
    let LocalizedText::Translations(locales) = text else {
        panic!("expected a translation map");
    };
    assert_eq!(
        locales.keys().map(String::as_str).collect::<Vec<_>>(),
        ["zh", "en"]
    );
}

#[test]
fn engines_preserve_named_and_unknown_engine_order() {
    let input = r#"{"node":">=20","pnpm":"10","dsh":"1","npm":">=10"}"#;
    let engines: DshEnginesManifest = serde_json::from_str(input).unwrap();
    assert_eq!(serde_json::to_string(&engines).unwrap(), input);
    assert_eq!(engines.node(), Some(">=20"));
    assert_eq!(engines.dsh(), Some("1"));
    assert_eq!(engines.npm(), Some(">=10"));
    assert_eq!(engines.0["pnpm"], "10");
}

#[test]
fn full_manifest_retains_input_order_as_an_object_and_uses_field_order_as_a_struct() {
    let input = r#"{"dsh":{"profile":{"bundles":["b","a"]}},"engines":{"node":">=20","pnpm":"10","dsh":"1"},"version":"1","name":"p"}"#;
    let object: serde_json::Map<String, Value> = serde_json::from_str(input).unwrap();
    assert_eq!(serde_json::to_string(&object).unwrap(), input);

    let manifest: DshPackageManifest = serde_json::from_str(input).unwrap();
    let output = serde_json::to_string(&manifest).unwrap();
    assert_ne!(output, input);
    assert_eq!(
        output,
        r#"{"name":"p","version":"1","engines":{"node":">=20","pnpm":"10","dsh":"1"},"dsh":{"profile":{"bundles":["b","a"]}}}"#
    );
}
