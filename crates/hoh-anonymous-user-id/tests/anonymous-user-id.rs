use hoh_anonymous_user_id::*;
use std::{collections::HashMap, fs, path::Path};
fn env(home: &Path) -> HashMap<String, String> {
    [("DSH_HOME".into(), home.to_string_lossy().into_owned())].into()
}
fn get(store: &AnonymousUserIdStore, home: &Path) -> AnonymousUserId {
    store
        .get_or_create_anonymous_user_id(AnonymousUserIdOptions {
            env: Some(&env(home)),
            random_uuid: None,
        })
        .unwrap()
}
fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
#[test]
fn creates_persists_and_returns_bare_uuid_line_on_first_use() {
    let d = tempfile::tempdir().unwrap();
    let id = get(&AnonymousUserIdStore::default(), d.path());
    assert!(is_uuid(id.as_ref()));
    assert_eq!(
        fs::read_to_string(d.path().join(ANONYMOUS_USER_ID_FILE_NAME)).unwrap(),
        format!("{id}\n")
    );
}
#[test]
fn creates_missing_home_directory() {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("nested/home");
    let id = get(&AnonymousUserIdStore::default(), &home);
    assert_eq!(
        fs::read_to_string(home.join(ANONYMOUS_USER_ID_FILE_NAME)).unwrap(),
        format!("{id}\n")
    );
}
#[test]
fn returns_persisted_id_tolerating_surrounding_whitespace() {
    let d = tempfile::tempdir().unwrap();
    let existing = "01234567-89ab-4cde-8f01-23456789abcd";
    fs::write(
        d.path().join(ANONYMOUS_USER_ID_FILE_NAME),
        format!("  {existing}\n\n"),
    )
    .unwrap();
    assert_eq!(
        get(&AnonymousUserIdStore::default(), d.path()).as_ref(),
        existing
    );
}
#[test]
fn overwrites_corrupt_file_with_fresh_id() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join(ANONYMOUS_USER_ID_FILE_NAME);
    fs::write(&file, "not-a-uuid\n").unwrap();
    let id = get(&AnonymousUserIdStore::default(), d.path());
    assert!(is_uuid(id.as_ref()));
    assert_eq!(fs::read_to_string(file).unwrap(), format!("{id}\n"));
}
#[test]
fn adopts_concurrent_winner_after_initial_read() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join(ANONYMOUS_USER_ID_FILE_NAME);
    let winner = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let generate = || {
        fs::write(&file, format!("{winner}\n")).unwrap();
        "ffffffff-0000-4000-8000-000000000000".into()
    };
    let id = get_or_create_anonymous_user_id(
        &AnonymousUserIdStore::default(),
        AnonymousUserIdOptions {
            env: Some(&env(d.path())),
            random_uuid: Some(&generate),
        },
    )
    .unwrap();
    assert_eq!(id.as_ref(), winner);
}
#[test]
fn returns_usable_id_when_home_cannot_contain_files() {
    let d = tempfile::tempdir().unwrap();
    let blocked = d.path().join("blocked");
    fs::write(&blocked, "occupied\n").unwrap();
    let id = get(&AnonymousUserIdStore::default(), &blocked);
    assert!(is_uuid(id.as_ref()));
    assert!(!blocked.join(ANONYMOUS_USER_ID_FILE_NAME).exists());
}
#[test]
fn memoizes_per_home_one_read_deletion_proof() {
    let d = tempfile::tempdir().unwrap();
    let store = AnonymousUserIdStore::default();
    let first = get(&store, d.path());
    fs::remove_file(d.path().join(ANONYMOUS_USER_ID_FILE_NAME)).unwrap();
    assert_eq!(get(&store, d.path()), first);
}
#[test]
fn keeps_distinct_homes_on_distinct_ids() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let store = AnonymousUserIdStore::default();
    assert_ne!(get(&store, a.path()), get(&store, b.path()));
}
#[test]
fn reads_process_environment_by_default() {
    // Run a dedicated test process with a temporary HOME override: no unsafe set_var or shared environment race.
    let d = tempfile::tempdir().unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process_environment_child", "--ignored"])
        .env("DSH_HOME", d.path())
        .env("HOH_ANONYMOUS_USER_ID_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(d.path().join(ANONYMOUS_USER_ID_FILE_NAME).exists());
}
#[test]
#[ignore = "invoked only by reads_process_environment_by_default with an isolated environment"]
fn process_environment_child() {
    if std::env::var_os("HOH_ANONYMOUS_USER_ID_CHILD").as_deref() != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }
    let id = get_or_create_anonymous_user_id(
        &AnonymousUserIdStore::default(),
        AnonymousUserIdOptions::default(),
    )
    .unwrap();
    let home = hoh_home_paths::resolve_dsh_home(None, None).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(ANONYMOUS_USER_ID_FILE_NAME)).unwrap(),
        format!("{id}\n")
    );
}

#[test]
fn ignored_helper_without_parent_marker_does_not_write() {
    let d = tempfile::tempdir().unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process_environment_child", "--ignored"])
        .env("DSH_HOME", d.path())
        .env_remove("HOH_ANONYMOUS_USER_ID_CHILD")
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
    assert!(!d.path().join(ANONYMOUS_USER_ID_FILE_NAME).exists());
}
