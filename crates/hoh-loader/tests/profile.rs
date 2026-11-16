mod support;
use hoh_loader::*;
use serde_json::json;
use support::*;
#[test]
fn loads_an_explicitly_owned_profile_directory_outside_cli_discovery() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["a"]);
    let profile = Profile::load(
        &dir,
        &Source(vec![bundle("a", "[]")]),
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert_eq!(profile.dir, dir);
    assert_eq!(profile.name, "custom");
    assert_eq!(profile.layers.len(), 1);
    assert!(profile.skipped.is_empty());
}
#[test]
fn resolves_each_dsh_profile_bundles_entry_to_its_patch_layer_in_order_plus_the_user_layer() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["a", "b"]);
    std::fs::write(dir.join("cordis.patch.yml"), "- id: a\n  config: {v: 3}\n").unwrap();
    let source = Source(vec![
        bundle("a", "- insert: [{id: a, name: pkg-a}]"),
        bundle("b", "- id: a\n  config: {v: 2}"),
    ]);
    let profile = Profile::load(
        &dir,
        &source,
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert!(matches!(profile.layers[0].source,LayerSource::Bundle(ref n) if n=="a"));
    assert!(matches!(profile.layers[1].source,LayerSource::Bundle(ref n) if n=="b"));
    assert!(matches!(profile.layers[2].source, LayerSource::Profile));
    let tree = compose(&[], &profile.layers, &mut |w| panic!("{w}"));
    assert_eq!(tree.entries[0].config, json!({"v":3}));
}
#[test]
fn applies_a_dsh_bundle_patch_list_in_order_anchoring_inserted_paths_beside_each_file() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["multi"]);
    let source = Source(vec![Bundle {
        files: vec![
            (
                "first.patch.yml".into(),
                "- insert: [{id: a, name: ./local.js, config: {v: 1}}]".into(),
            ),
            (
                "layers/second.patch.yml".into(),
                "- id: a\n  config: {v: 2}\n- insert: [{id: b, name: ./local.js}]".into(),
            ),
        ],
        ..bundle("multi", "")
    }]);
    let profile = Profile::load(
        &dir,
        &source,
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    let rows = compose(&[], &profile.layers, &mut |w| panic!("{w}")).entries;
    assert_eq!(rows[0].name, "file:///bundles/multi/local.js");
    assert_eq!(rows[0].config, json!({"v":2}));
    assert_eq!(rows[1].name, "file:///bundles/multi/layers/local.js");
}
#[test]
fn skips_an_unsupported_saved_bundle_before_composing_any_runtime_overrides_and_retains_the_selection()
 {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["base", "restricted"]);
    let before = std::fs::read(dir.join("package.json")).unwrap();
    let mut restricted = bundle("restricted", "- id: runtime\n  config: {mode: persistent}");
    restricted.platforms = Some(vec!["darwin".into()]);
    let source = Source(vec![
        bundle(
            "base",
            "- insert: [{id: runtime, name: p, config: {mode: isolated}}]",
        ),
        restricted,
    ]);
    let profile = Profile::load(
        &dir,
        &source,
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert_eq!(profile.skipped.len(), 1);
    assert_eq!(profile.skipped[0].name, "restricted");
    assert!(profile.skipped[0].reason.contains("Host is running linux"));
    assert_eq!(
        compose(&[], &profile.layers, &mut |_| {}).entries[0].config,
        json!({"mode":"isolated"})
    );
    assert_eq!(std::fs::read(dir.join("package.json")).unwrap(), before);
}
#[test]
fn skips_a_bundle_with_invalid_patch_retains_selections_and_retries_it_on_reread() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["before", "missing", "broken", "after"]);
    let source = Source(vec![
        bundle("before", "- insert: [{id: a, name: p}]"),
        bundle("broken", "[invalid"),
        bundle("after", "- id: a\n  disabled: true"),
    ]);
    let profile = Profile::load(
        &dir,
        &source,
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert_eq!(
        profile
            .skipped
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["missing", "broken"]
    );
    assert_eq!(profile.layers.len(), 2);
    let mut source = source;
    source.0.push(bundle("missing", "[]"));
    source.0[1] = bundle("broken", "[]");
    assert!(
        Profile::load(
            &dir,
            &source,
            &root.path().join("home"),
            &[],
            &env(root.path())
        )
        .unwrap()
        .skipped
        .is_empty()
    );
}
#[test]
fn still_rejects_invalid_profile_manifests_and_user_patches() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    let source = Source(vec![]);
    for manifest in [
        "[]",
        "{broken",
        r#"{"dsh":{"profile":{"bundles":[1]}}}"#,
        r#"{"dsh":{"profile":{"bundles":null}}}"#,
    ] {
        std::fs::write(dir.join("package.json"), manifest).unwrap();
        assert!(
            Profile::load(
                &dir,
                &source,
                &root.path().join("home"),
                &[],
                &env(root.path())
            )
            .is_err()
        );
    }
    std::fs::write(dir.join("package.json"), "{}").unwrap();
    std::fs::write(dir.join("cordis.patch.yml"), "invalid").unwrap();
    assert!(
        Profile::load(
            &dir,
            &source,
            &root.path().join("home"),
            &[],
            &env(root.path())
        )
        .is_err()
    );
}
#[test]
fn applies_layers_over_an_empty_root_and_reports_skipped_patches() {
    let mut warnings = Vec::new();
    let result = compose(
        &[],
        &[
            layer(
                "a",
                "- id: absent\n  disabled: true\n- insert: [{id: a, name: p}]",
            ),
            layer("b", "- id: a\n  config: {v: 1}"),
        ],
        &mut |w| warnings.push(w),
    );
    assert_eq!(warnings, ["[a] patch: entry absent not found"]);
    assert_eq!(result.entries[0].config, json!({"v":1}));
}
#[test]
fn composes_current_files_from_profile_data_and_retains_launch_overlay_and_telemetry_precedence() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["base"]);
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let source = Source(vec![bundle(
        "base",
        "- insert: [{id: session-telemetry-otel, name: p, disabled: false, config: {v: base}}]",
    )]);
    std::fs::write(
        dir.join("cordis.patch.yml"),
        "- id: session-telemetry-otel\n  config: {v: profile}\n",
    )
    .unwrap();
    std::fs::write(
        home.join("cordis.patch.yml"),
        "- id: session-telemetry-otel\n  config: {v: home}\n",
    )
    .unwrap();
    let overlay = root.path().join("overlay.yml");
    std::fs::write(
        &overlay,
        "- id: session-telemetry-otel\n  config: {v: overlay}\n  disabled: false\n",
    )
    .unwrap();
    let mut environment = env(root.path());
    environment.var = std::sync::Arc::new(|s| (s == "DSH_TELEMETRY_DISABLED").then(|| "0".into()));
    let profile = Profile::load(&dir, &source, &home, &[overlay], &environment).unwrap();
    let rows = compose(&[], &profile.layers, &mut |w| panic!("{w}")).entries;
    assert_eq!(rows[0].config, json!({"v":"overlay"}));
    assert_eq!(rows[0].disabled, json!(true));
    assert_eq!(profile.layers.len(), 5);
    let dir = write_profile(root.path(), &[]);
    let profile = Profile::load(
        &dir,
        &Source(vec![]),
        &root.path().join("none"),
        &[],
        &environment,
    )
    .unwrap();
    assert!(
        !profile
            .layers
            .iter()
            .any(|l| matches!(l.source, LayerSource::Launcher(_)))
    );
}
#[test]
fn rejects_a_bundle_atomically_if_a_later_file_is_broken() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["broken"]);
    let mut b = bundle("broken", "- insert: [{id: a, name: p}]");
    b.files.push(("bad.yml".into(), "[invalid".into()));
    let profile = Profile::load(
        &dir,
        &Source(vec![b]),
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert!(profile.layers.is_empty());
    assert_eq!(profile.skipped.len(), 1);
}
