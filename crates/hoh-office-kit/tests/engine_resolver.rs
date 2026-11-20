use hoh_office_kit::locate::{KitModuleResolution, resolve_kit_module};

#[test]
fn module_resolution_anchors_the_engine_callback_at_the_same_entry() {
    let root = tempfile::tempdir().unwrap();
    let entry = root.path().join("kit/lib/index.js");
    let manifest = root.path().join("kit/node_modules/engine/package.json");
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    std::fs::write(&manifest, "{}").unwrap();
    let KitModuleResolution::Available {
        entry: actual,
        resolve_engine_package,
    } = resolve_kit_module(|| Ok(entry.clone()))
    else {
        panic!("expected available")
    };
    assert_eq!(actual, entry);
    assert_eq!(resolve_engine_package("engine").unwrap(), manifest);
}
