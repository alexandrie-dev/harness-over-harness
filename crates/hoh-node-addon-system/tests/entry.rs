#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hoh_node_addon_system::{
    LAUNCHER_BIN, LAUNCHER_FAILURE_EXIT, LandlockEnforcement::*, LauncherGrants, grant_args,
    launcher_path, probe,
};
use tempfile::TempDir;

fn fake(root: &Path, name: &str, script: &str) -> PathBuf {
    let file = root.join(name);
    fs::write(&file, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    file
}

#[test]
fn constants_are_part_of_the_cli_contract() {
    assert_eq!(LAUNCHER_BIN, "landlock-run");
    assert_eq!(LAUNCHER_FAILURE_EXIT, 125);
}

#[test]
fn grant_args_flag_spelling_ordering_and_empty_grants() {
    assert!(grant_args(&LauncherGrants::default()).is_empty());
    assert_eq!(
        grant_args(&LauncherGrants {
            read_only: vec!["/".into()],
            ..Default::default()
        }),
        ["--ro", "/"]
    );
    assert_eq!(
        grant_args(&LauncherGrants {
            read_only: vec!["/".into(), "/opt".into()],
            read_write: vec!["/tmp/work".into()]
        }),
        ["--ro", "/", "--ro", "/opt", "--rw", "/tmp/work"]
    );
    assert_eq!(
        grant_args(&LauncherGrants {
            read_write: vec!["/a".into()],
            read_only: vec!["/b".into()]
        }),
        ["--ro", "/b", "--rw", "/a"]
    );
}

#[test]
fn launcher_path_is_absolute_and_next_to_current_executable() {
    let path = launcher_path().unwrap();
    assert!(path.is_absolute());
    assert_eq!(
        path,
        std::env::current_exe()
            .unwrap()
            .with_file_name("landlock-run")
    );
}

#[test]
fn missing_launcher_is_unusable() {
    let root = TempDir::new().unwrap();
    assert_eq!(probe(Some(&root.path().join("missing")), None), Unusable);
}

#[test]
fn verdict_parsing_over_fake_launchers() {
    let root = TempDir::new().unwrap();
    for (name, script, expected) in [
        ("full", "echo 'landlock: fully enforced'; exit 0", Full),
        (
            "partial",
            "echo 'landlock: partially enforced (older ABI)'; exit 0",
            Partial,
        ),
        ("failing", "exit 125", Unusable),
        ("empty", "exit 0", Full),
        (
            "substring",
            "echo 'prefix partially enforced suffix'",
            Partial,
        ),
        ("stderr", "echo 'partially enforced' >&2", Full),
        (
            "partial_failure",
            "echo 'partially enforced'; exit 7",
            Unusable,
        ),
        ("signal", "kill -TERM $$", Unusable),
    ] {
        assert_eq!(
            probe(Some(&fake(root.path(), name, script)), None),
            expected,
            "{name}"
        );
    }
}

#[test]
fn hanging_probe_is_unusable() {
    let root = TempDir::new().unwrap();
    let launcher = fake(root.path(), "hanging", "exec sleep 10");
    assert_eq!(
        probe(Some(&launcher), Some(Duration::from_millis(200))),
        Unusable
    );
}

#[test]
fn real_launcher_probe() {
    assert!(matches!(
        probe(Some(Path::new(env!("CARGO_BIN_EXE_landlock-run"))), None),
        Full | Partial
    ));
}

#[test]
fn zero_timeout_is_unbounded_and_large_output_is_drained() {
    let root = TempDir::new().unwrap();
    let launcher = fake(
        root.path(),
        "large",
        "i=0; while [ $i -lt 10000 ]; do echo 'output'; i=$((i+1)); done; echo 'partially enforced'",
    );
    assert_eq!(probe(Some(&launcher), Some(Duration::ZERO)), Partial);
}

#[test]
fn excessive_output_is_unusable() {
    let root = TempDir::new().unwrap();
    let launcher = fake(root.path(), "overflow", "exec head -c 1048577 /dev/zero");
    assert_eq!(probe(Some(&launcher), None), Unusable);
}
