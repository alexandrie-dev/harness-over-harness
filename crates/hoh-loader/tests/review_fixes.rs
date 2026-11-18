mod support;
use hoh_kernel::{FiberState, Kernel};
use hoh_loader::*;
use serde_json::{Value, json};
use std::sync::Arc;
use support::{live::*, *};

#[tokio::test(start_paused = true)]
async fn ancestor_disablement_follows_owning_groups_and_recomposition() {
    let world = World::new("- id: g\n  name: cordis:group\n  group: true\n  disabled: true\n  config: [{id: c, name: noop}]\n").await;
    let group = world.row("g").fiber.unwrap();
    assert_eq!(group.state(), FiberState::Active);
    assert!(world.row("c").disabled);
    assert!(world.row("c").fiber.is_none());
    world.patch("- {id: g, disabled: false}").await.unwrap();
    assert!(world.active("c"));
    let child = world.row("c").fiber.unwrap();
    world
        .patch("- {id: g, disabled: !!js \"true\"}")
        .await
        .unwrap();
    assert!(world.row("c").disabled);
    assert!(child.is_disposed());
    assert_eq!(world.row("g").fiber.unwrap().uid(), group.uid());
    world.close().await;
}

async fn rejected_root(text: Option<&str>) -> (LoadError, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cordis.yml");
    if let Some(text) = text {
        std::fs::write(&path, text).unwrap();
    }
    let kernel = Kernel::start();
    let error = boot(
        &kernel.root(),
        BootOptions {
            registry: Arc::new(PluginRegistry::default()),
            base: path.clone(),
            layers: vec![],
            env: env(dir.path()),
        },
    )
    .await
    .err()
    .unwrap();
    kernel.shutdown().await;
    (error, path)
}
#[tokio::test(start_paused = true)]
async fn missing_root_is_a_bootstrap_startup_error_with_the_source_message() {
    let (error, path) = rejected_root(None).await;
    assert!(matches!(error, LoadError::Startup(_)), "{error}");
    assert_eq!(
        error.to_string(),
        format!(
            "hoh: startup failed: 1 required plugin did not activate\n\nFailed plugins (1):\n  include (required)\n    Package: cordis:include\n    config file not found: {}",
            path.display()
        )
    );
}
#[tokio::test(start_paused = true)]
async fn malformed_root_is_a_parse_stage_startup_error() {
    let (error, path) = rejected_root(Some("invalid: [unclosed\n")).await;
    assert!(matches!(error, LoadError::Startup(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains(&format!("failed to parse config file {}", path.display()))
    );
    assert!(error.to_string().contains("unexpected end"));
}
#[tokio::test(start_paused = true)]
async fn nonarray_root_is_a_validate_stage_startup_error() {
    let (error, path) = rejected_root(Some("entries: []\n")).await;
    assert!(matches!(error, LoadError::Startup(_)), "{error}");
    assert_eq!(
        error.to_string(),
        format!(
            "hoh: startup failed: 1 required plugin did not activate\n\nFailed plugins (1):\n  include (required)\n    Package: cordis:include\n    failed to validate config file {}\n    config file must be a top-level array",
            path.display()
        )
    );
}

#[tokio::test(start_paused = true)]
async fn initial_json_writes_generated_ids_without_a_trailing_newline() {
    let world=World::new("- {id: initialized, name: cordis:include, config: {path: ./initial.json, initial: [{name: noop}]}}").await;
    assert_eq!(
        std::fs::read_to_string(world.dir.path().join("initial.json")).unwrap(),
        "[\n  {\n    \"name\": \"noop\",\n    \"id\": \"00000001\"\n  }\n]"
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn initial_yaml_persists_the_assigned_id() {
    let world=World::new("- {id: initialized, name: cordis:include, config: {path: ./initial.yml, initial: [{name: noop}]}}").await;
    assert_eq!(
        std::fs::read_to_string(world.dir.path().join("initial.yml")).unwrap(),
        "- name: noop\n  id: \"00000001\"\n"
    );
    assert_eq!(world.row("00000001").options.id, "00000001");
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn recompose_uses_cached_base_until_refresh() {
    let world = World::new("- {id: x, name: noop, config: {value: cached}}").await;
    std::fs::write(
        world.dir.path().join("cordis.yml"),
        "- {id: x, name: noop, config: {value: disk}}\n",
    )
    .unwrap();
    world.patch("[]").await.unwrap();
    assert_eq!(world.row("x").options.config, json!({"value":"cached"}));
    world.loader.refresh("include").await.unwrap();
    assert_eq!(world.row("x").options.config, json!({"value":"disk"}));
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn recompose_aggregates_every_introduced_failure_with_the_source_header() {
    let world = World::new("[]").await;
    let error = world
        .patch("- insert: [{id: a, name: fails}, {id: b, name: fails}]")
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "hoh: warning: 2 entries did not activate\na (fails): activation boom\nb (fails): activation boom"
    );
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn optional_import_failures_retain_the_source_phase_label() {
    let world = World::new("- {id: absent, name: absent}").await;
    let mut warnings = vec![];
    hoh_loader::startup_diagnostics::audit_startup_entries(&world.loader, "hoh", &mut |line| {
        warnings.push(line)
    })
    .await
    .unwrap();
    assert_eq!(
        warnings,
        [
            "hoh: warning: 1 entry did not activate\nabsent (absent): failed to import: cannot resolve plugin absent\n"
        ]
    );
    world.close().await;
}

#[tokio::test(start_paused = true)]
async fn schema_include_errors_preserve_position_without_config_snippets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.yml");
    let text = "- name: noop\n  config: [\n";
    std::fs::write(&path, text).unwrap();
    let LoadError::Yaml { line, column, .. } = parse_entries(text, &path).unwrap_err() else {
        panic!("syntax error expected")
    };
    let result = hoh_loader::config_schema::collect_config_schemas(
        "test",
        dir.path(),
        &json!([{"name":"cordis:include","config":{"path":"./bad.yml"}}]),
        &PluginRegistry::default(),
        &[],
    )
    .await;
    assert_eq!(
        result["x-cordis"]["diagnostics"][0]["message"],
        format!(
            "invalid YAML include {} at line {line}, column {column}",
            path.display()
        )
    );
}
#[tokio::test(start_paused = true)]
async fn schema_publishes_the_source_annotations_verbatim() {
    let result = hoh_loader::config_schema::collect_config_schemas(
        "test",
        std::path::Path::new("/fixtures"),
        &json!([]),
        &PluginRegistry::default(),
        &[],
    )
    .await;
    assert_eq!(
        result["$defs"]["entry"]["description"],
        "Loader entry. Unknown metadata and plugin names are accepted; plugin-specific validation is available only for the names collected in this profile. Distinct resolutions of one name use a union of their schemas."
    );
    assert_eq!(
        result["description"],
        "Describes the parsed entry-list YAML printed by --dump-config. Use $defs.patchList for a profile/home/CLI overlay. Parse !!js with the Cordis entry-list YAML dialect. Expression results, service dependencies, plugin startup checks, and sequence-dependent patch targets still require runtime validation."
    );
    assert_eq!(
        result["$comment"],
        "Bundle, profile, home, and CLI layers apply in that order. A patch config replaces the whole config. JSON Schema defaults are annotations; Schemastery validates a fallback on null/omission unless required rejects it first. Schemastery unions select the first successful branch. Root patch id constraints describe the current index, not ids introduced or changed by earlier patches. Include-local patches have a separate index. Relative plugin insertions require their source patch directory; this document does not infer future overlay locations."
    );
}
#[test]
fn sanitize_nonobject_manifest_names_the_file_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("package.json");
    std::fs::write(&path, "[]").unwrap();
    let error =
        hoh_loader::profile_sanitize::sanitize_profile("test", dir.path(), &[], 1).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "test: profile manifest {} must hold a JSON object",
            path.display()
        )
    );
}
#[test]
fn sanitize_read_error_names_the_manifest_and_preserves_node_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("package.json");
    std::fs::create_dir(&path).unwrap();
    let error =
        hoh_loader::profile_sanitize::sanitize_profile("test", dir.path(), &[], 1).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "test: failed to read profile manifest {}: Error: EISDIR: illegal operation on a directory, read",
            path.display()
        )
    );
}
#[test]
fn sanitize_json_parse_error_has_no_bin_prefix() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{broken").unwrap();
    let expected = hoh_util_values::js_json_parse("{broken")
        .unwrap_err()
        .to_string();
    let error =
        hoh_loader::profile_sanitize::sanitize_profile("test", dir.path(), &[], 1).unwrap_err();
    assert_eq!(error.to_string(), expected);
}
#[test]
fn sanitize_spreads_nonobject_dsh_and_profile_like_javascript() {
    let dir = tempfile::tempdir().unwrap();
    for input in [json!({"dsh":"xy"}), json!({"dsh":{"profile":["a","b"]}})] {
        std::fs::write(dir.path().join("package.json"), input.to_string()).unwrap();
        hoh_loader::profile_sanitize::sanitize_profile("test", dir.path(), &["base".into()], 1)
            .unwrap();
        let actual: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
        )
        .unwrap();
        if input["dsh"].is_string() {
            assert_eq!(actual["dsh"]["0"], "x");
            assert_eq!(actual["dsh"]["1"], "y");
        } else {
            assert_eq!(actual["dsh"]["profile"]["0"], "a");
            assert_eq!(actual["dsh"]["profile"]["1"], "b");
        }
    }
}
#[tokio::test(start_paused = true)]
async fn metadata_and_volatile_changes_take_the_ordinary_update_path() {
    let world = World::new("- {id: x, name: objects, disabled: false, config: {title: old}}").await;
    let changes = world
        .patch("- {id: x, disabled: null, config: {title: new}}")
        .await
        .unwrap();
    assert!(changes.contains(&EntryChange::Updated(
        "include:x".into(),
        UpdateAction::Config
    )));
    assert_eq!(locked(&world.observed.configs).len(), 2);
    assert!(locked(&world.observed.events).is_empty());
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn integer_index_ids_start_in_javascript_property_order() {
    let world=World::new("- {id: '10', name: noop, config: 10}\n- {id: '2', name: noop, config: 2}\n- {id: '01', name: noop, config: 1}\n- {id: last, name: noop, config: 0}").await;
    assert_eq!(
        *locked(&world.observed.values),
        vec![json!(2), json!(10), json!(1), json!(0)]
    );
    world.close().await;
}
