use super::*;
use hoh_kernel::{Cfg, Context, Error, Kernel, Plugin, Volatile};
use hoh_loader::{
    BootOptions, Bundle, BundleSource, EvalEnv, LoadError, PluginRegistry, Profile, ProfileContext,
    ProfileContextKey, boot, config_editor::ConfigEditor,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

fn schema() -> Value {
    json!({
        "type":"object",
        "properties":{
            "ordinary":{"type":"string"},
            "count":{"type":"integer","default":2,"x-volatile":true},
            "token":{"type":"string","x-role":"secret","x-volatile":true},
            "nested":{"type":"object","properties":{
                "fixed":{"type":"string"},
                "live":{"type":"integer","default":3,"x-volatile":true}
            }},
            "rows":{"type":"array","default":[{"name":"default","token":"private"}],"items":{"type":"object","properties":{
                "name":{"type":"string"},"token":{"type":"string","x-role":"secret"}
            }},"x-volatile":true}
        }
    })
}

#[test]
fn volatile_form_selects_nested_live_fields() {
    let form = volatile_form(&schema()).unwrap();
    assert_eq!(
        form["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["count", "token", "nested", "rows"]
    );
    assert_eq!(
        form["properties"]["nested"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["live"]
    );
}

#[test]
fn volatile_form_omits_ordinary_only_schema() {
    assert!(
        volatile_form(&json!({"type":"object","properties":{"fixed":{"type":"string"}}})).is_none()
    );
}

#[test]
fn volatile_root_is_detached_and_plain() {
    let source = json!({"type":"object","x-volatile":true,"properties":{"value":{"type":"string","x-volatile":true}}});
    let form = volatile_form(&source).unwrap();
    assert!(form.get("x-volatile").is_none());
    assert!(form["properties"]["value"].get("x-volatile").is_none());
    assert_eq!(source["x-volatile"], true);
}

#[test]
fn secret_defaults_and_required_are_removed() {
    let source = json!({"type":"object","x-volatile":true,"required":["secret","visible"],"properties":{
        "secret":{"type":"string","x-role":"secret","default":"hidden"},
        "visible":{"type":"string","default":"shown"}
    }});
    let form = volatile_form(&source).unwrap();
    assert_eq!(form["required"], json!(["visible"]));
    assert!(form["properties"]["secret"].get("default").is_none());
    assert_eq!(form["properties"]["visible"]["default"], "shown");
}

#[test]
fn nested_secret_values_are_removed_from_container_defaults() {
    let form = volatile_form(&schema()).unwrap();
    assert_eq!(
        form["properties"]["rows"]["default"],
        json!([{"name":"default"}])
    );
    assert!(!form.to_string().contains("private"));
}

#[test]
fn referenced_defaults_are_redacted_without_expanding_recursive_refs() {
    let schema = json!({
        "type":"object","x-volatile":true,
        "properties":{"nested":{"$ref":"#/$defs/Nested","default":{"name":"one","token":"private"},"x-volatile":true}},
        "$defs":{"Nested":{"type":"object","properties":{"name":{"type":"string"},"token":{"type":"string","x-role":"secret"},"child":{"$ref":"#/$defs/Nested"}}}}
    });
    let form = volatile_form(&schema).unwrap();
    assert_eq!(
        form["properties"]["nested"]["default"],
        json!({"name":"one"})
    );
    assert_eq!(form["properties"]["nested"]["$ref"], "#/$defs/Nested");
    assert!(!form.to_string().contains("private"));
}

#[test]
fn references_remain_resolvable_in_forms() {
    let source = json!({"type":"object","properties":{"node":{"$ref":"#/$defs/Node","x-volatile":true}},"$defs":{"Node":{"type":"object","properties":{"name":{"type":"string"},"child":{"$ref":"#/$defs/Node"}}}}});
    let form = volatile_form(&source).unwrap();
    assert!(form.get("$defs").is_some());
    assert_eq!(
        project_form(&form, &json!({"node":{"name":"a","child":{"name":"b"}}})),
        json!({"node":{"name":"a","child":{"name":"b"}}})
    );
}

#[test]
fn project_form_hides_ordinary_fields() {
    let form = volatile_form(&schema()).unwrap();
    assert_eq!(
        project_form(
            &form,
            &json!({"ordinary":"hidden","count":4,"nested":{"fixed":"hidden","live":8}})
        ),
        json!({"count":4,"nested":{"live":8}})
    );
}

#[test]
fn project_form_preserves_expression_nodes() {
    let form = volatile_form(&schema()).unwrap();
    assert_eq!(
        project_form(&form, &json!({"nested":{"__jsExpr":"ctx.live"}})),
        json!({"nested":{"__jsExpr":"ctx.live"}})
    );
}

#[test]
fn volatile_paths_follow_ancestors() {
    let schema = schema();
    assert!(is_volatile_path(&schema, &["nested".into(), "live".into()]));
    assert!(!is_volatile_path(
        &schema,
        &["nested".into(), "fixed".into()]
    ));
    assert!(is_volatile_path(
        &schema,
        &["rows".into(), "0".into(), "token".into()]
    ));
    assert!(!is_volatile_path(&schema, &[]));
}

#[test]
fn redaction_strips_object_array_and_dict_secrets() {
    let schema = json!({"type":"object","properties":{
        "apiKey":{"x-role":"secret"},
        "providers":{"type":"object","additionalProperties":{"type":"object","properties":{"apiKey":{"x-role":"secret"},"url":{"type":"string"}}}},
        "fallbacks":{"type":"array","items":{"type":"object","properties":{"apiKey":{"x-role":"secret"},"url":{"type":"string"}}}}
    }});
    let redacted = redact_secrets(
        &schema,
        &json!({"apiKey":"top","providers":{"one":{"apiKey":"x","url":"u"}},"fallbacks":[{"apiKey":"y","url":"v"}]}),
    );
    assert_eq!(
        redacted.value,
        json!({"providers":{"one":{"url":"u"}},"fallbacks":[{"url":"v"}]})
    );
    assert_eq!(
        redacted.secrets,
        vec![
            SettingsSecretView {
                path: vec!["apiKey".into()],
                set: true
            },
            SettingsSecretView {
                path: vec!["providers".into(), "one".into(), "apiKey".into()],
                set: true
            },
            SettingsSecretView {
                path: vec!["fallbacks".into(), "0".into(), "apiKey".into()],
                set: true
            },
        ]
    );
}

#[test]
fn redaction_drops_dictionary_entries_whose_value_is_secret() {
    let schema = json!({"type":"object","properties":{"tokens":{"type":"object","additionalProperties":{"type":"string","x-role":"secret"}}}});
    let redacted = redact_secrets(&schema, &json!({"tokens":{"a":"x","b":"y"}}));
    assert_eq!(redacted.value, json!({"tokens":{}}));
    assert_eq!(
        redacted.secrets,
        vec![
            SettingsSecretView {
                path: vec!["tokens".into(), "a".into()],
                set: true
            },
            SettingsSecretView {
                path: vec!["tokens".into(), "b".into()],
                set: true
            },
        ]
    );
}

#[test]
fn redaction_handles_nullable_dictionary_and_array_types() {
    let dictionary = redact_secrets(
        &json!({
            "type":["object","null"],
            "additionalProperties":{"type":"string","x-role":"secret"}
        }),
        &json!({"one":"dict-secret"}),
    );
    assert_eq!(dictionary.value, json!({}));
    assert_eq!(
        dictionary.secrets,
        vec![SettingsSecretView {
            path: vec!["one".into()],
            set: true,
        }]
    );

    let array = redact_secrets(
        &json!({
            "type":["array","null"],
            "items":{"type":"object","properties":{"token":{"type":"string","x-role":"secret"}}}
        }),
        &json!([{"token":"array-secret"}]),
    );
    assert_eq!(array.value, json!([{}]));
    assert_eq!(
        array.secrets,
        vec![SettingsSecretView {
            path: vec!["0".into(), "token".into()],
            set: true,
        }]
    );
}

#[test]
fn project_form_distinguishes_nullable_objects_from_dictionaries() {
    assert_eq!(
        project_form(
            &json!({"type":["object","null"]}),
            &json!({"unknown":"hidden"}),
        ),
        json!({})
    );
    let dictionary = json!({
        "type":["object","null"],
        "additionalProperties":{"type":"string"}
    });
    assert_eq!(
        project_form(&dictionary, &json!({"dynamic":"kept"})),
        json!({"dynamic":"kept"})
    );
}

#[test]
fn project_form_preserves_volatile_dictionary_keys() {
    let schema = json!({"type":"object","properties":{"providers":{"type":"object","additionalProperties":{"type":"object","properties":{"endpoint":{"type":"string"}}},"x-volatile":true}}});
    let form = volatile_form(&schema).unwrap();
    let value = json!({"providers":{"one":{"endpoint":"https://example.test"}}});
    assert_eq!(project_form(&form, &value), value);
}

#[test]
fn redaction_enumerates_unset_object_slots() {
    let redacted = redact_secrets(
        &json!({"type":"object","properties":{"token":{"x-role":"secret"},"nested":{"type":"object","properties":{"token":{"x-role":"secret"}}}}}),
        &Value::Null,
    );
    assert_eq!(redacted.value, Value::Null);
    assert_eq!(
        redacted.secrets,
        vec![
            SettingsSecretView {
                path: vec!["token".into()],
                set: false
            },
            SettingsSecretView {
                path: vec!["nested".into(), "token".into()],
                set: false
            },
        ]
    );
}

#[test]
fn redaction_preserves_unknown_fields_and_input() {
    let input = json!({"token":"hidden","extra":{"keep":true}});
    let copy = input.clone();
    let redacted = redact_secrets(
        &json!({"type":"object","properties":{"token":{"x-role":"secret"}}}),
        &input,
    );
    assert_eq!(input, copy);
    assert_eq!(redacted.value, json!({"extra":{"keep":true}}));
}

#[test]
fn redaction_conservatively_combines_union_branches() {
    let schema = json!({"anyOf":[{"type":"object","properties":{"token":{"x-role":"secret"}}},{"type":"object","properties":{"token":{"type":"string"},"name":{"type":"string"}}}]});
    let redacted = redact_secrets(&schema, &json!({"token":"hidden","name":"visible"}));
    assert_eq!(redacted.value, json!({"name":"visible"}));
    assert_eq!(
        redacted.secrets,
        vec![SettingsSecretView {
            path: vec!["token".into()],
            set: true
        }]
    );

    let intersection = redact_secrets(
        &json!({"allOf":[
            {"type":"object","properties":{"token":{"x-role":"secret"}}},
            {"type":"object","properties":{"token":{"type":"string"},"name":{"type":"string"}}}
        ]}),
        &json!({"token":"hidden","name":"visible"}),
    );
    assert_eq!(intersection.value, json!({"name":"visible"}));
    assert_eq!(
        intersection.secrets,
        vec![SettingsSecretView {
            path: vec!["token".into()],
            set: true,
        }]
    );
}

#[test]
fn redaction_matches_malformed_opaque_and_relationless_source_cases() {
    let schema = json!({"type":"object","properties":{
        "providers":{"type":"object","additionalProperties":{"x-role":"secret"}},
        "fallbacks":{"type":"array","items":{"x-role":"secret"}},
        "nested":{"type":"object","properties":{"token":{"x-role":"secret"}}}
    }});
    let malformed = redact_secrets(
        &schema,
        &json!({"providers":"not-a-dict","fallbacks":"not-an-array"}),
    );
    assert_eq!(
        malformed.value,
        json!({"providers":"not-a-dict","fallbacks":"not-an-array"})
    );
    assert_eq!(
        malformed.secrets,
        vec![SettingsSecretView {
            path: vec!["nested".into(), "token".into()],
            set: false,
        }]
    );

    let opaque = redact_secrets(
        &json!({"type":"object","properties":{"blob":{"type":"object","x-role":"secret","properties":{"inner":{"type":"string"}}}}}),
        &json!({"blob":{"inner":"secret"}}),
    );
    assert_eq!(opaque.value, json!({}));
    assert_eq!(
        opaque.secrets,
        vec![SettingsSecretView {
            path: vec!["blob".into()],
            set: true,
        }]
    );

    for (schema, value) in [
        (json!({"type":"object"}), json!({"k":"v"})),
        (json!({"type":"array"}), json!(["v"])),
    ] {
        assert_eq!(redact_secrets(&schema, &value).value, value);
    }
}

#[test]
fn merge_layers_recurses_objects_and_replaces_arrays() {
    assert_eq!(
        merge_layers(
            json!({"a":{"x":1,"y":2},"list":[1]}),
            json!({"a":{"x":3},"list":[2]})
        ),
        json!({"a":{"x":3,"y":2},"list":[2]})
    );
}

#[test]
fn mutate_sets_nested_objects() {
    let result = mutate_one(
        json!({}),
        &json!({}),
        &SettingsPathOp::Set {
            path: vec!["nested".into(), "live".into()],
            value: json!(8),
        },
        &schema(),
        &schema(),
    )
    .unwrap();
    assert_eq!(result, json!({"nested":{"live":8}}));
}

#[test]
fn mutate_creates_optional_dictionary_containers() {
    let schema = json!({"type":"object","properties":{
        "providers":{"type":"object","additionalProperties":{"type":"object","properties":{"endpoint":{"type":"string"}}},"x-volatile":true}
    }});
    let result = mutate_one(
        json!({}),
        &json!({}),
        &SettingsPathOp::Set {
            path: vec!["providers".into(), "one".into(), "endpoint".into()],
            value: json!("https://example.test"),
        },
        &schema,
        &schema,
    )
    .unwrap();
    assert_eq!(
        result,
        json!({"providers":{"one":{"endpoint":"https://example.test"}}})
    );
}

#[test]
fn mutate_appends_and_splices_arrays() {
    let set = mutate_one(
        json!({"rows":[{"name":"one"}]}),
        &json!({}),
        &SettingsPathOp::Set {
            path: vec!["rows".into(), "1".into()],
            value: json!({"name":"two"}),
        },
        &schema(),
        &schema(),
    )
    .unwrap();
    assert_eq!(set, json!({"rows":[{"name":"one"},{"name":"two"}]}));
    let unset = mutate_one(
        set,
        &json!({}),
        &SettingsPathOp::Unset {
            path: vec!["rows".into(), "0".into()],
        },
        &schema(),
        &schema(),
    )
    .unwrap();
    assert_eq!(unset, json!({"rows":[{"name":"two"}]}));
}

#[test]
fn mutate_uses_array_schema_default() {
    let result = mutate_one(
        json!({}),
        &json!({}),
        &SettingsPathOp::Set {
            path: vec!["rows".into(), "0".into(), "name".into()],
            value: json!("edited"),
        },
        &schema(),
        &schema(),
    )
    .unwrap();
    assert_eq!(
        result,
        json!({"rows":[{"name":"edited","token":"private"}]})
    );
}

#[test]
fn mutate_rejects_invalid_array_indexes() {
    for index in ["2", "01", "-1", "+1"] {
        let error = mutate_one(
            json!({"rows":[]}),
            &json!({}),
            &SettingsPathOp::Set {
                path: vec!["rows".into(), index.into()],
                value: json!({}),
            },
            &schema(),
            &schema(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("out of range"));
    }
}

#[test]
fn lookup_and_schema_reject_signed_array_indexes() {
    let value = json!(["zero", "one"]);
    assert!(lookup(&value, &["+1".into()]).is_none());

    let array_schema = json!({"type":"array","items":{"type":"string"}});
    assert!(schema::child_schema(&array_schema, &array_schema, "+1").is_none());
}

#[test]
fn empty_path_scalar_set_uses_source_root_error() {
    let error = mutate_one(
        json!({}),
        &json!({}),
        &SettingsPathOp::Set {
            path: vec![],
            value: json!(5),
        },
        &schema(),
        &schema(),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "Config root must be a plain object");
}

#[test]
fn unset_restores_inherited_object_field() {
    let result = mutate_one(
        json!({"count":9}),
        &json!({"count":2}),
        &SettingsPathOp::Unset {
            path: vec!["count".into()],
        },
        &schema(),
        &schema(),
    )
    .unwrap();
    assert_eq!(result, json!({"count":2}));
}

#[test]
fn empty_unset_restores_whole_base() {
    let result = mutate_one(
        json!({"count":9}),
        &json!({"count":2}),
        &SettingsPathOp::Unset { path: vec![] },
        &schema(),
        &schema(),
    )
    .unwrap();
    assert_eq!(result, json!({"count":2}));
}

#[test]
fn validation_rejects_ordinary_fields() {
    let schema = schema();
    let form = volatile_form(&schema).unwrap();
    let error = validate_paths(
        &json!({"ordinary":"changed"}),
        &schema,
        &form,
        &form,
        &mut vec![],
    )
    .unwrap_err();
    assert!(error.to_string().contains("not volatile"));
}

#[test]
fn validation_rejects_empty_unknown_and_fixed_objects() {
    let schema = schema();
    let form = volatile_form(&schema).unwrap();
    for value in [json!({"unknown":{}}), json!({"nested":{"fixed":{}}})] {
        let error = validate_paths(&value, &schema, &form, &form, &mut vec![]).unwrap_err();
        assert!(error.to_string().contains("not volatile"));
    }
}

#[test]
fn strip_preserves_ordinary_siblings() {
    let schema = schema();
    let form = volatile_form(&schema).unwrap();
    assert_eq!(
        strip_volatile(
            &json!({"ordinary":"fixed","count":8,"nested":{"fixed":"x","live":4}}),
            &schema,
            &form,
            &form,
            &mut vec![]
        ),
        json!({"ordinary":"fixed","nested":{"fixed":"x"}})
    );
}

#[test]
fn volatile_paths_and_stripping_do_not_descend_into_dynamic_containers() {
    let schema = json!({"type":"object","properties":{
        "list":{"type":"array","items":{"type":"object","properties":{"a":{"type":"integer","x-volatile":true}}}},
        "dict":{"type":"object","additionalProperties":{"type":"integer","x-volatile":true}},
        "live":{"type":"integer","x-volatile":true}
    }});
    assert!(!is_volatile_path(
        &schema,
        &["list".into(), "0".into(), "a".into()]
    ));
    assert!(!is_volatile_path(
        &schema,
        &["dict".into(), "dynamic".into()]
    ));
    let form = volatile_form(&schema).unwrap();
    assert_eq!(
        strip_volatile(
            &json!({"list":[{"a":1}],"dict":{"dynamic":2},"live":3,"unknown":4}),
            &schema,
            &form,
            &form,
            &mut vec![],
        ),
        json!({"list":[{"a":1}],"dict":{"dynamic":2},"unknown":4})
    );
}

#[test]
fn descriptor_debug_omits_all_secret_bearing_fields() {
    let descriptor = SettingsDescriptor {
        ns: "debug".into(),
        auto_generate: true,
        schema: json!({"default":"schema-secret"}),
        value: json!({"token":"value-secret"}),
        revision: 3,
        base: json!({"token":"base-secret"}),
        user: json!({"token":"user-secret"}),
        applies: SettingsApplyMode::Live,
        secrets: None,
    };
    let debug = format!("{descriptor:?}");
    for secret in [
        "schema-secret",
        "value-secret",
        "base-secret",
        "user-secret",
    ] {
        assert!(!debug.contains(secret));
    }
    assert!(debug.contains("SettingsDescriptor"));
    assert!(debug.contains("debug"));
}

#[test]
fn conflict_error_has_stable_contract() {
    let error = SettingsConflictError {
        namespace: "first".into(),
        expected: 1,
        actual: 2,
    };
    assert_eq!(SettingsConflictError::CODE, "SETTINGS_CONFLICT");
    assert_eq!(
        error.to_string(),
        "settings namespace \"first\" changed since it was read (expected revision 1, now 2)"
    );
}

#[test]
fn redacted_secret_source_alias_is_public() {
    let secret: RedactedSecret = SettingsSecretView {
        path: vec!["token".into()],
        set: true,
    };
    assert!(secret.set);
}

#[test]
fn wire_path_ops_round_trip() {
    let value = json!({"op":"set","path":["a"],"value":3});
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<SettingsPathOpView>(value.clone()).unwrap())
            .unwrap(),
        value
    );
}

#[test]
fn legacy_yaml_parses_sections_and_empty_documents() {
    assert_eq!(yaml::parse("").unwrap(), Value::Null);
    assert_eq!(
        yaml::parse("first:\n  count: 2\n  enabled: true\n").unwrap(),
        json!({"first":{"count":2,"enabled":true}})
    );
}

#[test]
fn legacy_yaml_supports_core_tags_radix_and_aliases() {
    assert_eq!(
        yaml::parse(
            "text: !!str 12\nhex: !!int 0x10\noctal: 0o10\nbase: &base {count: 2}\ncopy: *base\n"
        )
        .unwrap(),
        json!({"text":"12","hex":16,"octal":8,"base":{"count":2},"copy":{"count":2}})
    );
}

#[test]
fn legacy_yaml_rejects_non_finite_numbers_and_alias_bombs() {
    assert!(
        yaml::parse("first: {count: .inf}\n")
            .unwrap_err()
            .contains("non-finite")
    );
    let mut bomb = String::from("a: &a [x, x]\n");
    for index in 0..18 {
        let before = if index == 0 {
            "a".into()
        } else {
            format!("a{index}")
        };
        let next = format!("a{}", index + 1);
        bomb.push_str(&format!("{next}: &{next} [*{before}, *{before}]\n"));
    }
    assert!(yaml::parse(&bomb).unwrap_err().contains("node limit"));
}

#[test]
fn legacy_section_names_follow_source_mapping() {
    assert_eq!(legacy_namespace("ui-developer-tools"), "ui-settings");
    assert_eq!(legacy_namespace("ui-onboarding"), "ui-settings-general");
    assert_eq!(legacy_namespace("custom"), "custom");
}

#[derive(Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct ProbeConfig {
    ordinary: String,
    #[serde(default)]
    count: Volatile<u32>,
    #[serde(default)]
    #[schemars(extend("x-role" = "secret"))]
    token: Volatile<String>,
}
struct Probe;
impl Plugin for Probe {
    const NAME: &'static str = "settings-probe";
    type Config = ProbeConfig;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<ProbeConfig>) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct EmptyConfig {
    #[serde(default)]
    count: Volatile<u32>,
}
struct EmptyProbe;
impl Plugin for EmptyProbe {
    const NAME: &'static str = "settings-empty-probe";
    type Config = EmptyConfig;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<EmptyConfig>) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct NullableSecretRow {
    name: String,
    #[schemars(extend("x-role" = "secret"))]
    token: String,
}
#[derive(Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct NullableSecretConfig {
    #[serde(default)]
    rows: Volatile<Option<Vec<NullableSecretRow>>>,
}
struct NullableSecretProbe;
impl Plugin for NullableSecretProbe {
    const NAME: &'static str = "settings-nullable-secret-probe";
    type Config = NullableSecretConfig;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<NullableSecretConfig>) -> Result<(), Error> {
        Ok(())
    }
}
struct Source(Vec<Bundle>);
impl BundleSource for Source {
    fn bundle(&self, name: &str) -> Result<Bundle, LoadError> {
        self.0
            .iter()
            .find(|bundle| bundle.name == name)
            .cloned()
            .ok_or_else(|| LoadError::Runtime(format!("missing bundle {name}")))
    }
}
fn environment(root: &Path) -> EvalEnv {
    EvalEnv {
        home: root.join("home"),
        cwd: root.into(),
        exe: root.join("hoh"),
        platform: "linux",
        var: Arc::new(|_| None),
    }
}
struct RuntimeFixture {
    _directory: tempfile::TempDir,
    kernel: Kernel,
    ctx: Context,
    loader: hoh_kernel::Service<LoaderKey>,
    settings: Arc<SettingsForms>,
    source: Arc<dyn BundleSource>,
    profile: PathBuf,
    home: PathBuf,
}
async fn runtime_fixture() -> RuntimeFixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let profile = root.join("profile");
    let home = root.join("home");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(
        profile.join("package.json"),
        json!({"dsh":{"profile":{"bundles":["bundle"]}}}).to_string(),
    )
    .unwrap();
    std::fs::write(profile.join("cordis.patch.yml"), "[]\n").unwrap();
    let source: Arc<dyn BundleSource> = Arc::new(Source(vec![Bundle {
        name: "bundle".into(),
        dir: Some(root.join("bundle")),
        platforms: None,
        files: vec![(
            "cordis.patch.yml".into(),
            concat!(
                "- insert:\n",
                "  - {id: first, name: probe, config: {ordinary: fixed, count: 2, token: private}}\n",
                "  - {id: empty, name: empty-probe}\n",
                "  - {id: nullable-secret, name: nullable-secret-probe, config: {rows: [{name: visible, token: nullable-private}]}}\n",
            )
            .into(),
        )],
    }]));
    let base = root.join("base.yml");
    std::fs::write(&base, "[]\n").unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let mut registry = PluginRegistry::default();
    registry.add::<Probe>("probe");
    registry.add::<EmptyProbe>("empty-probe");
    registry.add::<NullableSecretProbe>("nullable-secret-probe");
    let loader = boot(
        &ctx,
        BootOptions {
            registry: Arc::new(registry),
            base,
            layers: vec![],
            env: environment(&root),
        },
    )
    .await
    .unwrap();
    ctx.provide::<ProfileContextKey>(Arc::new(ProfileContext {
        name: "test".into(),
        dir: profile.clone(),
        home: home.clone(),
    }))
    .await
    .unwrap();
    let composed =
        Profile::load(&profile, source.as_ref(), &home, &[], &environment(&root)).unwrap();
    loader.recompose(composed.layers).await.unwrap();
    let editor = ConfigEditor::mount(
        &ctx,
        profile.clone(),
        source.clone(),
        home.clone(),
        vec![],
        environment(&root),
    )
    .await
    .unwrap();
    let (settings, _refresh) = SettingsForms::create(ctx.clone(), editor);
    RuntimeFixture {
        _directory: directory,
        kernel,
        ctx,
        loader,
        settings,
        source,
        profile,
        home,
    }
}

#[tokio::test]
async fn real_editor_updates_live_config_without_remounting() {
    let fixture = runtime_fixture().await;
    let uid = fixture
        .loader
        .entries()
        .into_iter()
        .find(|entry| entry.options.id == "first")
        .unwrap()
        .fiber
        .unwrap()
        .uid();
    fixture
        .settings
        .update("first", json!({"count":7}), None)
        .await
        .unwrap();
    let entry = fixture
        .loader
        .entries()
        .into_iter()
        .find(|entry| entry.options.id == "first")
        .unwrap();
    assert_eq!(entry.fiber.as_ref().unwrap().uid(), uid);
    assert_eq!(entry.fiber.unwrap().effective_config().unwrap()["count"], 7);
    assert!(
        std::fs::read_to_string(fixture.profile.join("cordis.patch.yml"))
            .unwrap()
            .contains("count: 7")
    );
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn real_describe_redacts_secrets_and_rejects_stale_revision() {
    let fixture = runtime_fixture().await;
    let descriptor = fixture
        .settings
        .describe(SettingsDescribeOptions {
            redact_secrets: true,
        })
        .await
        .unwrap()
        .remove(0);
    assert_eq!(descriptor.value, json!({"count":2}));
    assert_eq!(
        descriptor.secrets.unwrap(),
        vec![SettingsSecretView {
            path: vec!["token".into()],
            set: true
        }]
    );
    fixture
        .settings
        .update("first", json!({"count":3}), Some(descriptor.revision))
        .await
        .unwrap();
    let error = fixture
        .settings
        .update("first", json!({"count":4}), Some(descriptor.revision))
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<SettingsConflictError>().is_some());
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn concurrent_writes_serialize_and_only_one_revision_wins() {
    let fixture = runtime_fixture().await;
    let revision = fixture
        .settings
        .describe(SettingsDescribeOptions::default())
        .await
        .unwrap()
        .into_iter()
        .find(|descriptor| descriptor.ns == "first")
        .unwrap()
        .revision;
    let first = fixture
        .settings
        .update("first", json!({"count":7}), Some(revision));
    let second = fixture
        .settings
        .update("first", json!({"count":8}), Some(revision));
    let (first, second) = tokio::join!(first, second);
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let error = first.err().or_else(|| second.err()).unwrap();
    assert!(error.downcast_ref::<SettingsConflictError>().is_some());
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn inherited_secret_is_restored_without_crossing_redacted_describe() {
    let fixture = runtime_fixture().await;
    fixture
        .settings
        .update("first", json!({"token":"changed"}), None)
        .await
        .unwrap();
    fixture
        .settings
        .mutate(
            "first",
            vec![SettingsPathOp::Unset {
                path: vec!["token".into()],
            }],
            None,
        )
        .await
        .unwrap();
    let plain = fixture
        .settings
        .describe(SettingsDescribeOptions::default())
        .await
        .unwrap()
        .into_iter()
        .find(|descriptor| descriptor.ns == "first")
        .unwrap();
    assert_eq!(plain.value["token"], "private");
    let redacted = fixture
        .settings
        .describe(SettingsDescribeOptions {
            redact_secrets: true,
        })
        .await
        .unwrap()
        .into_iter()
        .find(|descriptor| descriptor.ns == "first")
        .unwrap();
    assert!(!redacted.value.to_string().contains("private"));
    assert!(!redacted.base.to_string().contains("private"));
    assert!(!redacted.user.to_string().contains("private"));
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn real_describe_redacts_nullable_secret_arrays() {
    let fixture = runtime_fixture().await;
    let descriptor = fixture
        .settings
        .describe(SettingsDescribeOptions {
            redact_secrets: true,
        })
        .await
        .unwrap()
        .into_iter()
        .find(|descriptor| descriptor.ns == "nullable-secret")
        .unwrap();
    assert_eq!(descriptor.value, json!({"rows":[{"name":"visible"}]}));
    assert!(!descriptor.base.to_string().contains("nullable-private"));
    assert!(!descriptor.user.to_string().contains("nullable-private"));
    assert_eq!(
        descriptor.secrets,
        Some(vec![SettingsSecretView {
            path: vec!["rows".into(), "0".into(), "token".into()],
            set: true,
        }])
    );
    fixture.kernel.shutdown().await;
}

#[test]
fn mutation_preflight_rejects_volatile_nodes_below_dynamic_containers() {
    let schema = json!({"type":"object","properties":{
        "list":{"type":"array","items":{"type":"object","properties":{"a":{"type":"integer","x-volatile":true}}}},
        "dict":{"type":"object","additionalProperties":{"type":"integer","x-volatile":true}},
        "live":{"type":"integer","x-volatile":true}
    }});
    for (path, expected) in [
        (
            vec!["list".into(), "0".into(), "a".into()],
            "Config field \"list.0.a\" is not volatile",
        ),
        (
            vec!["dict".into(), "dynamic".into()],
            "Config field \"dict.dynamic\" is not volatile",
        ),
    ] {
        let error = validate_mutation_paths(
            &schema,
            &[SettingsPathOp::Set {
                path,
                value: json!(9),
            }],
        )
        .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
}

#[tokio::test]
async fn absent_and_explicit_empty_config_share_revision() {
    let fixture = runtime_fixture().await;
    let before = fixture
        .settings
        .describe(SettingsDescribeOptions::default())
        .await
        .unwrap()
        .into_iter()
        .find(|descriptor| descriptor.ns == "empty")
        .unwrap();
    let events = Arc::new(std::sync::Mutex::new(Vec::<(String, u64)>::new()));
    let recorded = events.clone();
    fixture
        .ctx
        .clone()
        .on::<SettingsDocumentUpdated>(move |event| {
            recorded
                .lock()
                .unwrap()
                .push((event.ns.to_string(), event.revision));
        })
        .unwrap();

    std::fs::write(
        fixture.profile.join("cordis.patch.yml"),
        "- id: empty\n  config: {}\n",
    )
    .unwrap();
    let composed = Profile::load(
        &fixture.profile,
        fixture.source.as_ref(),
        &fixture.home,
        &[],
        &environment(fixture._directory.path()),
    )
    .unwrap();
    fixture.loader.recompose(composed.layers).await.unwrap();
    let after = fixture
        .settings
        .describe(SettingsDescribeOptions::default())
        .await
        .unwrap()
        .into_iter()
        .find(|descriptor| descriptor.ns == "empty")
        .unwrap();
    assert_eq!(after.revision, before.revision);
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .all(|(namespace, _)| namespace != "empty")
    );
    fixture
        .settings
        .update("empty", json!({"count":1}), Some(before.revision))
        .await
        .unwrap();
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn real_legacy_import_renames_before_writing() {
    let fixture = runtime_fixture().await;
    std::fs::create_dir_all(&fixture.home).unwrap();
    std::fs::write(fixture.home.join("settings.yaml"), "first:\n  count: 9\n").unwrap();
    fixture.settings.import_legacy_document().await.unwrap();
    assert!(!fixture.home.join("settings.yaml").exists());
    assert!(fixture.home.join("settings.yaml.imported").exists());
    let entry = fixture
        .loader
        .entries()
        .into_iter()
        .find(|entry| entry.options.id == "first")
        .unwrap();
    assert_eq!(entry.fiber.unwrap().effective_config().unwrap()["count"], 9);
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn real_replace_and_mutate_preserve_ordinary_fields() {
    let fixture = runtime_fixture().await;
    fixture
        .settings
        .update("first", json!({"count":8,"token":"changed"}), None)
        .await
        .unwrap();
    fixture
        .settings
        .replace("first", json!({}), None)
        .await
        .unwrap();
    let entry = fixture
        .loader
        .entries()
        .into_iter()
        .find(|entry| entry.options.id == "first")
        .unwrap();
    assert_eq!(
        entry.fiber.as_ref().unwrap().effective_config().unwrap()["ordinary"],
        "fixed"
    );
    assert_eq!(
        entry.fiber.as_ref().unwrap().effective_config().unwrap()["count"],
        2
    );
    fixture
        .settings
        .mutate(
            "first",
            vec![SettingsPathOp::Set {
                path: vec!["count".into()],
                value: json!(6),
            }],
            None,
        )
        .await
        .unwrap();
    fixture
        .settings
        .mutate(
            "first",
            vec![SettingsPathOp::Unset {
                path: vec!["count".into()],
            }],
            None,
        )
        .await
        .unwrap();
    let descriptor = fixture
        .settings
        .describe(SettingsDescribeOptions::default())
        .await
        .unwrap()
        .remove(0);
    assert_eq!(descriptor.value["count"], 2);
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn real_presentation_policy_is_per_instance_and_reversible() {
    let fixture = runtime_fixture().await;
    let owner = fixture
        .loader
        .entries()
        .into_iter()
        .find(|entry| entry.options.id == "first")
        .unwrap()
        .fiber
        .unwrap();
    let undo = fixture
        .settings
        .configure(SettingsPresentation { auto: Some(false) }, owner.clone())
        .unwrap();
    assert!(
        !fixture
            .settings
            .describe(SettingsDescribeOptions::default())
            .await
            .unwrap()[0]
            .auto_generate
    );
    assert!(
        fixture
            .settings
            .configure(SettingsPresentation { auto: Some(true) }, owner)
            .is_err()
    );
    undo();
    assert!(
        fixture
            .settings
            .describe(SettingsDescribeOptions::default())
            .await
            .unwrap()[0]
            .auto_generate
    );
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn removed_form_emits_one_terminal_revision() {
    let fixture = runtime_fixture().await;
    let events = Arc::new(std::sync::Mutex::new(Vec::<(String, u64)>::new()));
    let recorded = events.clone();
    fixture
        .ctx
        .clone()
        .on::<SettingsDocumentUpdated>(move |event| {
            recorded
                .lock()
                .unwrap()
                .push((event.ns.to_string(), event.revision))
        })
        .unwrap();
    let first = fixture
        .settings
        .describe(SettingsDescribeOptions::default())
        .await
        .unwrap()
        .remove(0);
    let entry = fixture
        .loader
        .entries()
        .into_iter()
        .find(|entry| entry.options.id == "first")
        .unwrap();
    fixture.loader.remove(&entry.id).await.unwrap();
    assert!(
        fixture
            .settings
            .describe(SettingsDescribeOptions::default())
            .await
            .unwrap()
            .iter()
            .all(|descriptor| descriptor.ns != "first")
    );
    assert!(
        fixture
            .settings
            .describe(SettingsDescribeOptions::default())
            .await
            .unwrap()
            .iter()
            .all(|descriptor| descriptor.ns != "first")
    );
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|(ns, revision)| ns == "first" && *revision == first.revision + 1)
            .count(),
        1
    );
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn removed_form_events_follow_configuration_order() {
    let fixture = runtime_fixture().await;
    let descriptors = fixture
        .settings
        .describe(SettingsDescribeOptions::default())
        .await
        .unwrap();
    let expected = descriptors
        .iter()
        .map(|descriptor| descriptor.ns.to_string())
        .collect::<Vec<_>>();
    let events = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let recorded = events.clone();
    fixture
        .ctx
        .clone()
        .on::<SettingsDocumentUpdated>(move |event| {
            recorded.lock().unwrap().push(event.ns.to_string());
        })
        .unwrap();
    fixture.loader.recompose(vec![]).await.unwrap();
    assert!(
        fixture
            .settings
            .describe(SettingsDescribeOptions::default())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(*events.lock().unwrap(), expected);
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn real_missing_and_ordinary_field_writes_are_refused() {
    let fixture = runtime_fixture().await;
    assert!(
        fixture
            .settings
            .update("missing", json!({}), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("No configurable")
    );
    assert!(
        fixture
            .settings
            .update("first", json!({"ordinary":"changed"}), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not volatile")
    );
    fixture.kernel.shutdown().await;
}
