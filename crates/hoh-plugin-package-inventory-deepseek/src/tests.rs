use super::*;
use serde_json::json;
use std::io::Write;
use tempfile::TempDir;

fn file_url(path: &Path) -> String {
    Url::from_file_path(path).unwrap().into()
}

fn package(root: &Path, relative: &str, manifest: serde_json::Value) -> PathBuf {
    let directory = root.join(relative);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(directory.join("plugin.mjs"), "export default () => {}\n").unwrap();
    directory.join("plugin.mjs")
}

fn entry(module_name: impl Into<String>, base: &Path) -> ActiveEntry {
    ActiveEntry {
        module_name: module_name.into(),
        base_url: Some(file_url(&base.join("cordis.yml"))),
        bare_base_url: None,
    }
}

#[test]
fn config_defaults_enabled_and_accepts_explicit_false() {
    assert!(serde_json::from_value::<Config>(json!({})).unwrap().enabled);
    assert!(
        !serde_json::from_value::<Config>(json!({"enabled": false}))
            .unwrap()
            .enabled
    );
    assert!(
        serde_json::from_value::<Config>(json!({"enabled": true, "futureOption": 1}))
            .unwrap()
            .enabled
    );
}

#[test]
fn manifest_keeps_nonblank_strings_and_omits_invalid_metadata() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("package.json");
    fs::write(&path, r#"{"name":" package ","version":" 1.0 "}"#).unwrap();
    assert_eq!(
        identity_from_manifest(&path).unwrap(),
        Some(DeepSeekPluginPackageIdentity {
            name: " package ".into(),
            version: Some(" 1.0 ".into()),
        })
    );
    for value in [json!({}), json!({"name": "  "}), json!({"name": 1})] {
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(identity_from_manifest(&path).unwrap(), None);
    }
    fs::write(&path, r#"{"name":"optional"}"#).unwrap();
    assert_eq!(
        identity_from_manifest(&path).unwrap(),
        Some(DeepSeekPluginPackageIdentity {
            name: "optional".into(),
            version: None,
        })
    );
    for version in [
        json!(null),
        json!(""),
        json!("  "),
        json!(1),
        json!(false),
        json!({}),
        json!([]),
    ] {
        fs::write(
            &path,
            serde_json::to_vec(&json!({"name":"optional", "version": version})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            identity_from_manifest(&path).unwrap(),
            Some(DeepSeekPluginPackageIdentity {
                name: "optional".into(),
                version: None,
            })
        );
    }
}

#[test]
fn manifest_decodes_invalid_utf8_like_node_and_documents_lone_surrogates() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("package.json");
    fs::write(&path, b"{\"name\":\"a\xff\",\"version\":\"1\"}").unwrap();
    assert_eq!(
        identity_from_manifest(&path).unwrap(),
        Some(DeepSeekPluginPackageIdentity {
            name: "a\u{fffd}".into(),
            version: Some("1".into()),
        })
    );

    // Rust String cannot represent the unpaired UTF-16 surrogate that JSON.parse accepts.
    fs::write(&path, br#"{"name":"\ud800x"}"#).unwrap();
    assert!(matches!(
        identity_from_manifest(&path),
        Err(InventoryError::Parse { .. })
    ));
}

#[test]
fn null_manifest_is_unreadable_like_javascript_property_access() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("package.json");
    fs::write(&path, "null").unwrap();
    assert!(matches!(
        identity_from_manifest(&path),
        Err(InventoryError::NullManifest { .. })
    ));
}

#[test]
fn manifest_uses_javascript_trim_for_bom() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("package.json");
    fs::write(&path, "{\"name\":\"\\ufeff\",\"version\":\"1\"}").unwrap();
    assert_eq!(identity_from_manifest(&path).unwrap(), None);
    fs::write(
        &path,
        "{\"name\":\"\\ufeffnamed\\ufeff\",\"version\":\"\\ufeff\"}",
    )
    .unwrap();
    assert_eq!(
        identity_from_manifest(&path).unwrap(),
        Some(DeepSeekPluginPackageIdentity {
            name: "\u{feff}named\u{feff}".into(),
            version: None,
        })
    );
}

#[test]
fn parses_bare_package_and_subpath_names() {
    assert_eq!(bare_package_name("plain/subpath").as_deref(), Some("plain"));
    assert_eq!(
        bare_package_name("@scope/name/subpath").as_deref(),
        Some("@scope/name")
    );
    for value in [
        "./local.mjs",
        "../up.mjs",
        "file:///tmp/a.mjs",
        "cordis:noop",
        "/tmp/a.mjs",
    ] {
        assert_eq!(bare_package_name(value), None, "{value}");
    }
}

#[test]
fn relative_and_absolute_files_use_the_nearest_manifest() {
    let root = TempDir::new().unwrap();
    let parent = package(
        root.path(),
        "inspector",
        json!({"name":"inspector","version":"1"}),
    );
    let nested = package(
        root.path(),
        "inspector/skill",
        json!({"name":"skill","private":true}),
    );
    let resolver = PackageIdentityResolver::new(file_url(&root.path().join("cordis.yml")), None);
    assert_eq!(
        resolver
            .resolve(&entry("./inspector/plugin.mjs", root.path()))
            .unwrap()
            .unwrap()
            .name,
        "inspector"
    );
    assert_eq!(
        resolver
            .resolve(&entry(file_url(&nested), root.path()))
            .unwrap(),
        Some(DeepSeekPluginPackageIdentity {
            name: "skill".into(),
            version: None
        })
    );
    assert_eq!(
        resolver
            .resolve(&entry(parent.to_string_lossy(), root.path()))
            .unwrap()
            .unwrap()
            .version
            .as_deref(),
        Some("1")
    );
}

#[test]
fn bare_packages_resolve_per_tree_base_and_keep_parallel_versions() {
    let root = TempDir::new().unwrap();
    package(
        root.path(),
        "node_modules/versioned",
        json!({"name":"versioned","version":"1"}),
    );
    let nested = root.path().join("nested");
    package(
        &nested,
        "node_modules/versioned",
        json!({"name":"versioned","version":"2"}),
    );
    let resolver = PackageIdentityResolver::new(file_url(&root.path().join("cordis.yml")), None);
    let identities = [
        entry("versioned/plugin.mjs", root.path()),
        entry("versioned/plugin.mjs", &nested),
    ]
    .into_iter()
    .map(|entry| resolver.resolve(&entry).unwrap().unwrap());
    assert_eq!(
        sort_and_deduplicate(identities),
        vec![
            DeepSeekPluginPackageIdentity {
                name: "versioned".into(),
                version: Some("1".into())
            },
            DeepSeekPluginPackageIdentity {
                name: "versioned".into(),
                version: Some("2".into())
            },
        ]
    );
}

#[test]
fn directory_anchor_searches_its_own_node_modules() {
    let root = TempDir::new().unwrap();
    package(
        root.path(),
        "node_modules/pkg",
        json!({"name":"pkg","version":"1"}),
    );
    let host_base = format!("{}/", file_url(root.path()).trim_end_matches('/'));
    let resolver = PackageIdentityResolver::new(host_base, None);
    let active = ActiveEntry {
        module_name: "pkg/plugin.mjs".into(),
        base_url: None,
        bare_base_url: None,
    };

    assert_eq!(
        resolver.resolve(&active).unwrap(),
        Some(DeepSeekPluginPackageIdentity {
            name: "pkg".into(),
            version: Some("1".into()),
        })
    );
}

#[test]
fn absolute_module_paths_are_lexically_normalized_before_manifest_walk() {
    let root = TempDir::new().unwrap();
    package(root.path(), "a", json!({"name":"wrong","version":"1"}));
    let module_dir = root.path().join("b");
    fs::create_dir_all(&module_dir).unwrap();
    fs::write(module_dir.join("plugin.mjs"), "").unwrap();
    let module = root.path().join("a/../b/plugin.mjs");
    let resolver = PackageIdentityResolver::new(file_url(&root.path().join("cordis.yml")), None);

    assert_eq!(
        resolver
            .resolve(&entry(module.to_string_lossy(), root.path()))
            .unwrap(),
        None
    );
}

#[test]
fn package_service_is_authoritative_for_every_anchor() {
    let root = TempDir::new().unwrap();
    package(
        root.path(),
        "node_modules/present",
        json!({"name":"present","version":"1"}),
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let service = PluginPackages::new(move |package, anchor| {
        lock(&seen).push((package.to_owned(), anchor.to_owned()));
        None
    });
    let resolver =
        PackageIdentityResolver::new(file_url(&root.path().join("cordis.yml")), Some(service));
    assert!(
        resolver
            .resolve(&entry("present/plugin.mjs", root.path()))
            .is_err()
    );
    assert!(!lock(&calls).is_empty());
}

#[test]
fn package_service_can_return_a_manifest() {
    let root = TempDir::new().unwrap();
    let plugin = package(
        root.path(),
        "profile/pkg",
        json!({"name":"pkg","version":"9"}),
    );
    let manifest = plugin.parent().unwrap().join("package.json");
    let service =
        PluginPackages::new(move |package, _| (package == "pkg").then(|| manifest.clone()));
    let resolver =
        PackageIdentityResolver::new(file_url(&root.path().join("cordis.yml")), Some(service));
    assert_eq!(
        resolver
            .resolve(&entry("pkg/subpath", root.path()))
            .unwrap()
            .unwrap()
            .version
            .as_deref(),
        Some("9")
    );
}

#[test]
fn module_anchor_is_tried_after_tree_and_host_anchors() {
    let root = TempDir::new().unwrap();
    let module_root = root.path().join("installed/inventory");
    package(
        &module_root,
        "node_modules/owned",
        json!({"name":"owned","version":"7"}),
    );
    let missing_host = root.path().join("profile/cordis.yml");
    let resolver = PackageIdentityResolver::with_module_base(
        file_url(&missing_host),
        file_url(&module_root.join("src/lib.rs")),
        None,
    );
    assert_eq!(
        resolver
            .resolve(&entry("owned/plugin.mjs", missing_host.parent().unwrap()))
            .unwrap()
            .unwrap()
            .version
            .as_deref(),
        Some("7")
    );
}

#[test]
fn cordis_http_and_loose_modules_are_omitted() {
    let root = TempDir::new().unwrap();
    fs::create_dir(root.path().join("loose")).unwrap();
    fs::write(root.path().join("loose/plugin.mjs"), "").unwrap();
    let resolver = PackageIdentityResolver::new(file_url(&root.path().join("cordis.yml")), None);
    for module in [
        "cordis:noop",
        "https://plugins.example/test.mjs",
        "./loose/plugin.mjs",
    ] {
        assert_eq!(
            resolver.resolve(&entry(module, root.path())).unwrap(),
            None,
            "{module}"
        );
    }
}

#[test]
fn invalid_manifest_errors_are_not_cached_but_identities_are() {
    let root = TempDir::new().unwrap();
    let plugin = package(root.path(), "bad", json!({"name":"bad","version":"1"}));
    let manifest = plugin.parent().unwrap().join("package.json");
    fs::write(&manifest, "{").unwrap();
    let resolver = PackageIdentityResolver::new(file_url(&root.path().join("cordis.yml")), None);
    let active = entry("./bad/plugin.mjs", root.path());
    assert!(resolver.resolve(&active).is_err());
    fs::write(&manifest, r#"{"name":"fixed","version":"2"}"#).unwrap();
    assert_eq!(resolver.resolve(&active).unwrap().unwrap().name, "fixed");
    fs::write(&manifest, r#"{"name":"changed","version":"3"}"#).unwrap();
    assert_eq!(resolver.resolve(&active).unwrap().unwrap().name, "fixed");
}

#[test]
fn deleted_manifest_becomes_a_cached_loose_module() {
    let root = TempDir::new().unwrap();
    let plugin = package(root.path(), "gone", json!({"name":"gone","version":"1"}));
    fs::remove_file(plugin.parent().unwrap().join("package.json")).unwrap();
    let resolver = PackageIdentityResolver::new(file_url(&root.path().join("cordis.yml")), None);
    let active = entry("./gone/plugin.mjs", root.path());
    assert_eq!(resolver.resolve(&active).unwrap(), None);
    fs::write(
        plugin.parent().unwrap().join("package.json"),
        r#"{"name":"late"}"#,
    )
    .unwrap();
    assert_eq!(resolver.resolve(&active).unwrap(), None);
}

#[test]
fn name_only_and_versioned_identities_deduplicate_separately_and_sort() {
    let values = vec![
        DeepSeekPluginPackageIdentity {
            name: "same".into(),
            version: Some("undefined".into()),
        },
        DeepSeekPluginPackageIdentity {
            name: "same".into(),
            version: None,
        },
        DeepSeekPluginPackageIdentity {
            name: "same".into(),
            version: None,
        },
    ];
    assert_eq!(
        sort_and_deduplicate(values),
        vec![
            DeepSeekPluginPackageIdentity {
                name: "same".into(),
                version: None
            },
            DeepSeekPluginPackageIdentity {
                name: "same".into(),
                version: Some("undefined".into())
            },
        ]
    );
}

#[test]
fn sorting_uses_javascript_utf16_code_units() {
    let supplementary = "\u{10000}";
    let bmp = "\u{e000}";
    assert_eq!(compare_wire_text(supplementary, bmp), Ordering::Less);
    let sorted = sort_and_deduplicate([
        DeepSeekPluginPackageIdentity {
            name: bmp.into(),
            version: None,
        },
        DeepSeekPluginPackageIdentity {
            name: supplementary.into(),
            version: None,
        },
    ]);
    assert_eq!(sorted[0].name, supplementary);
}

#[test]
fn bare_base_can_override_the_composition_tree_for_preset_roots() {
    let root = TempDir::new().unwrap();
    let nested = root.path().join("nested");
    package(
        root.path(),
        "node_modules/preset",
        json!({"name":"preset","version":"4"}),
    );
    package(
        &nested,
        "node_modules/preset",
        json!({"name":"preset","version":"5"}),
    );
    let host = file_url(&root.path().join("cordis.yml"));
    let resolver = PackageIdentityResolver::new(host.clone(), None);
    let host_entry = ActiveEntry {
        module_name: "preset/plugin.mjs".into(),
        base_url: Some(file_url(&nested.join("cordis.yml"))),
        bare_base_url: Some(host),
    };
    let nested_entry = entry("preset/plugin.mjs", &nested);
    assert_eq!(
        resolver
            .resolve(&host_entry)
            .unwrap()
            .unwrap()
            .version
            .as_deref(),
        Some("4")
    );
    assert_eq!(
        resolver
            .resolve(&nested_entry)
            .unwrap()
            .unwrap()
            .version
            .as_deref(),
        Some("5")
    );
}

#[test]
fn unreadable_identity_format_preserves_module_and_reason() {
    let root = TempDir::new().unwrap();
    let plugin = package(root.path(), "bad", json!({"name":"bad"}));
    let manifest = plugin.parent().unwrap().join("package.json");
    let mut file = fs::File::create(&manifest).unwrap();
    file.write_all(b"{").unwrap();
    let resolver = PackageIdentityResolver::new(file_url(&root.path().join("cordis.yml")), None);
    let error = resolver
        .resolve(&entry("./bad/plugin.mjs", root.path()))
        .unwrap_err();
    assert!(error.to_string().contains("package.json"));
}
