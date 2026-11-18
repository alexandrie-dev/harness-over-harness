mod support;
use hoh_loader::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::live::Noop;
use support::*;
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dsh")
}
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for file in std::fs::read_dir(dir).unwrap() {
        let path = file.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|s| s == "yml") {
            out.push(path);
        }
    }
}
#[test]
fn every_shipped_bundle_and_preset_reads_unchanged_and_every_expression_parses() {
    let mut paths = Vec::new();
    files(&fixture().join("packages/bundle"), &mut paths);
    assert!(paths.len() >= 6);
    for path in paths {
        let text = std::fs::read_to_string(&path).unwrap();
        parse_patches(&text, &path).unwrap_or_else(|e| panic!("{e}"));
    }
}
#[test]
fn layers_of_the_real_headless_profile_compose_without_l10_warnings() {
    // packages/util/official-bundles/src/bundles.ts declares this exact order.
    // app-boot/profile.ts composes over an empty root (no checked-in profile base).
    let root = tempfile::tempdir().unwrap();
    let names = ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-headless"];
    let dir = write_profile(root.path(), &names);
    let source = Source(
        names
            .iter()
            .zip(["base", "headless"])
            .map(|(name, folder)| {
                let dir = fixture().join("packages/bundle").join(folder);
                Bundle {
                    name: (*name).into(),
                    files: vec![(
                        "cordis.patch.yml".into(),
                        std::fs::read_to_string(dir.join("cordis.patch.yml")).unwrap(),
                    )],
                    dir: Some(dir),
                    platforms: None,
                }
            })
            .collect(),
    );
    let profile = Profile::load(
        &dir,
        &source,
        &root.path().join("home"),
        &[],
        &env(root.path()),
    )
    .unwrap();
    assert!(profile.skipped.is_empty(), "{:?}", profile.skipped);
    let composition = compose(&[], &profile.layers, &mut |w| panic!("{w}"));
    assert!(!composition.entries.is_empty());
    assert!(
        composition
            .entries
            .iter()
            .any(|e| e.id == "session-telemetry-otel")
    );
    let dump = render_dump(&composition);
    assert_eq!(
        parse_entries(&dump, &root.path().join("dump.yml")).unwrap(),
        composition.entries
    );
}
#[test]
fn every_snapshot_entry_or_patch_document_reads_unchanged() {
    let mut paths = Vec::new();
    files(&fixture().join("snapshots"), &mut paths);
    let mut checked = 0;
    for path in paths {
        if matches!(
            path.file_name().and_then(|s| s.to_str()),
            Some("cordis.yml" | "cordis.snapshot.yml")
        ) {
            let text = std::fs::read_to_string(&path).unwrap();
            // Replay documents sometimes are overlay patches, despite the cordis basename.
            if let Err(entry_error) = parse_entries(&text, &path) {
                parse_patches(&text, &path).unwrap_or_else(|patch_error| {
                    panic!("entries: {entry_error}; patches: {patch_error}")
                });
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 244);
}

#[tokio::test(start_paused = true)]
async fn every_shipped_profile_composes_mounts_and_audits_cleanly() {
    macro_rules! fact {
        ($key:ident,$name:literal) => {
            struct $key;
            impl hoh_kernel::ServiceKey for $key {
                type Value = serde_json::Value;
                const NAME: &'static str = $name;
                fn view(v: &Self::Value) -> Option<Self::Value> {
                    Some(v.clone())
                }
            }
        };
    }
    fact!(Headless, "headlessStartup");
    fact!(Web, "webStartup");
    fact!(Acp, "acpAppStartup");
    fact!(Sdk, "sdkAppStartup");
    fact!(Activation, "sdkSessionActivation");
    fact!(Credentials, "credentials");
    fact!(ProfileContext, "profileContext");
    for (profile_name, folders) in [
        ("acp", vec!["base", "acp-app"]),
        ("web", vec!["base", "web-app"]),
        ("headless", vec!["base", "headless"]),
        ("sdk", vec!["base", "sdk-app"]),
        ("sdk-minimal", vec!["sdk-minimal"]),
    ] {
        let root = tempfile::tempdir().unwrap();
        let names: Vec<_> = folders
            .iter()
            .map(|folder| format!("@deepseek-ai/dsh-{folder}"))
            .collect();
        let name_refs: Vec<_> = names.iter().map(String::as_str).collect();
        let dir = write_profile(root.path(), &name_refs);
        let bundles = folders
            .iter()
            .zip(&names)
            .map(|(folder, name)| {
                let bundle_dir = fixture().join("packages/bundle").join(folder);
                let mut files = vec![(
                    "cordis.patch.yml".into(),
                    std::fs::read_to_string(bundle_dir.join("cordis.patch.yml")).unwrap(),
                )];
                if *folder == "web-app" {
                    for preset in ["standard", "ptc", "minimal", "cordis"] {
                        let file = format!("presets/{preset}.patch.yml");
                        files.push((
                            file.clone(),
                            std::fs::read_to_string(bundle_dir.join(file)).unwrap(),
                        ));
                    }
                }
                Bundle {
                    name: name.clone(),
                    dir: Some(bundle_dir),
                    platforms: None,
                    files,
                }
            })
            .collect();
        let profile = Profile::load(
            &dir,
            &Source(bundles),
            &root.path().join("home"),
            &[],
            &env(root.path()),
        )
        .unwrap();
        assert!(profile.skipped.is_empty());
        let composition = compose(&[], &profile.layers, &mut |warning| {
            panic!("{profile_name}: {warning}")
        });
        let mut registry = PluginRegistry::default();
        fn register(value: &serde_json::Value, registry: &mut PluginRegistry, root: &Path) {
            match value {
                serde_json::Value::Object(object) => {
                    if let Some(name) = object.get("name").and_then(serde_json::Value::as_str)
                        && !matches!(
                            name,
                            "cordis:group"
                                | "cordis:include"
                                | "@deepseek-ai/cordis-plugin-group"
                                | "@deepseek-ai/cordis-plugin-include"
                        )
                    {
                        registry
                            .add::<Noop>(name)
                            .resource_dir(name, root.join("resources"));
                        if name == "@deepseek-ai/dsh-agent-preset" {
                            registry.carrier(name);
                        }
                    }
                    for child in object.values() {
                        register(child, registry, root);
                    }
                }
                serde_json::Value::Array(array) => {
                    for child in array {
                        register(child, registry, root);
                    }
                }
                _ => {}
            }
        }
        register(&json_rows(&composition.entries), &mut registry, root.path());
        let base = root.path().join("cordis.yml");
        std::fs::write(&base, "[]\n").unwrap();
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        ctx.provide::<Headless>(Arc::new(serde_json::json!({})))
            .await
            .unwrap();
        ctx.provide::<Web>(Arc::new(serde_json::json!({})))
            .await
            .unwrap();
        ctx.provide::<Acp>(Arc::new(serde_json::json!({})))
            .await
            .unwrap();
        ctx.provide::<Sdk>(Arc::new(serde_json::json!({})))
            .await
            .unwrap();
        ctx.provide::<Activation>(Arc::new(serde_json::json!({})))
            .await
            .unwrap();
        ctx.provide::<Credentials>(Arc::new(serde_json::json!({})))
            .await
            .unwrap();
        ctx.provide::<ProfileContext>(Arc::new(serde_json::json!({"name":profile_name})))
            .await
            .unwrap();
        let loader = boot(
            &ctx,
            BootOptions {
                registry: Arc::new(registry),
                base,
                layers: profile.layers,
                env: env(root.path()),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{profile_name}: {error}"));
        loader.settled().await;
        let audit = loader.audit(&[]);
        assert!(audit.missing.is_empty());
        assert!(
            audit.unresolved.is_empty(),
            "{profile_name}: {:?}",
            audit.unresolved
        );
        assert!(
            audit.inactive.is_empty(),
            "{profile_name}: {:?}",
            audit
                .inactive
                .iter()
                .map(|fiber| (&fiber.entry_id, &fiber.error, &fiber.missing))
                .collect::<Vec<_>>()
        );
        assert!(
            !ctx.logger()
                .buffer()
                .iter()
                .any(|record| record.message.contains("patch: ")
                    || record.message.contains("patch insert:"))
        );
        kernel.shutdown().await;
    }
}
