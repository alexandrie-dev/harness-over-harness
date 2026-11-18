mod support;
use hoh_loader::*;
use serde_json::json;
use support::*;
#[test]
fn composes_overlay_layers_in_order_prints_js_verbatim_and_labels_each_section_with_its_source_and_patches()
 {
    let base =
        entries("- id: shared\n  name: p\n  config: {value: base}\n- id: untouched\n  name: p\n");
    let layers = [
        layer(
            "surface.yml",
            "- id: shared\n  config:\n    value: surface\n    key: !!js process.env.DSH_DUMP_SPEC\n- insert:\n    - id: surface-extra\n      name: p\n",
        ),
        layer("user.yml", "- id: surface-extra\n  config: {value: user}\n"),
    ];
    let composition = compose(&base, &layers, &mut |w| panic!("{w}"));
    let dump = render_dump(&composition);
    assert_eq!(entries(&dump), composition.entries);
    assert!(dump.contains("!!js \"process.env.DSH_DUMP_SPEC\""));
    assert!(dump.contains("# == shared · base · patched by surface.yml\n"));
    assert!(dump.contains("# == untouched · base\n"));
    assert!(dump.contains("# == surface-extra · surface.yml · patched by user.yml\n"));
    assert_eq!(composition.entries[0].config["value"], json!("surface"));
    assert_eq!(composition.entries[2].config, json!({"value":"user"}));
}
#[test]
fn groups_contiguous_rows_with_the_same_origin_and_patches_under_one_separator() {
    // Keep the source case name; binding L18 instead requires one id-labelled comment per row.
    let dump = render_dump(&compose(
        &entries("- id: a\n  name: p\n- id: b\n  name: p\n"),
        &[],
        &mut |_| {},
    ));
    assert_eq!(dump.matches("# == ").count(), 2);
}
#[tokio::test(start_paused = true)]
async fn composes_all_layers_as_one_flattened_patch_list_exactly_like_boot() {
    let base = entries("- id: g\n  name: cordis:group\n  group: true\n  config: []\n");
    let layers = [
        layer(
            "a.yml",
            "- id: g\n  config: [{id: child, name: p, config: {v: 1}}]\n",
        ),
        layer("b.yml", "- id: child\n  config: {v: 2}\n"),
    ];
    let mut warnings = Vec::new();
    let result = compose(&base, &layers, &mut |w| warnings.push(w));
    assert_eq!(warnings, ["[b.yml] patch: entry \"child\" not found"]);
    assert_eq!(result.entries[0].config[0]["config"]["v"], json!(1));
    assert_eq!(result.origins["g"].patched_by, ["a.yml"]);
    assert_eq!(
        apply_patches(
            &base,
            &layers
                .iter()
                .flat_map(|l| l.patches.clone())
                .collect::<Vec<_>>(),
            &mut |_| {}
        ),
        result.entries
    );
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("base.yml");
    std::fs::write(&file, json_rows(&base).to_string()).unwrap();
    let kernel = hoh_kernel::Kernel::start();
    let mut registry = PluginRegistry::default();
    registry.add::<support::live::Noop>("p");
    let loader = boot(
        &kernel.root(),
        BootOptions {
            registry: std::sync::Arc::new(registry),
            base: file,
            layers: layers.to_vec(),
            env: env(dir.path()),
        },
    )
    .await
    .unwrap();
    assert_eq!(loader.dump(), render_dump(&result));
    assert_eq!(
        *loader
            .resolve("include:child")
            .unwrap()
            .fiber
            .unwrap()
            .effective_config()
            .unwrap(),
        json!({"v":1})
    );
    let logged: Vec<_> = kernel
        .root()
        .logger()
        .buffer()
        .into_iter()
        .filter(|record| record.message.contains("patch:"))
        .map(|record| record.message)
        .collect();
    assert_eq!(logged, warnings);
    kernel.shutdown().await;
}
#[test]
fn reports_a_patch_whose_target_row_is_absent_through_warn_with_its_layer_label_and_keeps_composing()
 {
    let base = entries("- id: shared\n  name: p\n");
    let mut warnings = Vec::new();
    let result = compose(
        &base,
        &[layer(
            "overlay.yml",
            "- id: absent\n  config: {}\n- id: shared\n  config: {value: patched}\n",
        )],
        &mut |w| warnings.push(w),
    );
    assert_eq!(
        warnings,
        ["[overlay.yml] patch: entry \"absent\" not found"]
    );
    assert_eq!(result.entries[0].config, json!({"value":"patched"}));
}
#[tokio::test(start_paused = true)]
async fn fails_loud_on_a_missing_unparsable_or_non_array_base_config() {
    for text in ["invalid: [unclosed\n", "id: not-a-list\n"] {
        assert!(parse_entries(text, std::path::Path::new("/base.yml")).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let kernel = hoh_kernel::Kernel::start();
    let file = dir.path().join("missing.yml");
    let error = boot(
        &kernel.root(),
        BootOptions {
            registry: std::sync::Arc::new(PluginRegistry::default()),
            base: file.clone(),
            layers: vec![],
            env: env(dir.path()),
        },
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(error, LoadError::Startup(_)));
    assert!(
        error
            .to_string()
            .contains(&format!("config file not found: {}", file.display()))
    );
    kernel.shutdown().await;
}
#[test]
fn nested_changes_have_origins_and_unchanged_patches_add_no_provenance() {
    let base =
        entries("- id: g\n  name: cordis:group\n  group: true\n  config: [{id: child, name: p}]\n");
    let result = compose(
        &base,
        &[
            layer("a", "- id: child\n  disabled: true\n"),
            layer("b", "- id: child\n  disabled: true\n"),
        ],
        &mut |_| {},
    );
    assert_eq!(result.origins["g"].patched_by, ["a"]);
    assert_eq!(result.origins["child"].patched_by, ["a"]);
}
#[test]
fn an_empty_composition_is_a_loadable_empty_array() {
    assert_eq!(render_dump(&compose(&[], &[], &mut |_| {})), "[]\n");
}
