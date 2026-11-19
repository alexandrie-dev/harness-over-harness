use hoh::profile_initialization::initialize_profile_from_default;
use hoh_loader::{
    init_profile, read_profile_manifest, resolve_profile_dir, write_profile_manifest,
};
use hoh_official_bundles::PROFILE_TEMPLATES;
use serde_json::json;
use std::{io::Write, path::Path};
fn initialize(name: &str, template: &str, home: &Path) {
    initialize_profile_from_default(name, template, home, &mut |_| panic!("warning")).unwrap();
}
fn manifest(name: &str, home: &Path) -> serde_json::Value {
    read_profile_manifest("test", &resolve_profile_dir(name, home).unwrap()).unwrap()
}
fn names(path: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}
#[test]
fn copies_the_template_metadata_into_an_independent_profile() {
    for (source, template) in PROFILE_TEMPLATES.iter() {
        let home = tempfile::tempdir().unwrap();
        initialize("custom", source, home.path());
        assert_eq!(
            manifest("custom", home.path()),
            json!({"name":"dsh-profile-custom","private":true,"dependencies":{},"dsh":{"profile":{"bundles":template.bundles}}})
        );
        let dir = resolve_profile_dir("custom", home.path()).unwrap();
        assert!(
            std::fs::read_to_string(dir.join("cordis.patch.yml"))
                .unwrap()
                .contains("[]")
        );
        assert!(
            std::fs::read_to_string(dir.join("pnpm-workspace.yaml"))
                .unwrap()
                .contains("nodeLinker: hoisted")
        );
    }
}
#[test]
fn does_not_copy_the_local_source_profile_dependencies_or_user_patch() {
    let home = tempfile::tempdir().unwrap();
    let dir = resolve_profile_dir("web", home.path()).unwrap();
    init_profile(&dir, &["local-bundle".into()]).unwrap();
    let mut source = manifest("web", home.path());
    source["dependencies"] = json!({"local-bundle":"1.0.0"});
    write_profile_manifest(&dir, &source).unwrap();
    std::fs::write(
        dir.join("cordis.patch.yml"),
        "- id: local-only\n  disabled: true\n",
    )
    .unwrap();
    initialize("rescue", "web", home.path());
    let target = manifest("rescue", home.path());
    assert_eq!(target["dependencies"], json!({}));
    assert_eq!(
        target["dsh"]["profile"]["bundles"],
        json!(PROFILE_TEMPLATES["web"].bundles)
    );
    assert!(
        !std::fs::read_to_string(
            resolve_profile_dir("rescue", home.path())
                .unwrap()
                .join("cordis.patch.yml")
        )
        .unwrap()
        .contains("local-only")
    );
}
#[test]
fn replaces_an_existing_target_with_a_fresh_template_copy() {
    let home = tempfile::tempdir().unwrap();
    let dir = resolve_profile_dir("rescue", home.path()).unwrap();
    init_profile(&dir, &["existing-bundle".into()]).unwrap();
    let mut before = manifest("rescue", home.path());
    before["dependencies"] = json!({"existing-bundle":"1.0.0"});
    write_profile_manifest(&dir, &before).unwrap();
    std::fs::write(
        dir.join("cordis.patch.yml"),
        "- id: existing\n  disabled: true\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("node_modules/existing-bundle")).unwrap();
    initialize("rescue", "web", home.path());
    assert_eq!(
        manifest("rescue", home.path()),
        json!({"name":"dsh-profile-rescue","private":true,"dependencies":{},"dsh":{"profile":{"bundles":PROFILE_TEMPLATES["web"].bundles}}})
    );
    assert!(!dir.join("node_modules").exists());
    assert!(
        !std::fs::read_to_string(dir.join("cordis.patch.yml"))
            .unwrap()
            .contains("existing")
    );
    assert_eq!(names(dir.parent().unwrap()), ["rescue"]);
}
#[test]
fn replaces_a_residual_target_directory() {
    let home = tempfile::tempdir().unwrap();
    let dir = resolve_profile_dir("rescue", home.path()).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("residual"), "").unwrap();
    initialize("rescue", "web", home.path());
    assert!(!dir.join("residual").exists());
    assert_eq!(
        manifest("rescue", home.path())["dsh"]["profile"]["bundles"],
        json!(PROFILE_TEMPLATES["web"].bundles)
    );
}
#[test]
fn rejects_a_case_variant_of_a_shipped_profile_without_changing_it() {
    let home = tempfile::tempdir().unwrap();
    let dir = resolve_profile_dir("web", home.path()).unwrap();
    init_profile(&dir, &["existing-bundle".into()]).unwrap();
    let before = std::fs::read(dir.join("package.json")).unwrap();
    let error =
        initialize_profile_from_default("Web", "headless", home.path(), &mut |_| {}).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("profile \"Web\" names shipped profile \"web\"")
    );
    assert_eq!(std::fs::read(dir.join("package.json")).unwrap(), before);
}
#[test]
fn rejects_shipped_target_names_without_creating_them() {
    for name in PROFILE_TEMPLATES.keys() {
        let home = tempfile::tempdir().unwrap();
        let error =
            initialize_profile_from_default(name, "web", home.path(), &mut |_| {}).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("names shipped profile \"{name}\""))
        );
        assert!(!resolve_profile_dir(name, home.path()).unwrap().exists());
    }
}
#[test]
fn rejects_unknown_templates_without_creating_the_target() {
    for name in ["unknown", "toString"] {
        let home = tempfile::tempdir().unwrap();
        let error =
            initialize_profile_from_default("rescue", name, home.path(), &mut |_| {}).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "dsh: unknown default profile \"{name}\"; expected one of \"acp\", \"headless\", \"sdk\", \"sdk-minimal\", \"web\""
            )
        );
        assert!(!resolve_profile_dir("rescue", home.path()).unwrap().exists());
    }
}
#[test]
fn profile_initialization_child() {
    // Fixture facts are an additional libtest filter argument; no ambient environment is read.
    let Some(encoded) =
        std::env::args().find_map(|arg| arg.strip_prefix("hoh-init-fixture=").map(str::to_owned))
    else {
        return;
    };
    let facts: Vec<String> = serde_json::from_str(&encoded).unwrap();
    let home = Path::new(&facts[0]);
    let ready = Path::new(&facts[1]);
    let gate = Path::new(&facts[2]);
    std::fs::write(ready, "").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !gate.exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if let Err(error) = initialize_profile_from_default("rescue", "web", home, &mut |_| {}) {
        let _ = writeln!(std::io::stderr().lock(), "{error}");
        std::process::exit(1);
    }
}
#[test]
fn leaves_one_complete_template_copy_after_two_synchronized_processes_replace_the_target() {
    let home = tempfile::tempdir().unwrap();
    let gate = home.path().join("start");
    let ready = [home.path().join("ready-1"), home.path().join("ready-2")];
    let exe = std::env::current_exe().unwrap();
    let mut children = vec![];
    for marker in &ready {
        let facts = serde_json::to_string(&json!([home.path(), marker, gate])).unwrap();
        children.push(
            std::process::Command::new(&exe)
                .args([
                    "--exact",
                    "profile_initialization_child",
                    &format!("hoh-init-fixture={facts}"),
                ])
                .env_clear()
                .env("HOME", home.path())
                .current_dir(home.path())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !ready.iter().all(|p| p.exists()) {
        if std::time::Instant::now() >= deadline {
            for child in &mut children {
                let _ = child.kill();
            }
            panic!("profile initialization marker did not appear");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::fs::write(gate, "").unwrap();
    let mut success = false;
    for mut child in children {
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                panic!("profile initializer timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let result = child.wait_with_output().unwrap();
        if result.status.success() {
            success = true;
        } else {
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("was recreated concurrently"),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
    assert!(success);
    assert_eq!(
        manifest("rescue", home.path())["dsh"]["profile"]["bundles"],
        json!(PROFILE_TEMPLATES["web"].bundles)
    );
    assert_eq!(names(&home.path().join("profiles")), ["rescue"]);
}
