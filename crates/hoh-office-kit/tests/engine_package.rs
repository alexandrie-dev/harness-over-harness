use hoh_office_kit::locate::unpacked_path;

#[test]
fn maps_only_the_first_complete_asar_segment() {
    for (path, expected) in [
        (
            "/App/app.asar/dsh/package.json",
            "/App/app.asar.unpacked/dsh/package.json",
        ),
        (
            r"C:\DSH\app.asar\dsh\package.json",
            r"C:\DSH\app.asar.unpacked\dsh\package.json",
        ),
        ("/App/app.asar.unpacked/x", "/App/app.asar.unpacked/x"),
        ("/workspace/x", "/workspace/x"),
        ("/notes.asar.txt/x", "/notes.asar.txt/x"),
        (
            "/app.asar/nested.asar/x",
            "/app.asar.unpacked/nested.asar/x",
        ),
    ] {
        assert_eq!(unpacked_path(path), expected);
    }
}

#[test]
fn preserves_wasm_and_maps_native_engine_manifests() {
    let directory = tempfile::tempdir().unwrap();
    let kit = directory.path().join("libreoffice-kit");
    let entry = kit.join("lib/index.js");
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    for name in ["@deepseek-ai/libreoffice-kit-wasm", "native"] {
        let manifest = kit.join("node_modules").join(name).join("package.json");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, "{}").unwrap();
    }
    let resolve = hoh_office_kit::locate::create_engine_package_resolver(&entry);
    assert!(
        resolve("@deepseek-ai/libreoffice-kit-wasm")
            .unwrap()
            .ends_with("package.json")
    );
    assert!(resolve("native").unwrap().ends_with("package.json"));
}

#[test]
fn reports_an_incomplete_or_absent_package() {
    let directory = tempfile::tempdir().unwrap();
    let entry = directory.path().join("kit/lib/index.js");
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    let resolve = hoh_office_kit::locate::create_engine_package_resolver(&entry);
    assert!(resolve("absent").is_err());
    let directory_path = directory
        .path()
        .join("kit/node_modules/incomplete/package.json");
    std::fs::create_dir_all(&directory_path).unwrap();
    assert!(
        resolve("incomplete")
            .unwrap_err()
            .to_string()
            .contains("incomplete")
    );
}
