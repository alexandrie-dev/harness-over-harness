mod support;
use hoh_loader::*;
use serde_json::json;
use std::path::Path;
use support::*;
#[test]
fn returns_undefined_when_no_user_patch_file_exists() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    let p = Profile::load(
        &dir,
        &Source(vec![]),
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert!(p.layers.is_empty());
}
#[test]
fn parses_a_patch_list_and_preserves_js_expressions_as_loader_expression_nodes() {
    let p = patches(
        "- id: agent-loop\n  name: '@deepseek-ai/dsh-agent-loop'\n  config:\n    model: !!js process.env.DSH_SPEC_MODEL\n- insert: [{id: llm, name: '@deepseek-ai/dsh-llm-pi-ai'}]\n",
    );
    assert_eq!(p.len(), 2);
    assert_eq!(
        p[0].set["config"]["model"],
        json!({"__jsExpr":"process.env.DSH_SPEC_MODEL"})
    );
    assert_eq!(p[1].insert.as_ref().unwrap().len(), 1);
}
#[test]
fn anchors_inserted_relative_plugins_to_the_patch_file_and_keeps_assertion_names_literal() {
    let p = patches(
        "- id: existing\n  name: ./assertion.mjs\n- insert:\n    - id: rule\n      name: ./rule.mjs\n    - id: nested\n      name: cordis:group\n      group: true\n      config: [{id: child, name: ../child.mjs}]\n",
    );
    assert_eq!(p[0].name.as_deref(), Some("./assertion.mjs"));
    let rows = p[1].insert.as_ref().unwrap();
    assert_eq!(rows[0].name, "file:///fixtures/rule.mjs");
    assert_eq!(rows[1].config[0]["name"], json!("file:///child.mjs"));
}
#[test]
fn loads_absolute_plugin_paths_from_patch_files_as_file_urls() {
    let p = patches(
        "- id: existing\n  name: '/absolute #100%.mjs'\n- insert: [{id: a, name: '/absolute #100%.mjs'}, {id: b, name: 'file:///a%20b.mjs'}, {id: c, name: '@dsh/p'}]\n",
    );
    assert_eq!(p[0].name.as_deref(), Some("/absolute #100%.mjs"));
    let rows = p[1].insert.as_ref().unwrap();
    assert_eq!(rows[0].name, "file:///absolute%20%23100%25.mjs");
    assert_eq!(rows[1].name, "file:///a%20b.mjs");
    assert_eq!(rows[2].name, "@dsh/p");
}
#[test]
fn fails_loud_on_an_unreadable_file_a_present_user_patch_layer_is_never_skipped() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    std::fs::create_dir(dir.join("cordis.patch.yml")).unwrap();
    assert!(
        Profile::load(
            &dir,
            &Source(vec![]),
            &root.path().join("home"),
            &[],
            &env(root.path())
        )
        .is_err()
    );
}
#[test]
fn fails_loud_on_unparsable_yaml_and_on_a_js_tag_with_no_expression_body() {
    for text in ["invalid: [unclosed\n", "- id: x\n  config:\n    a: !!js\n"] {
        assert!(parse_patches(text, Path::new("/bad.yml")).is_err());
    }
}
#[test]
fn fails_loud_when_the_file_is_not_a_top_level_array_or_an_entry_is_not_an_object() {
    for text in [
        "id: not-a-list\n",
        "- just-a-string\n",
        "- []\n",
        "- null\n",
    ] {
        assert!(parse_patches(text, Path::new("/bad.yml")).is_err());
    }
}
#[test]
fn missing_explicit_overlays_fail_instead_of_being_optional() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    assert!(
        Profile::load(
            &dir,
            &Source(vec![]),
            &root.path().join("home"),
            &[root.path().join("missing.yml")],
            &env(root.path())
        )
        .is_err()
    );
}
