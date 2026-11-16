mod support;
use hoh_loader::*;
use serde_json::json;
use support::*;
#[test]
fn detaches_file_entries_even_with_no_patches() {
    let base = entries("- id: x\n  name: p\n  config: {value: 1}\n");
    let mut tree = apply_patches(&base, &[], &mut |_| {});
    tree[0].config["value"] = json!(2);
    assert_eq!(base[0].config, json!({"value":1}));
}
#[test]
fn clones_inserted_groups_and_lets_later_patches_address_their_descendants() {
    let patches = patches(
        "- insert:\n    - id: group\n      name: cordis:group\n      group: true\n      config:\n        - id: child\n          name: p\n          config: {v: 1}\n- id: child\n  config: {v: 2}\n",
    );
    let tree = apply_patches(&[], &patches, &mut |w| panic!("{w}"));
    assert_eq!(tree[0].config[0]["config"], json!({"v":2}));
    assert_eq!(
        patches[0].insert.as_ref().unwrap()[0].config[0]["config"],
        json!({"v":1})
    );
}
#[test]
fn warning_texts_are_exact_and_each_skips_only_one_patch() {
    let base = entries("- id: a\n  name: p\n  config: {x: 1}\n");
    let patches = patches(
        "- id: absent\n  insert: []\n- id: a\n  insert: []\n- config: {}\n- id: absent\n  config: {}\n- id: a\n  name: wrong\n  config: {}\n- id: a\n  disabled: true\n",
    );
    let mut warnings = Vec::new();
    let tree = apply_patches(&base, &patches, &mut |w| warnings.push(w));
    assert_eq!(
        warnings,
        [
            "patch insert: entry absent not found",
            "patch insert: entry a is not a group",
            "patch: id is required for non-insert patches",
            "patch: entry absent not found",
            "patch: name mismatch for a (expected p, got wrong), skipping"
        ]
    );
    assert_eq!(tree[0].disabled, json!(true));
    assert_eq!(tree[0].config, base[0].config);
}
#[test]
fn set_replaces_whole_fields_and_null_removes_them() {
    let base =
        entries("- id: a\n  name: p\n  config: {x: 1, y: 2}\n  disabled: true\n  inject: [s]\n");
    let tree = apply_patches(
        &base,
        &patches("- id: a\n  config: {x: 3}\n  disabled: null\n  group: null\n  inject: null\n"),
        &mut |_| {},
    );
    assert_eq!(tree[0].config, json!({"x":3}));
    assert!(tree[0].disabled.is_null());
    assert!(!tree[0].group);
    assert!(tree[0].inject.is_none());
}
#[test]
fn inserts_append_to_root_and_group_and_later_patches_see_them() {
    let base = entries("- id: g\n  name: cordis:group\n  group: true\n");
    let tree = apply_patches(
        &base,
        &patches(
            "- id: g\n  insert:\n    - id: child\n      name: p\n- insert:\n    - id: root\n      name: p\n- id: child\n  disabled: true\n",
        ),
        &mut |w| panic!("{w}"),
    );
    assert_eq!(tree[1].id, "root");
    assert_eq!(tree[0].config[0]["disabled"], json!(true));
}
#[test]
fn explicit_ids_containing_the_separator_are_patch_targets() {
    let base = entries("- id: 'package:a'\n  name: p\n");
    let tree = apply_patches(
        &base,
        &patches("- id: 'package:a'\n  disabled: true\n"),
        &mut |w| panic!("{w}"),
    );
    assert_eq!(tree[0].disabled, json!(true));
}

#[test]
fn replacing_group_config_does_not_reindex_new_children_or_mutate_detached_old_children() {
    let base = entries(
        "- id: g\n  name: cordis:group\n  group: true\n  config: [{id: old, name: p, config: {v: 1}}]",
    );
    let patches = patches(
        "- id: g\n  config: [{id: new, name: p}]\n- id: old\n  config: {v: 2}\n- id: new\n  disabled: true\n- id: g\n  insert: [{id: inserted, name: p}]\n- id: inserted\n  disabled: true",
    );
    let mut warnings = Vec::new();
    let tree = apply_patches(&base, &patches, &mut |w| warnings.push(w));
    assert_eq!(warnings, ["patch: entry new not found"]);
    assert_eq!(tree[0].config[0], json!({"id":"new","name":"p"}));
    assert_eq!(tree[0].config[1]["disabled"], json!(true));
    assert_eq!(base[0].config[0]["config"], json!({"v":1}));
}
