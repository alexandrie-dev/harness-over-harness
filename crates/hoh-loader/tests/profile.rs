mod support;
use hoh_loader::*;
use serde_json::json;
use support::*;
#[test]
fn joins_the_home_and_rejects_traversal_shaped_names() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_profile_dir("tui", home.path()).unwrap(),
        home.path().join("profiles/tui")
    );
    for name in ["", ".", "..", "a/b", "a\\b", "node_modules"] {
        assert_eq!(
            resolve_profile_dir(name, home.path())
                .unwrap_err()
                .to_string(),
            format!(
                "dsh: invalid profile name {}",
                hoh_util_values::js_json_stringify(&json!(name))
            )
        );
    }
}
#[test]
fn creates_manifest_user_patch_layer_and_pnpm_workspace_once_never_overwriting() {
    let home = tempfile::tempdir().unwrap();
    let dir = resolve_profile_dir("tui", home.path()).unwrap();
    init_profile(&dir, &["base".into()]).unwrap();
    let manifest = read_profile_manifest("t", &dir).unwrap();
    assert_eq!(
        manifest,
        json!({"name":"dsh-profile-tui","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["base"]}}})
    );
    let files: [(&str, String); 3] = ["package.json", "cordis.patch.yml", "pnpm-workspace.yaml"]
        .map(|file| (file, std::fs::read_to_string(dir.join(file)).unwrap()));
    assert!(
        files[1].1.starts_with(
            "# Your patch layer for this dsh profile, applied after every bundle layer:"
        )
    );
    assert_eq!(
        files[2].1,
        "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n"
    );
    std::fs::write(dir.join("cordis.patch.yml"), "- id: x\n  config: {}\n").unwrap();
    init_profile(&dir, &["other".into()]).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("package.json")).unwrap(),
        files[0].1
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("cordis.patch.yml")).unwrap(),
        "- id: x\n  config: {}\n"
    );
}
#[test]
fn writes_and_reads_back_and_fails_loud_on_a_broken_manifest() {
    let dir = tempfile::tempdir().unwrap();
    write_profile_manifest(
        dir.path(),
        &json!({"name":"p","dsh":{"profile":{"bundles":["a"]}}}),
    )
    .unwrap();
    assert_eq!(
        read_profile_manifest("t", dir.path()).unwrap()["dsh"]["profile"]["bundles"],
        json!(["a"])
    );
    std::fs::write(dir.path().join("package.json"), "[]").unwrap();
    assert_eq!(
        read_profile_manifest("t", dir.path())
            .unwrap_err()
            .to_string(),
        format!(
            "t: profile manifest {} must hold a JSON object",
            dir.path().join("package.json").display()
        )
    );
    assert!(
        read_profile_manifest("t", &dir.path().join("nope"))
            .unwrap_err()
            .to_string()
            .starts_with("t: failed to read profile manifest ")
    );
}
#[test]
fn auto_initializes_only_shipped_templates_and_fails_loud_otherwise() {
    let home = tempfile::tempdir().unwrap();
    let source = Source(vec![]);
    let error =
        load_profile("t", "custom", &source, home.path(), &[], &env(home.path())).unwrap_err();
    assert_eq!(
        error.to_string(),
        "t: profile \"custom\" does not exist; create it with 'dsh plugin --profile custom add <package>'"
    );
    load_profile("t", "web", &source, home.path(), &[], &env(home.path())).unwrap();
    assert_eq!(
        read_profile_manifest("t", &resolve_profile_dir("web", home.path()).unwrap()).unwrap()["dsh"]
            ["profile"]["bundles"],
        json!(["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"])
    );
}
#[test]
fn named_profile_discovery_resolves_a_relative_home_through_eval_env() {
    let root = tempfile::tempdir().unwrap();
    let relative = std::path::Path::new(root.path().file_name().unwrap());
    let expected = root.path().join(relative).join("profiles/custom");
    std::fs::create_dir_all(&expected).unwrap();
    write_profile_manifest(&expected, &json!({"dsh":{"profile":{"bundles":[]}}})).unwrap();
    let profile = load_profile(
        "test",
        "custom",
        &Source(vec![]),
        relative,
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert_eq!(profile.dir, expected);
}
#[test]
fn normalizes_only_the_exact_installation_owned_headless_bundle_tuple() {
    let home = tempfile::tempdir().unwrap();
    let dir = resolve_profile_dir("headless", home.path()).unwrap();
    let stock = vec![
        "@deepseek-ai/dsh-base".into(),
        "@deepseek-ai/dsh-web-app".into(),
        "@deepseek-ai/dsh-headless".into(),
    ];
    init_profile(&dir, &stock).unwrap();
    load_profile(
        "t",
        "headless",
        &Source(vec![]),
        home.path(),
        &[],
        &env(home.path()),
    )
    .unwrap();
    assert_eq!(
        read_profile_manifest("t", &dir).unwrap()["dsh"]["profile"]["bundles"],
        json!(["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-headless"])
    );
    let custom = [stock, vec!["custom-bundle".into()]].concat();
    write_profile_manifest(
        &dir,
        &json!({"private":true,"dsh":{"profile":{"bundles":custom}}}),
    )
    .unwrap();
    let before = std::fs::read(dir.join("package.json")).unwrap();
    load_profile(
        "t",
        "headless",
        &Source(vec![]),
        home.path(),
        &[],
        &env(home.path()),
    )
    .unwrap();
    assert_eq!(std::fs::read(dir.join("package.json")).unwrap(), before);
}
#[test]
fn removes_retired_bundles_and_keeps_the_rest_of_an_application_owned_manifest() {
    let dir = tempfile::tempdir().unwrap();
    for retired in [
        "@deepseek-ai/dsh-experimental-schedule-bundle",
        "@deepseek-ai/dsh-experimental-agent-team-profile",
    ] {
        write_profile_manifest(dir.path(),&json!({"dependencies":{"custom-bundle":"^1.0.0"},"custom":true,"dsh":{"profile":{"bundles":["base",retired,"custom-bundle"]}}})).unwrap();
        let profile = Profile::load(
            dir.path(),
            &Source(vec![bundle("base", "[]"), bundle("custom-bundle", "[]")]),
            &dir.path().join("home"),
            &[],
            &env(dir.path()),
        )
        .unwrap();
        assert!(profile.skipped.is_empty());
        let manifest = read_profile_manifest("t", dir.path()).unwrap();
        assert_eq!(
            manifest["dsh"]["profile"]["bundles"],
            json!(["base", "custom-bundle"])
        );
        assert_eq!(manifest["dependencies"], json!({"custom-bundle":"^1.0.0"}));
        assert_eq!(manifest["custom"], true);
    }
}
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
    for manifest in ["[]", "{broken", r#"{"dsh":{"profile":{"bundles":5}}}"#] {
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
    assert_eq!(warnings, ["[a] patch: entry \"absent\" not found"]);
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

#[test]
fn optional_manifest_chain_accepts_missing_or_null_at_each_level() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    for manifest in [
        json!({}),
        json!({"dsh":null}),
        json!({"dsh":{}}),
        json!({"dsh":{"profile":null}}),
        json!({"dsh":{"profile":{}}}),
        json!({"dsh":{"profile":{"bundles":null}}}),
        json!({"dsh":false}),
        json!({"dsh":{"profile":42}}),
    ] {
        std::fs::write(dir.join("package.json"), manifest.to_string()).unwrap();
        let profile = Profile::load(
            &dir,
            &Source(vec![]),
            &root.path().join("home"),
            &[],
            &env(root.path()),
        )
        .unwrap();
        assert!(profile.layers.is_empty());
        assert!(profile.skipped.is_empty());
    }
}
#[test]
fn manifest_uses_js_json_parser_beyond_serde_default_depth() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    let text = format!(
        r#"{{"unused":{}0{},"dsh":{{"profile":{{"bundles":null}}}}}}"#,
        "[".repeat(129),
        "]".repeat(129)
    );
    std::fs::write(dir.join("package.json"), text).unwrap();
    let profile = Profile::load(
        &dir,
        &Source(vec![]),
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert!(profile.layers.is_empty());
}
#[test]
fn nested_telemetry_row_does_not_emit_a_launcher_patch() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["base"]);
    let source = Source(vec![bundle(
        "base",
        "- insert: [{id: g, name: cordis:group, group: true, config: [{id: session-telemetry-otel, name: p}]}]",
    )]);
    let mut environment = env(root.path());
    environment.var =
        std::sync::Arc::new(|name| (name == "DSH_TELEMETRY_DISABLED").then(|| "1".into()));
    let profile =
        Profile::load(&dir, &source, &root.path().join("home"), &[], &environment).unwrap();
    assert!(
        !profile
            .layers
            .iter()
            .any(|l| matches!(l.source, LayerSource::Launcher(_)))
    );
}
#[test]
fn repeated_loads_read_current_files_when_telemetry_variable_is_unset() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["base"]);
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let source = Source(vec![bundle(
        "base",
        "- insert: [{id: session-telemetry-otel, name: p, disabled: false}]",
    )]);
    let overlay = root.path().join("overlay.yml");
    std::fs::write(
        &overlay,
        "- id: session-telemetry-otel\n  disabled: false\n",
    )
    .unwrap();
    let load = |overlays: &[std::path::PathBuf]| {
        let profile = Profile::load(&dir, &source, &home, overlays, &env(root.path())).unwrap();
        assert!(
            !profile
                .layers
                .iter()
                .any(|l| matches!(l.source, LayerSource::Launcher(_)))
        );
        compose(&[], &profile.layers, &mut |w| panic!("{w}")).entries[0]
            .disabled
            .clone()
    };
    assert_eq!(load(&[]), json!(false));
    std::fs::write(
        dir.join("cordis.patch.yml"),
        "- id: session-telemetry-otel\n  disabled: true\n",
    )
    .unwrap();
    assert_eq!(load(&[]), json!(true));
    assert_eq!(load(std::slice::from_ref(&overlay)), json!(false));
    std::fs::write(
        home.join("cordis.patch.yml"),
        "- id: session-telemetry-otel\n  disabled: false\n",
    )
    .unwrap();
    assert_eq!(load(&[]), json!(false));
    std::fs::write(home.join("cordis.patch.yml"), "[]\n").unwrap();
    assert_eq!(load(&[]), json!(true));
    std::fs::write(
        dir.join("cordis.patch.yml"),
        "- id: session-telemetry-otel\n  disabled: false\n",
    )
    .unwrap();
    assert_eq!(load(&[]), json!(false));
    std::fs::write(&overlay, "- id: session-telemetry-otel\n  disabled: true\n").unwrap();
    assert_eq!(load(&[overlay]), json!(true));
}
#[test]
fn multiple_files_of_one_bundle_have_one_provenance_boundary() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["base", "multi"]);
    let mut multi = bundle("multi", "- id: a\n  config: {v: 1}");
    multi
        .files
        .push(("second.yml".into(), "- id: a\n  config: {v: 2}".into()));
    let profile = Profile::load(
        &dir,
        &Source(vec![
            bundle("base", "- insert: [{id: a, name: p, config: {v: 0}}]"),
            multi,
        ]),
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert_eq!(profile.layers.len(), 2);
    let result = compose(&[], &profile.layers, &mut |w| panic!("{w}"));
    assert_eq!(result.origins["a"].patched_by, ["multi"]);
    assert_eq!(result.entries[0].config, json!({"v":2}));
}
#[test]
fn absolute_bundle_segments_follow_node_join_instead_of_replacing_the_base() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &["b", "/virtual"]);
    let mut b = bundle("b", "");
    b.files = vec![(
        "/layers/one.yml".into(),
        "- insert: [{id: a, name: ./plugin.js}]".into(),
    )];
    let mut virtual_bundle = bundle("/virtual", "");
    virtual_bundle.dir = None;
    virtual_bundle.files = vec![(
        "/layers/two.yml".into(),
        "- insert: [{id: b, name: ./plugin.js}]".into(),
    )];
    let profile = Profile::load(
        &dir,
        &Source(vec![b, virtual_bundle]),
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert!(profile.skipped.is_empty());
    assert_eq!(
        profile.layers[0].file.as_deref(),
        Some(std::path::Path::new("/bundles/b/layers/one.yml"))
    );
    assert_eq!(
        profile.layers[1].file.as_ref().unwrap(),
        &dir.join(".hoh-bundles/virtual/layers/two.yml")
    );
    let result = compose(&[], &profile.layers, &mut |w| panic!("{w}"));
    assert_eq!(result.entries[0].name, "file:///bundles/b/layers/plugin.js");
}

#[test]
fn skips_non_string_bundle_elements_with_the_source_reason_and_loads_valid_neighbors() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_profile(root.path(), &[]);
    let manifest = dir.join("package.json");
    std::fs::write(&manifest, r#"{"dsh":{"profile":{"bundles":["a",5,"b"]}}}"#).unwrap();
    let saved = std::fs::read(&manifest).unwrap();
    let source = Source(vec![
        bundle("a", "- insert: [{id: a, name: p}]"),
        bundle("b", "- insert: [{id: b, name: p}]"),
    ]);
    let profile = Profile::load(
        &dir,
        &source,
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert_eq!(profile.layers.len(), 2);
    assert!(matches!(&profile.layers[0].source,LayerSource::Bundle(name) if name=="a"));
    assert!(matches!(&profile.layers[1].source,LayerSource::Bundle(name) if name=="b"));
    assert_eq!(profile.skipped.len(), 1);
    assert_eq!(profile.skipped[0].name, "5");
    assert_eq!(
        profile.skipped[0].reason,
        "TypeError [ERR_INVALID_ARG_TYPE]: The \"request\" argument must be of type string. Received type number (5)"
    );
    let rows = compose(&[], &profile.layers, &mut |w| panic!("{w}")).entries;
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(std::fs::read(&manifest).unwrap(), saved);
    std::fs::write(&manifest, r#"{"dsh":{"profile":{"bundles":[-0]}}}"#).unwrap();
    let zero = Profile::load(
        &dir,
        &source,
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert_eq!(
        zero.skipped[0].reason,
        "TypeError [ERR_INVALID_ARG_TYPE]: The \"request\" argument must be of type string. Received type number (-0)"
    );
}
