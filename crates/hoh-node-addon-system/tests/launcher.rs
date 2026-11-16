#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use hoh_node_addon_system::{LAUNCHER_FAILURE_EXIT, run_launcher};
use tempfile::TempDir;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_landlock-run");
const PARTIAL: &str = "landlock-run: partial enforcement (older Landlock ABI)\n";

fn run(args: &[&str], work: &Path) -> Output {
    Command::new(LAUNCHER)
        .args(args)
        .current_dir(work)
        .output()
        .unwrap()
}

fn shell(work: &Path, grants: &[(&str, &Path)], script: &str) -> Output {
    let mut command = Command::new(LAUNCHER);
    for (flag, path) in grants {
        command.arg(flag).arg(path);
    }
    command
        .args(["--", "/bin/sh", "-c", script])
        .current_dir(work)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

fn notice(bytes: &[u8]) {
    assert!(
        bytes.is_empty() || bytes == PARTIAL.as_bytes(),
        "{}",
        text(bytes)
    );
}

fn usage(args: &[&str], message: &str) {
    let expected = format!("landlock-run: usage error: {message}\n");
    // This branch cannot apply Landlock; direct function use is safe here.
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    assert_eq!(
        run_launcher(args.iter().map(OsString::from), &mut stdout, &mut stderr),
        LAUNCHER_FAILURE_EXIT
    );
    assert!(stdout.is_empty());
    assert_eq!(text(&stderr), expected);
    let temp = TempDir::new().unwrap();
    let output = run(args, temp.path());
    assert_eq!(output.status.code(), Some(125));
    assert!(output.stdout.is_empty());
    assert_eq!(text(&output.stderr), expected);
}

#[test]
fn no_command() {
    usage(&[], "missing `-- <argv>...` command");
}
#[test]
fn empty_command_after_separator() {
    usage(&["--"], "missing `-- <argv>...` command");
}
#[test]
fn grants_without_command() {
    usage(
        &["--ro", "/", "--rw", "/tmp"],
        "missing `-- <argv>...` command",
    );
}
#[test]
fn unknown_flag() {
    usage(&["--bogus", "--", "true"], "unknown argument: --bogus");
}
#[test]
fn command_without_separator() {
    usage(&["/bin/sh"], "unknown argument: /bin/sh");
}
#[test]
fn dangling_read_only_path() {
    usage(&["--ro"], "--ro requires a path");
}
#[test]
fn dangling_read_write_path() {
    usage(&["--rw"], "--rw requires a path");
}
#[test]
fn probe_takes_no_other_arguments() {
    for args in [
        vec!["--probe", "--ro", "/"],
        vec!["--probe", "--"],
        vec!["--probe", "--probe"],
        vec!["--ro", "/", "--probe"],
        vec!["--rw", "/", "--probe"],
    ] {
        usage(&args, "--probe takes no other arguments");
    }
}
#[test]
fn flags_used_as_path_are_consumed_literally() {
    usage(&["--ro", "--probe"], "missing `-- <argv>...` command");
    usage(&["--rw", "--"], "missing `-- <argv>...` command");
}

#[test]
fn probe_the_functional_availability_signal() {
    let work = TempDir::new().unwrap();
    let output = run(&["--probe"], work.path());
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(output.stderr.is_empty());
    assert!(matches!(
        text(&output.stdout),
        "landlock: fully enforced\n" | "landlock: partially enforced (older ABI)\n"
    ));
}

#[test]
fn confined_exec_and_command_exit_code_pass_through() {
    let work = TempDir::new().unwrap();
    let output = shell(work.path(), &[("--ro", Path::new("/"))], "echo confined-ok");
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "confined-ok\n");
    notice(&output.stderr);
    let output = shell(work.path(), &[("--ro", Path::new("/"))], "exit 7");
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn child_125_is_not_a_launcher_failure() {
    let work = TempDir::new().unwrap();
    let output = shell(work.path(), &[("--ro", Path::new("/"))], "exit 125");
    assert_eq!(output.status.code(), Some(125));
    notice(&output.stderr);
    // A partial notice is allowed by the C source, but no fatal diagnostic.
    assert!(!text(&output.stderr).contains("exec failed"));
}

#[test]
fn read_only_grant_allows_reading_and_refuses_writing() {
    let work = TempDir::new().unwrap();
    fs::write(work.path().join("input"), "read-ok\n").unwrap();
    let output = shell(
        work.path(),
        &[("--ro", Path::new("/"))],
        "cat input; touch denied",
    );
    assert_ne!(output.status.code(), Some(0));
    assert_eq!(text(&output.stdout), "read-ok\n");
    assert!(!work.path().join("denied").exists());
    assert!(text(&output.stderr).contains("Permission denied"));
}

#[test]
fn read_write_grant_allows_reading_and_writing() {
    let work = TempDir::new().unwrap();
    let output = shell(
        work.path(),
        &[("--ro", Path::new("/")), ("--rw", work.path())],
        "echo ok > granted; cat granted; touch touched",
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "ok\n");
    assert_eq!(
        fs::read_to_string(work.path().join("granted")).unwrap(),
        "ok\n"
    );
    assert!(work.path().join("touched").exists());
    notice(&output.stderr);
}

#[test]
fn path_outside_every_grant_is_refused() {
    let work = TempDir::new().unwrap();
    fs::write(work.path().join("secret"), "secret").unwrap();
    // Only runtime directories are readable: the temporary tree has no grant.
    let runtime_grants: Vec<_> = ["/usr", "/bin", "/lib", "/lib64"]
        .into_iter()
        .map(Path::new)
        .filter(|path| path.exists())
        .map(|path| ("--ro", path))
        .collect();
    let output = shell(work.path(), &runtime_grants, "cat secret; touch denied");
    assert_ne!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(
        text(&output.stderr).contains("Permission denied"),
        "{}",
        text(&output.stderr)
    );
    assert!(!work.path().join("denied").exists());
    assert!(!text(&output.stderr).contains("landlock-run: exec failed"));
}

#[test]
fn grant_on_a_file_dev_null() {
    let work = TempDir::new().unwrap();
    let probe = run(&["--probe"], work.path());
    let output = shell(
        work.path(),
        &[("--ro", Path::new("/")), ("--rw", Path::new("/dev/null"))],
        "echo ok > /dev/null; cat /dev/null",
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    if text(&probe.stdout) == "landlock: fully enforced\n" {
        assert!(output.stderr.is_empty());
    }
    notice(&output.stderr);
}

#[test]
fn unopenable_grant_root_refuses_to_exec() {
    let work = TempDir::new().unwrap();
    let missing = work.path().join("missing");
    let output = shell(
        work.path(),
        &[
            ("--ro", Path::new("/")),
            ("--rw", work.path()),
            ("--ro", &missing),
        ],
        "echo x > marker",
    );
    assert_eq!(output.status.code(), Some(125));
    assert_eq!(
        text(&output.stderr),
        format!(
            "landlock-run: cannot open rule path: {}: No such file or directory\n",
            missing.display()
        )
    );
    assert!(!work.path().join("marker").exists());
}

#[test]
fn nonexistent_command_is_launcher_failure() {
    let work = TempDir::new().unwrap();
    let output = run(&["--ro", "/", "--", "./nonexistent-command"], work.path());
    assert_eq!(output.status.code(), Some(125));
    assert!(
        text(&output.stderr).ends_with("landlock-run: exec failed: No such file or directory\n"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn child_of_the_confined_command_is_confined_too() {
    let work = TempDir::new().unwrap();
    let output = shell(
        work.path(),
        &[("--ro", Path::new("/"))],
        "/bin/sh -c 'echo x > nested'; true",
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(!work.path().join("nested").exists());
    assert!(text(&output.stderr).contains("Permission denied"));
}

#[test]
fn path_resolution_argv_environment_and_no_new_privs() {
    let work = TempDir::new().unwrap();
    let output = Command::new(LAUNCHER)
        .args([
            "--ro",
            "/",
            "--",
            "sh",
            "-c",
            "printf '%s:%s\\n' \"$1\" \"$HOH_TEST_VALUE\"; cat /proc/self/status",
            "argv-zero",
            "--probe",
        ])
        .env("PATH", "/bin:/usr/bin")
        .env("HOH_TEST_VALUE", "unchanged")
        .current_dir(work.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(text(&output.stdout).starts_with("--probe:unchanged\n"));
    assert!(text(&output.stdout).contains("NoNewPrivs:\t1\n"));
}
