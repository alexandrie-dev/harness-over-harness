use hoh_office_kit::locate::create_engine_package_resolver;

#[test]
fn resolves_two_kit_installations_independently() {
    let root = tempfile::tempdir().unwrap();
    let mut fixtures = Vec::new();
    for installation in ["first", "second"] {
        let entry = root.path().join(installation).join("kit/lib/index.js");
        let manifest = root
            .path()
            .join(installation)
            .join("kit/node_modules/engine/package.json");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, "{}").unwrap();
        fixtures.push((create_engine_package_resolver(&entry), manifest));
    }
    assert_eq!(fixtures[0].0("engine").unwrap(), fixtures[0].1);
    assert_eq!(fixtures[1].0("engine").unwrap(), fixtures[1].1);
    assert_eq!(fixtures[0].0("engine").unwrap(), fixtures[0].1);
}
