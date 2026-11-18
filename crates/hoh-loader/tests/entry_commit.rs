mod support;
use hoh_loader::*;
use serde_json::json;
use support::live::*;
use support::*;

#[tokio::test(start_paused = true)]
async fn reports_assigned_ids_and_applied_options_for_create_update_move_and_remove() {
    let world = World::new("[]").await;
    let id = world
        .loader
        .create(
            entries("- {name: noop, config: {value: 1}}\n").remove(0),
            None,
        )
        .await
        .unwrap();
    world.loader.settled().await;
    assert!(!id.is_empty());
    let created = world.loader.resolve(&id).unwrap();
    assert_eq!(created.options.id, id);
    assert_eq!(created.options.config, serde_json::json!({"value":1}));
    assert!(created.fiber.is_some());
    world.loader.remove(&id).await.unwrap();
    world.loader.settled().await;
    assert!(world.loader.resolve(&id).is_none());
    assert!(created.fiber.unwrap().is_disposed());
    // move and journal reports are excluded by O4; raw options are inspected through EntryInfo.
    world.close().await;
}
#[tokio::test(start_paused = true)]
async fn mounts_explicit_ids_containing_the_separator_without_nested_path_resolution() {
    let world = World::new("[]").await;
    let id = world
        .loader
        .create(entries("- {id: 'a:b', name: noop}\n").remove(0), None)
        .await
        .unwrap();
    assert_eq!(id, "a:b");
    world.loader.settled().await;
    assert!(world.loader.entries().iter().any(|e| {
        e.id == "a:b"
            && e.fiber
                .as_ref()
                .is_some_and(|f| f.state() == hoh_kernel::FiberState::Active)
    }));
    assert!(world.loader.resolve("a:b").is_none());
    world.close().await;
}
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

#[test]
fn null_patch_controls_use_the_patch_algebra() {
    let patches = patches("- {id: ~, name: ~, insert: ~, config: {}}\n");
    assert!(patches[0].id.is_none() && patches[0].name.is_none() && patches[0].insert.is_none());
    let mut warnings = Vec::new();
    assert!(apply_patches(&[], &patches, &mut |w| warnings.push(w)).is_empty());
    assert_eq!(warnings, ["patch: id is required for non-insert patches"]);
}
#[test]
fn composed_ids_share_one_domain_across_base_and_independently_parsed_patch_files() {
    let file = std::path::Path::new("/same.yml");
    let base = parse_entries("- name: p\n", file).unwrap();
    let a = parse_patches(
        "- insert: [{name: p}, {name: cordis:group, group: true, config: [{name: p}]}]",
        file,
    )
    .unwrap();
    let b = parse_patches("- insert: [{name: p}]", file).unwrap();
    assert_eq!(
        a[0].insert.as_ref().unwrap()[0].id,
        b[0].insert.as_ref().unwrap()[0].id
    );
    assert!(a[0].insert.as_ref().unwrap()[0].id.is_empty()); // Parsing preserves anonymous rows.
    let layers = [
        Layer {
            source: LayerSource::Launcher("a"),
            file: None,
            patches: a,
        },
        Layer {
            source: LayerSource::Launcher("b"),
            file: None,
            patches: b,
        },
    ];
    let original = base.clone();
    let result = compose(&base, &layers, &mut |w| panic!("{w}"));
    let mut ids = std::collections::HashSet::new();
    fn visit(rows: &serde_json::Value, ids: &mut std::collections::HashSet<String>) {
        for row in rows.as_array().unwrap() {
            let id = row["id"].as_str().unwrap();
            assert!(!id.is_empty());
            assert!(ids.insert(id.into()), "duplicate {id}");
            if row["group"] == json!(true) {
                visit(&row["config"], ids);
            }
        }
    }
    visit(&serde_json::to_value(&result.entries).unwrap(), &mut ids);
    assert_eq!(ids.len(), 5);
    assert_eq!(base, original);
    assert_eq!(result.origins.len(), ids.len());
    let all = layers
        .iter()
        .flat_map(|l| l.patches.iter().cloned())
        .collect::<Vec<_>>();
    assert_eq!(apply_patches(&base, &all, &mut |_| {}), result.entries);
}

#[test]
fn duplicate_explicit_ids_are_preserved_and_the_source_index_targets_the_last_row() {
    let base = entries("- id: duplicate\n  name: original\n  config: {v: base}\n");
    let patches = patches(
        "- insert: [{id: duplicate, name: inserted}]\n- id: duplicate\n  name: inserted\n  config: {v: changed}\n",
    );
    let result = apply_patches(&base, &patches, &mut |w| panic!("{w}"));
    assert_eq!(result[0].id, "duplicate");
    assert_eq!(result[1].id, "duplicate");
    assert_eq!(result[0].config, json!({"v":"base"}));
    assert_eq!(result[1].config, json!({"v":"changed"}));
}

#[test]
fn anonymous_base_rows_keep_ids_and_origins_when_layers_insert_before_them() {
    let mut base = entries(
        "- id: before_root\n  name: cordis:group\n  group: true\n  config: []\n- name: base_root\n- id: g\n  name: cordis:group\n  group: true\n  config:\n    - id: before_child\n      name: cordis:group\n      group: true\n      config: []\n    - name: base_child\n",
    );
    base[1].id.clear();
    base[2].config[1]["id"] = json!("");
    let mut inserted = layer(
        "insert",
        "- id: before_root\n  insert: [{name: inserted_root}]\n- id: before_child\n  insert: [{name: inserted_child}]\n",
    );
    for patch in &mut inserted.patches {
        for row in patch.insert.iter_mut().flatten() {
            row.id.clear();
        }
    }
    fn id(rows: &serde_json::Value, name: &str) -> Option<String> {
        for row in rows.as_array()? {
            if row["name"] == json!(name) {
                return row["id"].as_str().map(str::to_owned);
            }
            if row["group"] == json!(true)
                && let Some(found) = id(&row["config"], name)
            {
                return Some(found);
            }
        }
        None
    }
    let before = compose(&base, &[], &mut |w| panic!("{w}"));
    let after = compose(&base, std::slice::from_ref(&inserted), &mut |w| {
        panic!("{w}")
    });
    let later = compose(
        &base,
        &[inserted, layer("later", "- id: g\n  disabled: true\n")],
        &mut |w| panic!("{w}"),
    );
    for name in ["base_root", "base_child"] {
        let original = id(&json_rows(&before.entries), name).unwrap();
        assert_eq!(
            id(&json_rows(&after.entries), name).unwrap(),
            original,
            "{name} shifted between snapshots"
        );
        assert_eq!(id(&json_rows(&later.entries), name).unwrap(), original);
        assert_eq!(after.origins[&original].created_by, "base");
        assert_eq!(later.origins[&original].created_by, "base");
    }
    for name in ["inserted_root", "inserted_child"] {
        let added = id(&json_rows(&after.entries), name).unwrap();
        assert_eq!(after.origins[&added].created_by, "insert");
        assert_eq!(id(&json_rows(&later.entries), name).unwrap(), added);
        assert_eq!(later.origins[&added].created_by, "insert");
    }
}

#[test]
fn generated_ids_reserve_all_explicit_ids_without_changing_explicit_duplicates() {
    let base = entries(
        "- id: '00000001'\n  name: first\n- id: '00000001'\n  name: second\n- name: anonymous_base\n",
    );
    let layers = [
        layer(
            "one",
            "- insert: [{id: '00000002', name: explicit}, {name: anonymous_insert}]",
        ),
        layer(
            "two",
            "- insert: [{id: '00000002', name: duplicate}, {name: anonymous_later}]",
        ),
    ];
    let composed = compose(&base, &layers, &mut |w| panic!("{w}"));
    assert_eq!(composed.entries[0].id, "00000001");
    assert_eq!(composed.entries[1].id, "00000001");
    assert_eq!(composed.entries[3].id, "00000002");
    assert_eq!(composed.entries[5].id, "00000002");
    let mut used = std::collections::HashSet::from(["00000001".to_owned(), "00000002".to_owned()]);
    for index in [2, 4, 6] {
        assert_eq!(composed.entries[index].id.len(), 8);
        assert!(used.insert(composed.entries[index].id.clone()));
    }
    assert!(base[2].id.is_empty());
}
#[test]
fn falsy_patch_controls_are_absent_and_truthy_numeric_ids_are_not_string_keys() {
    let base = entries("- id: '5'\n  name: p\n  config: {v: original}\n");
    let patches = patches(
        "- {id: 0, config: {}}\n- {id: false, config: {}}\n- {insert: false, config: {}}\n- {id: 5, config: {v: changed}}\n- {id: '', insert: 0, config: {}}\n- {id: null, insert: '', config: {}}\n",
    );
    for index in [0, 1, 2, 4, 5] {
        assert!(patches[index].id.is_none());
        assert!(patches[index].insert.is_none());
    }
    assert_eq!(patches[3].id, Some(json!(5)));
    let mut warnings = Vec::new();
    let output = apply_patches(&base, &patches, &mut |w| warnings.push(w));
    let required = "patch: id is required for non-insert patches";
    assert_eq!(
        warnings,
        [
            required,
            required,
            required,
            "patch: entry 5 not found",
            required,
            required
        ]
    );
    assert_eq!(output, base);
    let mut warnings = Vec::new();
    let layers = [Layer {
        source: LayerSource::Launcher("numeric"),
        file: None,
        patches: patches.clone(),
    }];
    compose(&base, &layers, &mut |w| warnings.push(w));
    assert_eq!(warnings[3], "[numeric] patch: entry 5 not found");
    let string_patch = patches_single_string_id();
    assert_eq!(
        apply_patches(&base, &string_patch, &mut |w| panic!("{w}"))[0].config,
        json!({"v":"changed"})
    );
    fn patches_single_string_id() -> Vec<Patch> {
        support::patches("- {id: '5', config: {v: changed}}")
    }
}
#[test]
fn anonymous_group_replacements_keep_their_creation_ids_across_prefixes() {
    let base = entries(
        "- id: earlier\n  name: cordis:group\n  group: true\n  config: []\n- id: g\n  name: cordis:group\n  group: true\n  config: []\n",
    );
    let first = layer("replacement", "- id: g\n  config: [{name: created}]\n");
    let second = layer(
        "insertion",
        "- id: earlier\n  insert: [{name: before_created}]\n",
    );
    let before = compose(&base, std::slice::from_ref(&first), &mut |w| panic!("{w}"));
    let after = compose(&base, &[first, second], &mut |w| panic!("{w}"));
    let id = before.entries[1].config[0]["id"].as_str().unwrap();
    assert_eq!(after.entries[1].config[0]["id"], json!(id));
    assert_eq!(after.origins[id].created_by, "replacement");
}
