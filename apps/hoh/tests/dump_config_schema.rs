mod support;
use hoh_kernel::{Cfg, Context, Error, NoConfig, Plugin};
use hoh_loader::{Layer, LayerSource, PluginRegistry, Profile, parse_patches};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
fn label() -> String {
    "schema-default".into()
}
fn attempts() -> f64 {
    3.0
}
fn enabled() -> bool {
    true
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
/// Namespace settings
struct NamespaceConfig {
    #[serde(default = "label")]
    /// Display label
    label: String,
    #[serde(default = "label")]
    /// Display label
    alias: String,
    #[serde(default = "attempts")]
    #[schemars(range(min = 1, max = 7))]
    attempts: f64,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct ClassConfig {
    #[serde(default = "enabled")]
    enabled: bool,
}
struct Namespace;
struct Class;
struct Absent;
macro_rules! plugin {
    ($name:ident,$config:ty) => {
        impl Plugin for $name {
            const NAME: &'static str = stringify!($name);
            type Config = $config;
            type Deps = ();
            async fn apply(_: Context, _: (), _: Cfg<$config>) -> Result<(), Error> {
                panic!("PLUGIN_APPLY_EXECUTED")
            }
        }
    };
}
plugin!(Namespace, NamespaceConfig);
plugin!(Class, ClassConfig);
plugin!(Absent, NoConfig);
fn registry() -> PluginRegistry {
    let mut r = PluginRegistry::default();
    r.add::<Namespace>("dsh-schema-acceptance-fixture/namespace")
        .add::<Class>("dsh-schema-acceptance-fixture/class")
        .add::<Absent>("dsh-schema-acceptance-fixture/absent");
    r
}
fn fixture(root: &Path, text: &str) -> Profile {
    let dir = root.join("profile");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("bundle.patch.yml");
    Profile {
        name: "schema-acceptance".into(),
        dir,
        layers: vec![Layer {
            source: LayerSource::Bundle("dsh-schema-acceptance-fixture".into()),
            file: Some(file.clone()),
            patches: parse_patches(text, &file).unwrap(),
        }],
        skipped: vec![],
    }
}
async fn dump(p: &Profile, root: &Path, r: &PluginRegistry) -> hoh::dump_config_schema::SchemaDump {
    hoh::dump_config_schema::run_dump_config_schema(p, &[], &root.join("home"), root, r)
        .await
        .unwrap()
}
#[tokio::test(start_paused = true)]
async fn prints_native_namespace_and_class_schemas_without_applying_plugins_or_evaluating_expressions()
 {
    let root = tempfile::tempdir().unwrap();
    let p = fixture(
        root.path(),
        "- insert:\n    - id: schema-namespace\n      name: dsh-schema-acceptance-fixture/namespace\n      config:\n        label: PRIVATE_ACTUAL_CONFIG_VALUE\n        attempts: 6\n        expression: !!js JSON.parse('CONFIG_EXPRESSION_EXECUTED')\n    - id: schema-class\n      name: dsh-schema-acceptance-fixture/class\n      disabled: !!js JSON.parse('DISABLED_EXPRESSION_EXECUTED')\n    - id: schema-absent\n      name: dsh-schema-acceptance-fixture/absent\n",
    );
    let result = dump(&p, root.path(), &registry()).await;
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stderr, "");
    assert!(!result.stdout.contains("PRIVATE_ACTUAL_CONFIG_VALUE"));
    assert!(!result.stdout.contains("EXPRESSION_EXECUTED"));
    assert!(!result.stdout.contains("\"uid\""));
    let expected: Value =
        hoh_util_values::js_json_parse(include_str!("expected/dump-config-schema.json")).unwrap();
    let mut common = result.document.clone();
    // Two native Config projections, plus the closed L1 marker and L4 injection dialect, are explicit differences.
    assert_eq!(
        common["$defs"]["loaderExpression"]["additionalProperties"],
        false
    );
    assert_eq!(
        common["$defs"]["entryMetadata"]["properties"]["inject"]["anyOf"],
        json!([{"type":"array","items":{"type":"string"}},{"type":"null"}])
    );
    common["$defs"]["loaderExpression"]
        .as_object_mut()
        .unwrap()
        .shift_remove("additionalProperties");
    for pointer in [
        "/$defs/config0",
        "/$defs/config1",
        "/$defs/entryMetadata/properties/inject",
    ] {
        *common.pointer_mut(pointer).unwrap() = expected.pointer(pointer).unwrap().clone();
    }
    assert_eq!(
        format!(
            "{}\n",
            hoh_util_values::js_json_stringify_pretty(&common, 2)
        ),
        include_str!("expected/dump-config-schema.json")
    );
    assert_eq!(
        result.document["x-cordis"]["entries"][2]["status"],
        "absent"
    );
    let yaml = hoh::dump_config::run_dump_config(
        &p,
        false,
        &[],
        &root.path().join("home"),
        root.path(),
        &mut |w| panic!("{w}"),
    )
    .unwrap();
    let rows = hoh_loader::parse_entries(&yaml, &root.path().join("dump.yml")).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].config["label"], "PRIVATE_ACTUAL_CONFIG_VALUE");
    // Kernel's closed validator covers Config projections, not the document's if/then dialect.
    // Keep exactly the referenced native definitions; the entire published document is pinned above.
    let projected = |key: &str, value: &Value| {
        hoh_kernel::config::validate(
            &json!({"$ref":format!("#/$defs/{key}"),"$defs":{
                "loaderExpression":result.document["$defs"]["loaderExpression"],
                "config0":result.document["$defs"]["config0"],
                "config1":result.document["$defs"]["config1"]
            }}),
            value,
        )
    };
    assert!(projected("config0", &rows[0].config).is_ok());
    assert!(projected("config0", &json!({"attempts":"wrong"})).is_err());
    assert!(projected("config1", &json!({"enabled":"wrong"})).is_err());
    assert!(projected("config1", &json!({"enabled":false})).is_ok());
}
#[tokio::test(start_paused = true)]
async fn expands_canonical_include_and_re_export_aliases() {
    let root = tempfile::tempdir().unwrap();
    let p = fixture(
        root.path(),
        "- insert:\n  - id: canonical\n    name: '@deepseek-ai/cordis-plugin-include'\n    config:\n      path: ./nested/plugins.yml\n      patches:\n        - insert: [{id: added, name: ./child.mjs}]\n  - id: alias\n    name: fixture/alias\n    config:\n      path: ./never-created.yml\n      initial:\n        - id: initial\n          name: fixture/alias\n          config: {path: ./nested/plugins.yml}\n",
    );
    std::fs::create_dir(p.dir.join("nested")).unwrap();
    std::fs::write(
        p.dir.join("nested/plugins.yml"),
        "- id: child\n  name: ./child.mjs\n",
    )
    .unwrap();
    let child = url::Url::from_file_path(p.dir.join("nested/child.mjs"))
        .unwrap()
        .to_string();
    let mut r = registry();
    r.alias("fixture/alias", "cordis:include")
        .alias(&child, "dsh-schema-acceptance-fixture/class");
    let result = dump(&p, root.path(), &r).await;
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let entries = result.document["x-cordis"]["entries"].as_array().unwrap();
    assert_eq!(entries.iter().filter(|e| e["tree"] == "include").count(), 3);
    assert_eq!(
        entries
            .iter()
            .filter(|e| e["status"] == "schema" && e.get("tree").is_none())
            .map(|e| e["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["/0/include/0", "/0/include/1", "/1/include/0/include/0"]
    );
    assert!(!p.dir.join("never-created.yml").exists());
}
#[tokio::test(start_paused = true)]
async fn does_not_treat_a_custom_marker_with_include_like_config_as_a_native_carrier() {
    let root = tempfile::tempdir().unwrap();
    let p = fixture(
        root.path(),
        "- insert: [{id: custom, name: custom-carrier, config: {path: ./not-an-include.yml, initial: [{name: missing-child}]}}]\n",
    );
    let mut r = registry();
    r.add::<Absent>("custom-carrier").carrier("custom-carrier");
    let result = dump(&p, root.path(), &r).await;
    assert_eq!(result.exit_code, 1);
    assert!(result.stderr.contains("unrecognized Loader tree carrier"));
    assert_eq!(
        result.document["x-cordis"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!p.dir.join("not-an-include.yml").exists());
}
#[tokio::test(start_paused = true)]
async fn returns_positioned_malformed_row_diagnostics_without_losing_healthy_schema_entries() {
    let root = tempfile::tempdir().unwrap();
    let doc=hoh_loader::config_schema::collect_config_schemas("test",root.path(),&json!([{ "id":"before","name":"dsh-schema-acceptance-fixture/class"},{},{"name":17},{"id":"after","name":"dsh-schema-acceptance-fixture/class"}]),&registry(),&[]).await;
    let entries = doc["x-cordis"]["entries"].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| (&e["path"], &e["status"]))
            .collect::<Vec<_>>(),
        [
            (&json!("/0"), &json!("schema")),
            (&json!("/1"), &json!("error")),
            (&json!("/2"), &json!("error")),
            (&json!("/3"), &json!("schema"))
        ]
    );
    assert!(entries[1].get("name").is_none());
    assert!(entries[2].get("name").is_none());
    assert_eq!(entries[0]["configRef"], entries[3]["configRef"]);
}
#[tokio::test(start_paused = true)]
async fn marks_a_skipped_selected_bundle_incomplete_and_exits_1() {
    let root = tempfile::tempdir().unwrap();
    let mut p = fixture(
        root.path(),
        "- insert: [{id: schema-retained, name: dsh-schema-acceptance-fixture/absent}]\n",
    );
    p.skipped.push(hoh_loader::SkippedBundle {
        name: "dsh-schema-acceptance-missing-bundle".into(),
        reason: "not compiled".into(),
    });
    let result = dump(&p, root.path(), &registry()).await;
    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.document["x-cordis"]["entries"],
        json!([{"path":"/0","id":"schema-retained","name":"dsh-schema-acceptance-fixture/absent","status":"absent","configRef":"#/$defs/unknownConfig"}])
    );
    assert_eq!(
        result.document["x-cordis"]["diagnostics"],
        json!([{"level":"error","message":"Selected profile bundle \"dsh-schema-acceptance-missing-bundle\" could not be loaded; repair or remove its bundle selection."}])
    );
    assert!(
        result
            .stderr
            .contains("dsh-schema-acceptance-missing-bundle")
    );
}
#[tokio::test(start_paused = true)]
async fn keeps_schema_warnings_plain_and_provides_source_labelled_warnings_through_the_yaml_dump() {
    let root = tempfile::tempdir().unwrap();
    let p = fixture(
        root.path(),
        "- insert: [{id: existing, name: dsh-schema-acceptance-fixture/absent}]\n",
    );
    let overlay = root.path().join("unmatched.yml");
    std::fs::write(&overlay, "- id: missing\n  disabled: true\n").unwrap();
    let result = hoh::dump_config_schema::run_dump_config_schema(
        &p,
        std::slice::from_ref(&overlay),
        &root.path().join("home"),
        root.path(),
        &registry(),
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(
        result.stderr,
        "dsh: warning: patch: entry \"missing\" not found\n"
    );
    assert_eq!(
        result.document["x-cordis"]["diagnostics"],
        json!([{"level":"warning","message":"patch: entry \"missing\" not found"}])
    );
    let mut warnings = String::new();
    hoh::dump_config::run_dump_config(
        &p,
        false,
        std::slice::from_ref(&overlay),
        &root.path().join("home"),
        root.path(),
        &mut |w| warnings.push_str(&w),
    )
    .unwrap();
    assert_eq!(
        warnings,
        format!(
            "dsh: [{}] patch: entry \"missing\" not found\n",
            overlay.display()
        )
    );
}
#[tokio::test(start_paused = true)]
async fn keeps_dump_config_as_boot_free_yaml_containing_literal_configured_values() {
    let root = tempfile::tempdir().unwrap();
    let p = fixture(
        root.path(),
        "- insert:\n  - id: yaml-entry\n    name: dsh-schema-acceptance-fixture/absent\n    config:\n      value: ACTUAL_YAML_VALUE\n      expression: !!js JSON.parse('YAML_EXPRESSION_EXECUTED')\n",
    );
    let text = hoh::dump_config::run_dump_config(
        &p,
        false,
        &[],
        &root.path().join("home"),
        root.path(),
        &mut |w| panic!("{w}"),
    )
    .unwrap();
    for value in [
        "dsh-schema-acceptance-fixture/absent",
        "value: ACTUAL_YAML_VALUE",
        "!!js",
        "YAML_EXPRESSION_EXECUTED",
    ] {
        assert!(text.contains(value));
    }
    assert!(!text.contains("\"$schema\""));
}
#[tokio::test(start_paused = true)]
async fn composes_patch_layers_in_order_and_resolves_modules_beside_their_profile_or_overlay() {
    let root = tempfile::tempdir().unwrap();
    let mut p = fixture(
        root.path(),
        "- insert: [{id: schema-bundle, name: dsh-schema-acceptance-fixture/absent}]\n",
    );
    let file = p.dir.join("cordis.patch.yml");
    p.layers.push(Layer {
        source: LayerSource::Profile,
        file: Some(file.clone()),
        patches: parse_patches(
            "- insert: [{id: schema-profile, name: ./local-schema.mjs}]\n",
            &file,
        )
        .unwrap(),
    });
    let overlays = root.path().join("overlays");
    std::fs::create_dir(&overlays).unwrap();
    let first = overlays.join("first.yml");
    let second = overlays.join("second.yml");
    std::fs::write(
        &first,
        "- insert: [{id: schema-overlay, name: ./local-schema.mjs}]\n",
    )
    .unwrap();
    std::fs::write(&second,"- id: schema-overlay\n  config: {enabled: PRIVATE_OVERLAY_VALUE}\n- insert: [{id: schema-second-overlay, name: ./local-schema.mjs}]\n").unwrap();
    let local = url::Url::from_file_path(p.dir.join("local-schema.mjs"))
        .unwrap()
        .to_string();
    let extra = url::Url::from_file_path(overlays.join("local-schema.mjs"))
        .unwrap()
        .to_string();
    let mut r = registry();
    r.alias(&local, "dsh-schema-acceptance-fixture/class")
        .alias(&extra, "dsh-schema-acceptance-fixture/absent");
    let result = hoh::dump_config_schema::run_dump_config_schema(
        &p,
        &[first, second],
        &root.path().join("home"),
        root.path(),
        &r,
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert!(!result.stdout.contains("PRIVATE_OVERLAY_VALUE"));
    let entries = result.document["x-cordis"]["entries"].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| (
                e["id"].as_str().unwrap(),
                e["name"].as_str().unwrap(),
                e["status"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            (
                "schema-bundle",
                "dsh-schema-acceptance-fixture/absent",
                "absent"
            ),
            ("schema-profile", local.as_str(), "schema"),
            ("schema-overlay", extra.as_str(), "absent"),
            ("schema-second-overlay", extra.as_str(), "absent")
        ]
    );
}
