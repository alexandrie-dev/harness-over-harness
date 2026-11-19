mod support;

use hoh_agent_preset_registry::{
    AgentPresetRegistry, AgentPresets, Config as PresetConfig, PresetDefinition, PresetRuntime,
};
use hoh_kernel::Volatile;
use hoh_loader::{Entry, EvalEnv, PluginRegistry};
use hoh_plugin_package_inventory_deepseek::PluginPackages;
use serde_json::json;
use std::sync::Arc;
use support::{FIELD, Harness, Noop, file_url, package_plugin};

#[tokio::test]
async fn contributes_by_default_and_can_be_explicitly_disabled() {
    let enabled = Harness::start(None, |_, _| "[]\n".into()).await;
    assert!(enabled.fields(None).await.contains_key(FIELD));
    enabled.close().await;

    let disabled = Harness::start(Some(false), |_, _| "[]\n".into()).await;
    assert!(!disabled.fields(None).await.contains_key(FIELD));
    disabled.close().await;
}

#[tokio::test]
async fn reports_active_versions_once_retains_parallel_versions_and_excludes_inactive_or_loose() {
    let harness = Harness::start(Some(true), |root, registry| {
        let one_a = package_plugin(root, "one-a", json!({"name":"one","version":"1.0.0"}), registry);
        let one_b = package_plugin(root, "one-b", json!({"name":"one","version":"2.0.0"}), registry);
        let disabled = package_plugin(root, "disabled", json!({"name":"disabled","version":"1.0.0"}), registry);
        std::fs::create_dir_all(root.join("loose")).unwrap();
        std::fs::write(root.join("loose/plugin.mjs"), "").unwrap();
        registry.add::<Noop>("./loose/plugin.mjs");
        format!(
            "- {{id: a, name: {one_a}}}\n- {{id: duplicate, name: {one_a}}}\n- {{id: b, name: {one_b}}}\n- {{id: off, name: {disabled}, disabled: true}}\n- {{id: loose, name: ./loose/plugin.mjs}}\n"
        )
    })
    .await;
    assert_eq!(
        harness.fields(None).await[FIELD],
        json!({"version":1,"packages":[
            {"name":"one","version":"1.0.0"},
            {"name":"one","version":"2.0.0"}
        ]})
    );
    harness.close().await;
}

#[tokio::test]
async fn deduplicates_name_only_identities_separately_from_versioned_identities() {
    for reversed in [false, true] {
        let harness = Harness::start(Some(true), move |root, registry| {
            let versioned = package_plugin(
                root,
                "versioned",
                json!({"name":"same","version":"undefined"}),
                registry,
            );
            let unversioned =
                package_plugin(root, "unversioned", json!({"name":"same"}), registry);
            let (first, second) = if reversed {
                (unversioned, versioned)
            } else {
                (versioned, unversioned)
            };
            format!(
                "- {{id: first, name: {first}}}\n- {{id: second, name: {second}}}\n- {{id: third, name: {first}}}\n"
            )
        })
        .await;
        assert_eq!(
            harness.fields(None).await[FIELD],
            json!({"version":1,"packages":[
                {"name":"same"},
                {"name":"same","version":"undefined"}
            ]})
        );
        harness.close().await;
    }
}

#[tokio::test]
async fn omits_a_loose_module_whose_nearest_manifest_only_marks_module_type() {
    let harness = Harness::start(Some(true), |root, registry| {
        let marker = package_plugin(root, "marker", json!({"type":"module"}), registry);
        format!("- {{name: {marker}}}\n")
    })
    .await;
    assert_eq!(
        harness.fields(None).await[FIELD],
        json!({"version":1,"packages":[]})
    );
    harness.close().await;
}

#[tokio::test]
async fn omits_unavailable_version_metadata() {
    let harness = Harness::start(Some(true), |root, registry| {
        let versions = [
            json!(null),
            json!(""),
            json!("  "),
            json!(1),
            json!(false),
            json!({}),
            json!([]),
        ];
        let mut rows = String::new();
        for (index, version) in versions.into_iter().enumerate() {
            let module = package_plugin(
                root,
                &format!("invalid-{index}"),
                json!({"name":"optional-metadata","private": index % 2 == 0,"version":version}),
                registry,
            );
            rows.push_str(&format!("- {{id: row-{index}, name: {module}}}\n"));
        }
        rows
    })
    .await;
    assert_eq!(
        harness.fields(None).await[FIELD],
        json!({"version":1,"packages":[{"name":"optional-metadata"}]})
    );
    harness.close().await;
}

#[tokio::test]
async fn omits_unavailable_package_names() {
    let harness = Harness::start(Some(true), |root, registry| {
        let names = [json!(null), json!(""), json!("  "), json!(1)];
        let mut rows = String::new();
        for (index, name) in names.into_iter().enumerate() {
            let module = package_plugin(
                root,
                &format!("invalid-name-{index}"),
                json!({"name":name,"version":"1"}),
                registry,
            );
            rows.push_str(&format!("- {{id: row-{index}, name: {module}}}\n"));
        }
        rows
    })
    .await;
    assert_eq!(
        harness.fields(None).await[FIELD],
        json!({"version":1,"packages":[]})
    );
    harness.close().await;
}

#[tokio::test]
async fn retains_readable_packages_and_warns_at_most_once_for_an_unreadable_manifest() {
    for kind in ["invalid JSON", "null", "deleted"] {
        let harness = Harness::start(Some(true), move |root, registry| {
            let bad = package_plugin(root, "bad", json!({"name":"bad","version":"1"}), registry);
            let good = package_plugin(root, "good", json!({"name":"good","version":"2"}), registry);
            let manifest = root.join("bad/package.json");
            match kind {
                "deleted" => std::fs::remove_file(manifest).unwrap(),
                "null" => std::fs::write(manifest, "null").unwrap(),
                _ => std::fs::write(manifest, "{").unwrap(),
            }
            format!("- {{name: {bad}}}\n- {{name: {good}}}\n")
        })
        .await;
        for _ in 0..2 {
            assert_eq!(
                harness.fields(None).await[FIELD],
                json!({"version":1,"packages":[{"name":"good","version":"2"}]})
            );
        }
        let warnings: Vec<_> = harness
            .ctx
            .logger()
            .buffer()
            .into_iter()
            .filter(|record| {
                record
                    .message
                    .starts_with("plugin-package-inventory-deepseek: omitting unreadable")
            })
            .collect();
        assert_eq!(warnings.len(), usize::from(kind != "deleted"), "{kind}");
        if let Some(warning) = warnings.first() {
            assert!(warning.message.contains("./bad/plugin.mjs"));
        }
        harness.close().await;
    }
}

#[tokio::test]
async fn reports_private_file_and_bare_packages_then_adds_the_parent() {
    for kind in ["file", "bare"] {
        let harness = Harness::start(Some(true), move |root, registry| {
            let parent = package_plugin(
                root,
                "node_modules/inspector",
                json!({"name":"inspector","version":"1.0.0"}),
                registry,
            );
            let nested_dir = if kind == "file" {
                "node_modules/inspector/skill"
            } else {
                "node_modules/inspector-skill"
            };
            let nested_file = root.join(nested_dir).join("plugin.mjs");
            package_plugin(
                root,
                nested_dir,
                json!({"name":"inspector-skill","private":true}),
                registry,
            );
            let nested = if kind == "file" {
                file_url(&nested_file)
            } else {
                registry.add::<Noop>("inspector-skill/plugin.mjs");
                "inspector-skill/plugin.mjs".to_owned()
            };
            registry.add::<Noop>(&nested);
            registry.add::<Noop>(&parent);
            format!("- {{name: {}}}\n", serde_json::to_string(&nested).unwrap())
        })
        .await;
        assert_eq!(
            harness.fields(None).await[FIELD],
            json!({"version":1,"packages":[{"name":"inspector-skill"}]})
        );
        let parent: Entry = serde_json::from_value(json!({
            "id":"parent",
            "name":"./node_modules/inspector/plugin.mjs"
        }))
        .unwrap();
        harness.loader.create(parent, None).await.unwrap();
        harness.loader.settled().await;
        assert_eq!(
            harness.fields(None).await[FIELD],
            json!({"version":1,"packages":[
                {"name":"inspector","version":"1.0.0"},
                {"name":"inspector-skill"}
            ]})
        );
        harness.close().await;
    }
}

#[tokio::test]
async fn resolves_bare_subpaths_absolute_and_file_modules_and_skips_url_or_cordis_modules() {
    let harness = Harness::start(Some(true), |root, registry| {
        package_plugin(
            root,
            "node_modules/plain-package",
            json!({"name":"plain-package","version":"1.0.0"}),
            registry,
        );
        package_plugin(
            root,
            "node_modules/@scope/scoped-package",
            json!({"name":"@scope/scoped-package","version":"2.0.0"}),
            registry,
        );
        let absolute = package_plugin(
            root,
            "absolute-package",
            json!({"name":"absolute-package","version":"3.0.0"}),
            registry,
        );
        let absolute = root.join(absolute.trim_start_matches("./"));
        let absolute_text = absolute.to_string_lossy().into_owned();
        let absolute_url = file_url(&absolute);
        for name in [
            "plain-package/plugin.mjs",
            "@scope/scoped-package/plugin.mjs",
            &absolute_text,
            &absolute_url,
            "cordis:noop",
            "https://plugins.example/test.mjs",
        ] {
            registry.add::<Noop>(name);
        }
        [
            "plain-package/plugin.mjs".to_owned(),
            "@scope/scoped-package/plugin.mjs".to_owned(),
            absolute_text,
            absolute_url,
            "cordis:noop".to_owned(),
            "https://plugins.example/test.mjs".to_owned(),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            format!(
                "- {{id: row-{index}, name: {}}}\n",
                serde_json::to_string(&name).unwrap()
            )
        })
        .collect()
    })
    .await;
    assert_eq!(
        harness.fields(None).await[FIELD],
        json!({"version":1,"packages":[
            {"name":"@scope/scoped-package","version":"2.0.0"},
            {"name":"absolute-package","version":"3.0.0"},
            {"name":"plain-package","version":"1.0.0"}
        ]})
    );
    harness.close().await;
}

#[tokio::test]
async fn omits_loader_resolved_missing_bare_packages_with_or_without_profile_service() {
    let missing = Harness::start(Some(true), |_, registry| {
        registry.add::<Noop>("missing-package");
        "- name: missing-package\n".into()
    })
    .await;
    assert_eq!(
        missing.fields(None).await[FIELD],
        json!({"version":1,"packages":[]})
    );
    missing.close().await;

    let authoritative = Harness::start_with_packages(
        Some(true),
        |root, registry| {
            package_plugin(
                root,
                "node_modules/present-but-hidden",
                json!({"name":"present-but-hidden","version":"1"}),
                registry,
            );
            registry.add::<Noop>("present-but-hidden/plugin.mjs");
            "- name: present-but-hidden/plugin.mjs\n".into()
        },
        Some(PluginPackages::new(|_, _| None)),
        true,
    )
    .await;
    assert_eq!(
        authoritative.fields(None).await[FIELD],
        json!({"version":1,"packages":[]})
    );
    authoritative.close().await;
}

#[tokio::test]
async fn supports_direct_embedding_without_a_host_base_url() {
    let harness = Harness::start_with_packages(
        None,
        |_, registry| {
            registry.add::<Noop>("cordis:noop");
            "- name: cordis:noop\n".into()
        },
        None,
        false,
    )
    .await;
    assert_eq!(
        harness.fields(None).await[FIELD],
        json!({"version":1,"packages":[]})
    );
    harness.close().await;
}

#[tokio::test]
async fn uses_each_ordinary_loader_tree_base_for_conflicting_bare_versions() {
    let nested_base = Arc::new(std::sync::Mutex::new(None::<String>));
    let captured = nested_base.clone();
    let entry_base = Arc::new(move |entry: &hoh_loader::EntryInfo| {
        entry
            .id
            .contains("nested")
            .then(|| captured.lock().unwrap().clone())
            .flatten()
    });
    let harness = Harness::start_with_entry_bases(
        Some(true),
        move |root, registry| {
            package_plugin(
                root,
                "node_modules/versioned-plugin",
                json!({"name":"versioned-plugin","version":"1.0.0"}),
                registry,
            );
            let nested = root.join("nested");
            package_plugin(
                &nested,
                "node_modules/versioned-plugin",
                json!({"name":"versioned-plugin","version":"2.0.0"}),
                registry,
            );
            registry.add::<Noop>("versioned-plugin/plugin.mjs");
            let composition = nested.join("cordis.yml");
            std::fs::write(
                &composition,
                "- {id: nested-plugin, name: versioned-plugin/plugin.mjs}\n",
            )
            .unwrap();
            *nested_base.lock().unwrap() = Some(file_url(&composition));
            format!(
                "- {{id: host-plugin, name: versioned-plugin/plugin.mjs}}\n- {{id: nested, name: cordis:include, config: {{path: {}}}}}\n",
                serde_json::to_string(&file_url(&composition)).unwrap()
            )
        },
        entry_base,
    )
    .await;
    assert_eq!(
        harness.fields(None).await[FIELD],
        json!({"version":1,"packages":[
            {"name":"versioned-plugin","version":"1.0.0"},
            {"name":"versioned-plugin","version":"2.0.0"}
        ]})
    );
    harness.close().await;
}

#[tokio::test]
async fn uses_host_inventory_for_a_missing_session_and_withdraws_on_reload() {
    let harness = Harness::start(Some(true), |root, registry| {
        let host = package_plugin(
            root,
            "host",
            json!({"name":"host-only","version":"3"}),
            registry,
        );
        format!("- {{name: {host}}}\n")
    })
    .await;
    assert_eq!(
        harness.fields(Some("missing")).await[FIELD],
        json!({"version":1,"packages":[{"name":"host-only","version":"3"}]})
    );
    harness.inventory.dispose().await;
    assert!(!harness.fields(None).await.contains_key(FIELD));
    harness.close().await;
}

#[tokio::test]
async fn uses_host_inventory_for_a_joined_live_agent_without_a_mounted_preset() {
    let harness = Harness::start(Some(true), |root, registry| {
        let host = package_plugin(
            root,
            "host-live",
            json!({"name":"host-live","version":"3"}),
            registry,
        );
        format!("- {{name: {host}}}\n")
    })
    .await;
    let (_scope, _agent, _detach) = harness.enter_agent("bare-agent");
    assert_eq!(
        harness.fields(Some("bare-agent")).await[FIELD],
        json!({"version":1,"packages":[{"name":"host-live","version":"3"}]})
    );
    harness.close().await;
}

#[tokio::test]
async fn combines_host_and_live_agent_standing_preset_packages() {
    let harness = Harness::start(Some(true), |root, registry| {
        let host = package_plugin(
            root,
            "host-combined",
            json!({"name":"host-combined","version":"3"}),
            registry,
        );
        let preset_dir = root.join("node_modules/preset-only");
        std::fs::create_dir_all(&preset_dir).unwrap();
        std::fs::write(
            preset_dir.join("package.json"),
            r#"{"name":"preset-only","version":"4"}"#,
        )
        .unwrap();
        std::fs::write(preset_dir.join("plugin.mjs"), "").unwrap();
        format!("- {{name: {host}}}\n")
    })
    .await;

    let mut plugin_registry = PluginRegistry::default();
    plugin_registry.add::<Noop>("preset-only/plugin.mjs");
    let root = harness.root.path().to_path_buf();
    let presets = AgentPresetRegistry::new(
        &harness.ctx,
        PresetConfig {
            default: "fixture".into(),
            selected_default: Volatile(None),
        },
        PresetRuntime::new(
            Arc::new(plugin_registry),
            EvalEnv {
                home: root.clone(),
                cwd: root.clone(),
                exe: root.join("hoh"),
                platform: "linux",
                var: Arc::new(|_| None),
            },
        ),
    )
    .unwrap();
    // Publish after the inventory plugin activated: source lookup is live per request.
    let _provided = harness
        .ctx
        .provide::<AgentPresets>(Arc::new(presets.clone()))
        .await
        .unwrap();
    let _registration = presets
        .register(
            &harness.ctx,
            PresetDefinition {
                id: "fixture".into(),
                name: None,
                description: None,
                order: None,
                plugins: json!([{"id":"preset-only","name":"preset-only/plugin.mjs"}]),
            },
        )
        .await
        .unwrap();
    let (scope, _agent, _detach) = harness.enter_agent("preset-agent");
    presets.mount(&scope.ctx, Some("fixture")).await.unwrap();
    assert_eq!(
        harness.fields(Some("preset-agent")).await[FIELD],
        json!({"version":1,"packages":[
            {"name":"host-combined","version":"3"},
            {"name":"preset-only","version":"4"}
        ]})
    );
    harness.close().await;
}
