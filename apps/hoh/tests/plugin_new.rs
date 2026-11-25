use hoh::plugin_new::create;
use std::fs;

fn repository() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("crates/hoh-kernel")).unwrap();
    fs::write(root.path().join("crates/hoh-kernel/Cargo.toml"), "").unwrap();
    root
}

#[test]
fn creates_scaffold_and_never_overwrites_it() {
    let root = repository();
    let path = create(root.path(), "turn-note").unwrap();
    for name in [
        "Cargo.toml",
        "src/lib.rs",
        "hoh-plugin.json",
        "tests/smoke.rs",
    ] {
        assert!(path.join(name).is_file(), "missing {name}");
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(path.join("hoh-plugin.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "turn-note");
    fs::write(path.join("src/lib.rs"), "author's work").unwrap();
    assert!(create(root.path(), "turn-note").is_err());
    assert_eq!(
        fs::read_to_string(path.join("src/lib.rs")).unwrap(),
        "author's work"
    );
}

#[test]
fn rejects_unsafe_ids_and_non_repository_directories() {
    let root = repository();
    for id in ["", "../escape", "A", "-test", "test-", "test/foo", "a_b"] {
        assert!(create(root.path(), id).is_err(), "accepted {id:?}");
    }
    let other = tempfile::tempdir().unwrap();
    assert!(create(other.path(), "valid").is_err());
    assert!(!other.path().join("plugins").exists());
}
