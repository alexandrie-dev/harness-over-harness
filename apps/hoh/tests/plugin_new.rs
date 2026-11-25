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

fn cli(cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_hoh"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn agent_can_discover_generate_and_check_a_tool_without_booting_a_profile() {
    let root = repository();
    let cwd = root.path().join("crates/hoh-kernel");
    let templates = cli(&cwd, &["plugin", "templates", "--json"]);
    assert!(templates.status.success());
    assert_eq!(json(&templates)[1]["id"], "tool");

    let guide = cli(&cwd, &["plugin", "guide", "session-panel", "--json"]);
    assert!(guide.status.success());
    assert!(
        json(&guide)["guide"]
            .as_str()
            .unwrap()
            .contains("plugins/turn-note")
    );

    let create = cli(
        &cwd,
        &["plugin", "new", "demo-tool", "--template", "tool", "--json"],
    );
    assert!(create.status.success(), "{:?}", create);
    let scaffold = json(&create);
    assert_eq!(scaffold["template"], "tool");
    assert_eq!(
        scaffold["testCommand"],
        "cargo test --offline -p hoh-plugin-demo-tool"
    );
    let path = root.path().join("plugins/demo-tool");
    let manifest = path.join("hoh-plugin.json");
    let scaffold_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(
        scaffold_manifest["permissions"]["tools"][0],
        "demo_tool_echo"
    );

    let check = cli(&cwd, &["plugin", "check", "demo-tool", "--json"]);
    assert!(check.status.success(), "{:?}", check);
    assert_eq!(json(&check)["issues"], serde_json::json!([]));
    assert_eq!(json(&check)["linkedManifest"], "missing");

    fs::write(&manifest, "{").unwrap();
    let check = cli(&cwd, &["plugin", "check", "demo-tool", "--json"]);
    assert_eq!(check.status.code(), Some(1));
    assert_eq!(json(&check)["issues"][0]["file"], "hoh-plugin.json");
}

#[test]
fn check_compares_manifest_bytes_with_the_built_in_index() {
    let root = repository();
    let path = root.path().join("plugins/turn-note");
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"hoh-plugin-turn-note\"\n",
    )
    .unwrap();
    let manifest = path.join("hoh-plugin.json");
    let linked = include_str!("../../../plugins/turn-note/hoh-plugin.json");
    fs::write(&manifest, linked).unwrap();

    let args = ["plugin", "check", "turn-note", "--json"];
    let check = cli(root.path(), &args);
    assert!(check.status.success(), "{:?}", check);
    assert_eq!(json(&check)["linkedManifest"], "matches");

    fs::write(&manifest, format!("{linked}\n")).unwrap();
    let check = cli(root.path(), &args);
    assert!(check.status.success(), "{:?}", check);
    assert_eq!(json(&check)["linkedManifest"], "differs");
}

#[cfg(unix)]
#[test]
fn refuses_a_symlinked_plugins_directory() {
    let root = repository();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("plugins")).unwrap();
    assert!(create(root.path(), "demo").is_err());
    assert!(!outside.path().join("demo").exists());
}
