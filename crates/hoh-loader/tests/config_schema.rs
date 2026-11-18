mod support;
use hoh_kernel::{Cfg, Context, Error, Plugin};
use hoh_loader::config_schema::*;
use hoh_loader::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use support::{live::*, *};
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct ServerConfig {
    port: u16,
}
struct Server;
impl Plugin for Server {
    const NAME: &'static str = "server";
    type Config = ServerConfig;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<ServerConfig>) -> Result<(), Error> {
        panic!("schema collection must not apply plugins")
    }
}
struct Number;
impl Plugin for Number {
    const NAME: &'static str = "number";
    type Config = f64;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<f64>) -> Result<(), Error> {
        panic!("schema collection must not apply plugins")
    }
}
struct Text;
impl Plugin for Text {
    const NAME: &'static str = "string";
    type Config = String;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<String>) -> Result<(), Error> {
        panic!("schema collection must not apply plugins")
    }
}
fn registry() -> PluginRegistry {
    let mut r = PluginRegistry::default();
    r.add::<Server>("server")
        .add::<Number>("number")
        .add::<Text>("string")
        .add::<Noop>("noop");
    r
}
async fn collect(rows: Value) -> Value {
    let dir = tempfile::tempdir().unwrap();
    collect_config_schemas("test", dir.path(), &rows, &registry(), &[]).await
}
#[test]
fn uses_an_unknown_schema_for_an_indexed_target_without_a_collected_declaration() {
    let missing = ConfigSchemaEntry {
        path: "/0".into(),
        id: Some("missing".into()),
        name: Some("missing".into()),
        status: "error".into(),
        config_ref: String::new(),
        tree: None,
    };
    let result = build_config_schema_document(
        "test",
        &[],
        &indexmap::IndexMap::from([("missing".into(), missing)]),
        &[],
        &serde_json::Map::new(),
    );
    assert!(patch_validates(
        &result,
        json!([{"id":"missing","config":"value"}])
    ));
    assert_eq!(
        result["$defs"]["patch"]["allOf"][1]["then"]["properties"]["config"]["$ref"],
        "#/$defs/unknownConfig"
    );
    assert_eq!(result["x-cordis"]["entries"], json!([]));
}
fn entries(doc: &Value) -> &Vec<Value> {
    let entries = doc["x-cordis"]["entries"].as_array().unwrap();
    for entry in entries {
        assert!(
            entry["configRef"].is_string(),
            "missing public configRef: {entry}"
        );
        assert!(entry.get("config_ref").is_none());
    }
    entries
}
fn diagnostics(doc: &Value) -> &Vec<Value> {
    doc["x-cordis"]["diagnostics"].as_array().unwrap()
}
// An independent JSON Schema assertion helper for the standard conditional/document vocabulary.
// Kernel's validator intentionally excludes if/then/else (not emitted by schemars itself).
fn accepts(root: &Value, s: &Value, v: &Value) -> bool {
    if let Some(b) = s.as_bool() {
        return b;
    }
    if let Some(r) = s["$ref"].as_str() {
        let target = if r == "#" {
            root
        } else {
            root.pointer(r.strip_prefix('#').unwrap())
                .unwrap_or_else(|| panic!("dangling schema reference {r}"))
        };
        if !accepts(root, target, v) {
            return false;
        }
    }
    if let Some(c) = s.get("const")
        && c != v
    {
        return false;
    }
    if let Some(a) = s["enum"].as_array()
        && !a.contains(v)
    {
        return false;
    }
    if let Some(a) = s["anyOf"].as_array()
        && !a.iter().any(|s| accepts(root, s, v))
    {
        return false;
    }
    if let Some(a) = s["oneOf"].as_array()
        && a.iter().filter(|s| accepts(root, s, v)).count() != 1
    {
        return false;
    }
    if let Some(a) = s["allOf"].as_array()
        && !a.iter().all(|s| accepts(root, s, v))
    {
        return false;
    }
    if let Some(n) = s.get("not")
        && accepts(root, n, v)
    {
        return false;
    }
    if let Some(condition) = s.get("if") {
        let branch = if accepts(root, condition, v) {
            s.get("then")
        } else {
            s.get("else")
        };
        if branch.is_some_and(|branch| !accepts(root, branch, v)) {
            return false;
        }
    }
    if let Some(t) = s.get("type") {
        let matches = |t: &str| match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "number" => v.is_number(),
            "integer" => v.as_f64().is_some_and(|n| n.fract() == 0.0),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => panic!("unknown type {t}"),
        };
        if !(t.as_str().is_some_and(matches)
            || t.as_array()
                .is_some_and(|a| a.iter().any(|t| t.as_str().is_some_and(matches))))
        {
            return false;
        }
    }
    if let Some(object) = v.as_object() {
        if let Some(names) = s.get("propertyNames")
            && object
                .keys()
                .any(|name| !accepts(root, names, &json!(name)))
        {
            return false;
        }
        if s["required"]
            .as_array()
            .is_some_and(|a| a.iter().any(|k| !object.contains_key(k.as_str().unwrap())))
        {
            return false;
        }
        for (key, value) in object {
            if let Some(property) = s["properties"].get(key) {
                if !accepts(root, property, value) {
                    return false;
                }
            } else if let Some(property) = s.get("additionalProperties")
                && !accepts(root, property, value)
            {
                return false;
            }
        }
    }
    if let Some(array) = v.as_array() {
        for (i, value) in array.iter().enumerate() {
            if let Some(item) = s["prefixItems"].get(i).or_else(|| s.get("items"))
                && !accepts(root, item, value)
            {
                return false;
            }
        }
    }
    if let Some(n) = v.as_f64()
        && (s["minimum"].as_f64().is_some_and(|min| n < min)
            || s["maximum"].as_f64().is_some_and(|max| n > max))
    {
        return false;
    }
    true
}
fn validates(doc: &Value, value: Value) -> bool {
    accepts(doc, doc, &value)
}
fn patch_validates(doc: &Value, value: Value) -> bool {
    accepts(doc, &doc["$defs"]["patchList"], &value)
}

#[tokio::test(start_paused = true)]
async fn owns_ordered_composition_skipped_bundle_diagnostics_and_runtime_resolution_without_mutating_layers()
 {
    let dir = tempfile::tempdir().unwrap();
    let profile = Profile {
        name: "test".into(),
        dir: dir.path().into(),
        layers: vec![],
        skipped: vec![SkippedBundle {
            name: "missing".into(),
            reason: "unavailable".into(),
        }],
    };
    let layers = vec![
        layer("first", "- insert: [{id: x, name: number, config: 1}]"),
        layer("second", "- {id: x, config: 2}"),
    ];
    let original = layers[0].patches.clone();
    let doc = generate_config_schema(&profile, &[], &layers, &registry()).await;
    assert_eq!(layers[0].patches, original);
    assert_eq!(entries(&doc)[0]["name"], "number");
    assert_eq!(
        diagnostics(&doc)[0]["message"],
        "Selected profile bundle \"missing\" could not be loaded; repair or remove its bundle selection."
    );
    assert_eq!(doc["x-cordis"]["complete"], false);
}
#[tokio::test(start_paused = true)]
async fn includes_anonymous_repeated_disabled_and_conditional_rows_without_reading_their_config_values()
 {
    let doc=collect(json!([{ "name":"server","disabled":true,"config":{"secret":"do-not-print"}},{"id":"same","name":"server","disabled":{"__jsExpr":"do-not-evaluate"}},{"id":"same","name":"server","config":{"__jsExpr":"do-not-evaluate"}}])).await;
    assert!(entries(&doc)[0].get("id").is_none());
    assert_eq!(entries(&doc)[1]["id"], "same");
    assert_eq!(entries(&doc)[2]["path"], "/2");
    assert!(!doc.to_string().contains("do-not-"));
}
#[tokio::test(start_paused = true)]
async fn reports_failed_disabled_declarations_rather_than_equating_discovery_with_bootability() {
    let doc=collect(json!([{ "name":"missing-disabled-plugin","disabled":true},{"name":"cordis:include","disabled":true,"config":{"path":"./missing.yml"}}])).await;
    assert_eq!(doc["x-cordis"]["complete"], false);
    assert_eq!(entries(&doc)[0]["status"], "error");
    assert_eq!(
        diagnostics(&doc)
            .iter()
            .map(|d| d["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["/0", "/1"]
    );
}
#[tokio::test(start_paused = true)]
async fn records_dormant_carriers_without_config_as_childless_trees_and_names_a_missing_config_otherwise()
 {
    let doc=collect(json!([{ "name":"cordis:group","disabled":true},{"name":"cordis:include","disabled":{"__jsExpr":"'disabled'"}},{"name":"cordis:group","disabled":true,"group":true},{"name":"cordis:include"}])).await;
    assert_eq!(
        entries(&doc)
            .iter()
            .map(|e| e["tree"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["group", "include", "group", "include"]
    );
    assert_eq!(
        diagnostics(&doc),
        &vec![
            json!({"level":"error","path":"/2","message":"entry list config is missing"}),
            json!({"level":"error","path":"/3","message":"include config is missing"})
        ]
    );
}
#[tokio::test(start_paused = true)]
async fn walks_native_groups_but_not_ordinary_plugin_config_arrays() {
    let doc=collect(json!([{ "name":"cordis:group","config":[{"name":"server"}]},{"name":"noop","config":[{"name":"server"}]}])).await;
    assert_eq!(
        entries(&doc)
            .iter()
            .map(|e| e["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["/0", "/0/config/0", "/1"]
    );
}
#[tokio::test(start_paused = true)]
async fn recognizes_external_canonical_include_aliases_and_caches_them_per_tree_base() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("local.yml"), "- {name: noop}\n").unwrap();
    let mut registry = registry();
    registry.alias("external", "cordis:include");
    let doc = collect_config_schemas(
        "test",
        dir.path(),
        &json!([{ "name":"external","config":{"path":"local.yml"}}]),
        &registry,
        &[],
    )
    .await;
    assert_eq!(entries(&doc)[0]["tree"], "include");
    assert_eq!(entries(&doc)[1]["path"], "/0/include/0");
}
#[tokio::test(start_paused = true)]
async fn recognizes_an_external_canonical_group_even_when_include_cannot_resolve() {
    let mut registry = registry();
    registry.alias("external", "cordis:group");
    let doc = collect_config_schemas(
        "test",
        std::path::Path::new("/fixtures"),
        &json!([{ "name":"external","config":[]}]),
        &registry,
        &[],
    )
    .await;
    assert_eq!(entries(&doc)[0]["tree"], "group");
    assert!(diagnostics(&doc).is_empty());
}
#[tokio::test(start_paused = true)]
async fn reports_unknown_tree_carrier_identities_instead_of_silently_omitting_their_children() {
    let mut registry = registry();
    registry.carrier("noop");
    let doc = collect_config_schemas(
        "test",
        std::path::Path::new("/fixtures"),
        &json!([{ "name":"noop","config":[]}]),
        &registry,
        &[],
    )
    .await;
    assert_eq!(
        diagnostics(&doc)[0]["message"],
        "unrecognized Loader tree carrier; use cordis:group or cordis:include for native child collection"
    );
}
#[tokio::test(start_paused = true)]
async fn reads_literal_includes_using_their_directory_applies_patches_and_keeps_shared_includes_independent()
 {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("nested")).unwrap();
    std::fs::write(
        dir.path().join("nested/child.yml"),
        "- {id: n, name: number, config: 1}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("list.json"),
        r#"[{"name":"cordis:include","config":{"path":"./nested/child.yml"}}]"#,
    )
    .unwrap();
    let mut registry = registry();
    registry.alias("include-package", "cordis:include");
    let doc = collect_config_schemas(
        "test",
        dir.path(),
        &json!([
            {"name":"cordis:include","config":{"path":"./list.json"}},
            {"name":"include-package","config":{"path":"./nested/child.yml","patches":[
                {"id":"n","disabled":true}, {"insert":[{"name":"noop"}]},
                {"id":"unknown","disabled":true}
            ]}}
        ]),
        &registry,
        &[],
    )
    .await;
    assert_eq!(doc["x-cordis"]["complete"], true);
    assert_eq!(
        entries(&doc)
            .iter()
            .map(|entry| entry["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "/0",
            "/0/include/0",
            "/0/include/0/include/0",
            "/1",
            "/1/include/0",
            "/1/include/1"
        ]
    );
    assert_eq!(
        diagnostics(&doc),
        &vec![
            json!({"level":"warning","path":"/1","message":"patch: entry \"unknown\" not found"})
        ]
    );
    let first = &entries(&doc)[2];
    let second = &entries(&doc)[4];
    assert_eq!(first["id"], "n");
    assert_eq!(second["id"], "n");
    assert_eq!(first["configRef"], second["configRef"]);
}
#[tokio::test(start_paused = true)]
async fn expands_initial_entries_in_memory_without_writing_missing_include_files() {
    let dir = tempfile::tempdir().unwrap();
    let doc=collect_config_schemas("test",dir.path(),&json!([{ "name":"cordis:include","config":{"path":"missing.yml","initial":[{"name":"noop"}],"patches":[{"insert":[{"name":"noop"}]}]}}]),&registry(),&[]).await;
    assert_eq!(doc["x-cordis"]["complete"], true);
    assert_eq!(entries(&doc).len(), 3);
    assert!(!dir.path().join("missing.yml").exists());
}
#[tokio::test(start_paused = true)]
async fn reports_cycles_missing_files_and_malformed_documents_without_losing_subsequent_entries() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cycle.yml"),
        "- {name: cordis:include, config: {path: ./cycle.yml}}\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("bad.yml"), "secret: [private-content").unwrap();
    std::fs::write(dir.path().join("object.json"), "{}").unwrap();
    let doc=collect_config_schemas("test",dir.path(),&json!([{ "name":"cordis:include","config":{"path":"cycle.yml"}},{"name":"cordis:include","config":{"path":"missing.yml"}},{"name":"cordis:include","config":{"path":"bad.yml","initial":[{"name":"noop"}]}},{"name":"cordis:include","config":{"path":"object.json"}},{"name":"noop"}]),&registry(),&[]).await;
    assert_eq!(
        diagnostics(&doc)
            .iter()
            .map(|diagnostic| diagnostic["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["/0/include/0", "/1", "/2", "/3"]
    );
    assert!(
        diagnostics(&doc)[0]["message"]
            .as_str()
            .unwrap()
            .contains("include cycle")
    );
    assert!(
        diagnostics(&doc)[1]["message"]
            .as_str()
            .unwrap()
            .contains("include file not found")
    );
    assert_eq!(entries(&doc).last().unwrap()["name"], "noop");
    assert!(!doc.to_string().contains("private-content"));
}
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn detects_include_cycles_through_directory_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
    std::fs::write(
        dir.path().join("cycle.yml"),
        "- {name: cordis:include, config: {path: ./loop/cycle.yml}}\n",
    )
    .unwrap();
    let doc = collect_config_schemas(
        "test",
        dir.path(),
        &json!([{ "name":"cordis:include","config":{"path":"cycle.yml"}}]),
        &registry(),
        &[],
    )
    .await;
    assert!(
        diagnostics(&doc)[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("include cycle at ")
    );
}
#[tokio::test(start_paused = true)]
async fn preserves_null_group_overrides_and_disabled_config_expressions_in_include_patches() {
    let doc=collect(json!([{ "name":"cordis:include","config":{"path":"missing.yml","initial":[{"id":"noop","name":"noop"}],"patches":[{"id":"noop","group":null,"config":{"__jsExpr":"'never'"},"disabled":{"__jsExpr":"'never'"}}]}}])).await;
    assert_eq!(doc["x-cordis"]["complete"], true);
    assert_eq!(entries(&doc).len(), 2);
    assert!(diagnostics(&doc).is_empty());
}
#[tokio::test(start_paused = true)]
async fn rejects_malformed_and_dynamic_child_groups() {
    let doc=collect(json!([{ "name":"cordis:group","config":{"__jsExpr":"never"}},{"name":"cordis:group","config":[{"name":3}]}])).await;
    assert_eq!(
        diagnostics(&doc)
            .iter()
            .map(|d| d["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["/0", "/1/config/0"]
    );
}
#[tokio::test(start_paused = true)]
async fn keeps_valid_siblings_and_original_positions_when_composed_rows_are_malformed() {
    let doc=collect(json!([{ "name":"number"},{},{"name":17},null,{"id":"bad","name":"noop","group":"invalid"},{"name":"number"}])).await;
    assert_eq!(
        entries(&doc)
            .iter()
            .map(|e| e["status"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["schema", "error", "error", "error", "error", "schema"]
    );
    assert_eq!(entries(&doc)[0]["configRef"], entries(&doc)[5]["configRef"]);
    assert!(validates(&doc, json!([{ "name":"number","config":3}])));
}
#[tokio::test(start_paused = true)]
async fn keeps_healthy_group_and_include_siblings_after_malformed_child_rows() {
    let doc=collect(json!([{ "name":"cordis:group","config":[{"name":false},{"name":"string"}]},{"name":"cordis:include","config":{"path":"missing.yml","initial":[null,{"name":"string"}]}}])).await;
    assert_eq!(
        diagnostics(&doc)
            .iter()
            .map(|d| d["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["/0/config/0", "/1/include/0"]
    );
    assert!(
        entries(&doc)
            .iter()
            .any(|e| e["path"] == "/1/include/1" && e["status"] == "schema")
    );
}
#[tokio::test(start_paused = true)]
async fn allows_disabled_config_omission_without_disabling_group_true_or_supplied_config_validation()
 {
    let doc = collect(json!([{ "name":"string"}])).await;
    for name in ["string", "cordis:group", "cordis:include"] {
        assert!(validates(&doc, json!([{ "name":name,"disabled":true}])));
        assert!(validates(
            &doc,
            json!([{ "name":name,"disabled":{"__jsExpr":"ctx.disabled"}}])
        ));
        assert!(!validates(&doc, json!([{ "name":name,"disabled":false}])));
        assert!(!validates(
            &doc,
            json!([{ "name":name,"disabled":true,"group":true}])
        ));
    }
    assert!(!validates(
        &doc,
        json!([{ "name":"string","disabled":true,"config":3}])
    ));
    assert!(validates(
        &doc,
        json!([{ "name":"cordis:group","disabled":true,"config":[{"name":"string"}]}])
    ));
    assert!(!validates(
        &doc,
        json!([{ "name":"cordis:group","disabled":true,"group":true,"config":[{"name":"string"}]}])
    ));
}
#[tokio::test(start_paused = true)]
async fn does_not_retain_a_stale_patch_constraint_after_an_invalid_duplicate_id_row() {
    let doc = collect(json!([{ "id":"same","name":"number"},{"id":"same","name":17}])).await;
    assert_eq!(doc["x-cordis"]["complete"], false);
    assert!(patch_validates(
        &doc,
        json!([{ "id":"same","config":"unknown"}])
    ));
}
#[tokio::test(start_paused = true)]
async fn validates_known_entry_config_and_whole_config_patches_while_keeping_metadata_only_patches_valid()
 {
    let doc = collect(json!([{ "id":"server","name":"server"}])).await;
    assert!(validates(
        &doc,
        json!([{ "name":"server","config":{"port":8080},"extra":"metadata"}])
    ));
    assert!(!validates(&doc, json!([{ "name":"server"}])));
    assert!(!validates(
        &doc,
        json!([{ "name":"server","config":{"port":"wrong"}}])
    ));
    assert!(validates(
        &doc,
        json!([{ "name":"new-plugin","config":{"arbitrary":true}}])
    ));
    assert!(patch_validates(
        &doc,
        json!([{ "id":"server","disabled":true}])
    ));
    assert!(!patch_validates(
        &doc,
        json!([{ "id":"server","config":{"port":"wrong"}}])
    ));
    assert!(!patch_validates(
        &doc,
        json!([{ "id":"server","config":{}}])
    ));
    assert!(patch_validates(
        &doc,
        json!([{ "id":"server","name":"not-server","config":"ignored"}])
    ));
    assert!(patch_validates(
        &doc,
        json!([{ "config":{}},{"id":"new-id","config":{}}])
    ));
}
#[tokio::test(start_paused = true)]
async fn describes_loader_metadata_and_anonymous_insertion_separately_from_config_replacement() {
    let doc = collect(
        json!([{ "id":"group","name":"cordis:group","group":true,"config":[{"name":"server"}]}]),
    )
    .await;
    assert!(patch_validates(
        &doc,
        json!([{ "id":"group","insert":[{"name":"server","config":{"port":8080}}]}])
    ));
    assert!(!patch_validates(
        &doc,
        json!([{ "id":"group","config":{"__jsExpr":"not a literal group"}}])
    ));
    for invalid in [
        json!({"isolate":{"tools":false}}),
        json!({"inject":1}),
        json!({"id":{"__jsExpr":"not literal"}}),
    ] {
        let mut row = json!({"name":"server","config":{"port":8080}});
        row.as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        assert!(!validates(&doc, json!([row])));
    }
}
#[tokio::test(start_paused = true)]
async fn allows_config_and_disabled_expressions_but_keeps_include_and_group_containers_literal() {
    let doc = collect(json!([{ "name":"server"}])).await;
    assert!(validates(
        &doc,
        json!([{ "name":"server","config":{"port":{"__jsExpr":"ctx.port"}},"disabled":{"__jsExpr":"ctx.disabled"}}])
    ));
    assert!(validates(
        &doc,
        json!([{ "name":"server","config":{"__jsExpr":"ctx.config"}}])
    ));
    assert!(!validates(
        &doc,
        json!([{ "name":"cordis:group","config":{"__jsExpr":"ctx.entries"}}])
    ));
    assert!(!validates(
        &doc,
        json!([{ "name":"cordis:include","config":{"path":{"__jsExpr":"ctx.path"}}}])
    ));
}
#[tokio::test(start_paused = true)]
async fn uses_last_root_tree_ids_without_importing_include_local_ids_into_overlay_targeting() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("local.yml"), "- {id: same, name: noop}\n").unwrap();
    let doc=collect_config_schemas("test",dir.path(),&json!([{ "id":"same","name":"number"},{"id":"same","name":"string"},{"name":"cordis:include","config":{"path":"local.yml"}}]),&registry(),&[]).await;
    assert!(patch_validates(
        &doc,
        json!([{ "id":"same","config":"value"}])
    ));
    assert!(!patch_validates(&doc, json!([{ "id":"same","config":3}])));
    assert!(accepts(
        &doc,
        &doc["$defs"]["includePatch"],
        &json!({"id":"same","config":true})
    ));
}
#[tokio::test(start_paused = true)]
async fn does_not_keep_stale_target_constraints_when_an_unmounted_group_metadata_child_replaces_an_indexed_id()
 {
    let doc=collect(json!([{ "id":"same","name":"number"},{"name":"noop","group":true,"config":[{"id":"same","name":"noop"}]}])).await;
    assert!(patch_validates(
        &doc,
        json!([{ "id":"same","config":"not-number"}])
    ));
}
#[tokio::test(start_paused = true)]
async fn includes_caller_composition_diagnostics_without_mutating_them() {
    let diagnostics = vec![ConfigSchemaDiagnostic {
        level: "warning".into(),
        path: None,
        message: "retained warning".into(),
    }];
    let doc = collect_config_schemas(
        "test",
        std::path::Path::new("/fixtures"),
        &json!([]),
        &registry(),
        &diagnostics,
    )
    .await;
    assert_eq!(diagnostics[0].message, "retained warning");
    assert_eq!(
        doc["x-cordis"]["diagnostics"][0]["message"],
        "retained warning"
    );
}
#[tokio::test(start_paused = true)]
async fn does_not_validate_contents_of_an_insertion_that_loader_skips() {
    let doc=collect(json!([{ "name":"cordis:include","config":{"path":"missing.yml","initial":[{"id":"noop","name":"noop"}],"patches":[{"id":"missing","insert":[null]},{"config":{}}]}}])).await;
    assert_eq!(entries(&doc).len(), 2);
    assert_eq!(doc["x-cordis"]["complete"], true);
    assert_eq!(
        diagnostics(&doc),
        &vec![
            json!({"level":"warning","path":"/0","message":"patch insert: entry \"missing\" not found"}),
            json!({"level":"warning","path":"/0","message":"patch: id is required for non-insert patches"})
        ]
    );
}
#[tokio::test(start_paused = true)]
async fn reports_unavailable_include_children_when_composition_fails() {
    for config in [
        json!({"initial":[null,{"name":"noop"}],"patches":[{"id":"noop","disabled":true}]}),
        json!({"initial":[],"patches":[{"insert":[null]}]}),
        json!({"initial":[{"name":"cordis:group","group":true,"config":[null]}],"patches":[{"id":"noop","disabled":true}]}),
    ] {
        let mut config = config;
        config["path"] = json!("missing.yml");
        let doc =
            collect(json!([{ "name":"cordis:include","config":config},{"name":"noop"}])).await;
        assert_eq!(entries(&doc).len(), 2);
        assert_eq!(diagnostics(&doc).len(), 1);
        assert!(
            diagnostics(&doc)[0]["message"]
                .as_str()
                .unwrap()
                .contains("child declarations are unavailable")
        );
    }
}

#[tokio::test(start_paused = true)]
async fn omits_configuration_snippets_from_invalid_include_diagnostics() {
    for (extension, content) in [
        (
            "yaml",
            "- name: noop\n  config:\n    secret: PRIVATE_CONFIG_SECRET\n    broken: [",
        ),
        (
            "json",
            "[{\"name\":\"noop\",\"config\":{\"secret\":\"PRIVATE_CONFIG_SECRET\" broken}}]",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let filename = format!("invalid.{extension}");
        std::fs::write(dir.path().join(&filename), content).unwrap();
        let doc = collect_config_schemas(
            "test",
            dir.path(),
            &json!([{ "name":"cordis:include","config":{"path":filename}}]),
            &registry(),
            &[],
        )
        .await;
        assert_eq!(doc["x-cordis"]["complete"], false);
        assert!(
            diagnostics(&doc)[0]["message"]
                .as_str()
                .unwrap()
                .contains("invalid")
        );
        assert!(!doc.to_string().contains("PRIVATE_CONFIG_SECRET"));
    }
}
#[tokio::test(start_paused = true)]
async fn preserves_nonmissing_include_failures_instead_of_using_initial() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file"), "x").unwrap();
    std::fs::create_dir(dir.path().join("directory.yml")).unwrap();
    for path in ["file/child.yml", "directory.yml"] {
        let doc = collect_config_schemas(
            "test",
            dir.path(),
            &json!([{ "name":"cordis:include","config":{"path":path,"initial":[{"name":"noop"}]}}]),
            &registry(),
            &[],
        )
        .await;
        assert_eq!(entries(&doc).len(), 1);
        assert_eq!(doc["x-cordis"]["complete"], false);
        assert_eq!(diagnostics(&doc).len(), 1);
    }
}
#[tokio::test(start_paused = true)]
async fn marks_a_nonliteral_or_unsupported_include_incomplete() {
    for config in [
        Value::Null,
        json!({"__jsExpr":"throw new Error('never')"}),
        json!({"path":{"__jsExpr":"throw new Error('never')"}}),
        json!({"path":"./config.js"}),
        json!({"path":"./missing.yml","initial":{"__jsExpr":"throw new Error('never')"}}),
        json!({"path":"./missing.yml","initial":[{"name":"noop"}],"patches":{"__jsExpr":"never"}}),
        json!({"path":"./missing.yml","initial":[{"name":"noop"}],"patches":[{"__jsExpr":"never"}]}),
    ] {
        let row = if config.is_null() {
            json!({"name":"cordis:include"})
        } else {
            json!({"name":"cordis:include","config":config})
        };
        let doc = collect(json!([row])).await;
        assert_eq!(entries(&doc).len(), 1);
        assert_eq!(doc["x-cordis"]["complete"], false);
        assert_eq!(diagnostics(&doc).len(), 1);
    }
}
#[tokio::test(start_paused = true)]
async fn rejects_dynamic_include_patch_structure_without_printing_its_expression() {
    for patch in [
        json!({"id":{"__jsExpr":"PRIVATE_EXPRESSION"},"disabled":true}),
        json!({"name":{"__jsExpr":"PRIVATE_EXPRESSION"},"id":"noop"}),
        json!({"id":"noop","group":{"__jsExpr":"PRIVATE_EXPRESSION"}}),
        json!({"insert":{"__jsExpr":"PRIVATE_EXPRESSION"}}),
        json!({"insert":[{"name":"noop","id":{"__jsExpr":"PRIVATE_EXPRESSION"}}]}),
        json!({"insert":[{"name":"noop","group":{"__jsExpr":"PRIVATE_EXPRESSION"}}]}),
    ] {
        let doc=collect(json!([{ "name":"cordis:include","config":{"path":"missing.yml","initial":[{"id":"noop","name":"noop"}],"patches":[patch]}}])).await;
        assert_eq!(doc["x-cordis"]["complete"], false);
        assert_eq!(diagnostics(&doc).len(), 1);
        assert!(!doc.to_string().contains("PRIVATE_EXPRESSION"));
    }
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct Recursive {
    label: String,
    #[serde(default)]
    next: Option<Box<Recursive>>,
}
struct RecursivePlugin;
impl Plugin for RecursivePlugin {
    const NAME: &'static str = "recursive";
    type Config = Recursive;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<Recursive>) -> Result<(), Error> {
        panic!("must not apply during discovery")
    }
}
#[tokio::test(start_paused = true)]
async fn retains_recursive_references_and_restores_serialization_state_after_a_failing_lazy_schema()
{
    let mut registry = registry();
    registry.add::<RecursivePlugin>("recursive");
    let doc = collect_config_schemas(
        "test",
        std::path::Path::new("/fixtures"),
        &json!([{ "name":"recursive"},{"name":"number"}]),
        &registry,
        &[],
    )
    .await;
    assert!(validates(
        &doc,
        json!([{ "name":"recursive","config":{"label":"root","next":{"label":"child","next":null}}}])
    ));
    assert!(!validates(
        &doc,
        json!([{ "name":"recursive","config":{"label":"root","next":{"label":3}}}])
    ));
    assert!(validates(&doc, json!([{ "name":"number","config":3}])));
}
#[tokio::test(start_paused = true)]
async fn serializes_namespace_and_class_config_exports_retaining_descriptions_defaults_and_shared_refs()
 {
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct Config {
        /// The listening port.
        #[serde(default = "port")]
        port: u16,
        #[serde(default)]
        live: hoh_kernel::Volatile<String>,
    }
    fn port() -> u16 {
        3080
    }
    struct Configured;
    impl Plugin for Configured {
        const NAME: &'static str = "configured";
        type Config = Config;
        type Deps = ();
        async fn apply(_: Context, _: (), _: Cfg<Config>) -> Result<(), Error> {
            panic!("must not apply during discovery")
        }
    }
    let mut registry = registry();
    registry
        .add::<Configured>("configured")
        .alias("alias", "configured");
    let doc = collect_config_schemas(
        "test",
        std::path::Path::new("/fixtures"),
        &json!([{ "name":"configured"},{"name":"alias"}]),
        &registry,
        &[],
    )
    .await;
    assert_eq!(entries(&doc)[0]["configRef"], entries(&doc)[1]["configRef"]);
    assert!(doc.to_string().contains("The listening port."));
    assert!(doc.to_string().contains("3080"));
    assert!(doc.to_string().contains("x-volatile"));
}
#[tokio::test(start_paused = true)]
async fn accepts_expressions_at_every_config_value_but_not_dictionary_keys() {
    #[derive(Serialize, Deserialize, schemars::JsonSchema, Eq, Hash, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum Key {
        One,
        Two,
    }
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct Config {
        values: indexmap::IndexMap<Key, Vec<u16>>,
    }
    struct Dictionary;
    impl Plugin for Dictionary {
        const NAME: &'static str = "dictionary";
        type Config = Config;
        type Deps = ();
        async fn apply(_: Context, _: (), _: Cfg<Config>) -> Result<(), Error> {
            panic!("must not apply during discovery")
        }
    }
    let mut registry = registry();
    registry.add::<Dictionary>("dictionary");
    let doc = collect_config_schemas(
        "test",
        std::path::Path::new("/fixtures"),
        &json!([{ "name":"dictionary"}]),
        &registry,
        &[],
    )
    .await;
    assert!(validates(
        &doc,
        json!([{ "name":"dictionary","config":{"values":{"one":[{"__jsExpr":"ctx.port"}]}}}])
    ));
    assert!(!validates(
        &doc,
        json!([{ "name":"dictionary","config":{"values":{"arbitrary":[1]}}}])
    ));
    assert!(validates(
        &doc,
        json!([{ "name":"dictionary","config":{"__jsExpr":"ctx.config"}}])
    ));
}
