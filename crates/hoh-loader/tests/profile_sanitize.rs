use hoh_loader::profile_sanitize::sanitize_profile;
use serde_json::{Value, json};
use std::path::Path;
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"),json!({"dependencies":{"broken-plugin":"1.2.3"},"custom":"retained","dsh":{"profile":{"bundles":["broken-plugin"],"other":true}}}).to_string()).unwrap();
    std::fs::write(dir.path().join("cordis.patch.yml"), ": broken YAML").unwrap();
    dir
}
fn recover(dir: &Path) -> Option<std::path::PathBuf> {
    sanitize_profile(
        "test",
        dir,
        &["base".into(), "web".into()],
        1_789_555_200_000,
    )
    .unwrap()
}
fn manifest(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("package.json")).unwrap()).unwrap()
}
#[test]
fn recovers_a_profile_without_loading_its_broken_plugins_or_patch() {
    let dir = fixture();
    let package = dir.path().join("node_modules/broken-plugin");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), "{broken").unwrap();
    let backup = recover(dir.path()).unwrap();
    assert_eq!(std::fs::read_to_string(backup).unwrap(), ": broken YAML");
    assert!(!dir.path().join("cordis.patch.yml").exists());
    let value = manifest(dir.path());
    assert_eq!(value["custom"], "retained");
    assert_eq!(value["dependencies"]["broken-plugin"], "1.2.3");
    assert_eq!(value["dsh"]["profile"]["other"], true);
    assert_eq!(value["dsh"]["profile"]["bundles"], json!(["base", "web"]));
    assert_eq!(
        std::fs::read_to_string(package.join("package.json")).unwrap(),
        "{broken"
    );
}
#[test]
fn preserves_previous_backups_across_retries_and_later_recovery_actions() {
    let dir = fixture();
    let patch = dir.path().join("cordis.patch.yml");
    let first = recover(dir.path()).unwrap();
    assert!(recover(dir.path()).is_none());
    std::fs::write(&patch, "second patch").unwrap();
    let second = recover(dir.path()).unwrap();
    std::fs::write(&patch, "third patch").unwrap();
    let third = recover(dir.path()).unwrap();
    assert!(first.ends_with("cordis.patch.yml.bak-1789555200000"));
    assert!(second.ends_with("cordis.patch.yml.bak-1789555200000-1"));
    assert!(third.ends_with("cordis.patch.yml.bak-1789555200000-2"));
    assert_eq!(std::fs::read_to_string(first).unwrap(), ": broken YAML");
    assert_eq!(std::fs::read_to_string(second).unwrap(), "second patch");
    assert_eq!(std::fs::read_to_string(third).unwrap(), "third patch");
}
#[test]
fn does_not_create_an_absent_profile_and_backs_up_a_patch_even_without_a_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("absent");
    assert!(recover(&absent).is_none());
    assert!(!absent.exists());
    std::fs::create_dir(&absent).unwrap();
    std::fs::write(absent.join("cordis.patch.yml"), "orphan patch").unwrap();
    let backup = recover(&absent).unwrap();
    assert_eq!(std::fs::read_to_string(backup).unwrap(), "orphan patch");
    assert!(!absent.join("package.json").exists());
}
#[test]
fn leaves_the_patch_untouched_when_profile_json_is_invalid() {
    let dir = fixture();
    std::fs::write(dir.path().join("package.json"), "{broken").unwrap();
    assert!(sanitize_profile("test", dir.path(), &[], 0).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("cordis.patch.yml")).unwrap(),
        ": broken YAML"
    );
    assert!(
        !std::fs::read_dir(dir.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".bak-"))
    );
}
