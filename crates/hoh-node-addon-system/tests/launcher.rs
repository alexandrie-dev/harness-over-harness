#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hoh_node_addon_system::{LAUNCHER_FAILURE_EXIT, run_launcher};
use tempfile::TempDir;

const PARTIAL: &str = "landlock-run: partial enforcement (older Landlock ABI)\n";

fn launcher() -> PathBuf {
    std::env::var_os("HOH_LANDLOCK_RUN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_landlock-run")))
}

fn run(args: &[&str], work: &Path) -> Output {
    Command::new(launcher())
        .args(args)
        .current_dir(work)
        .output()
        .unwrap()
}

fn confined(work: &Path, grants: &[(&str, &Path)]) -> Command {
    let mut command = Command::new(launcher());
    for (flag, path) in grants {
        command.arg(flag).arg(path);
    }
    command.current_dir(work).arg("--");
    command
}

fn shell(work: &Path, grants: &[(&str, &Path)], script: &str) -> Output {
    confined(work, grants)
        .args(["/bin/sh", "-c", script])
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

fn expected_notice(work: &Path) -> &'static str {
    let output = run(&["--probe"], work);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(output.stderr.is_empty(), "{}", text(&output.stderr));
    match text(&output.stdout) {
        "landlock: fully enforced\n" => "",
        "landlock: partially enforced (older ABI)\n" => PARTIAL,
        other => panic!("unexpected probe output: {other:?}"),
    }
}

fn notice(bytes: &[u8], work: &Path) {
    assert_eq!(text(bytes), expected_notice(work));
}

fn require_full(work: &Path) {
    let output = run(&["--probe"], work);
    assert!(
        output.status.success()
            && output.stdout == b"landlock: fully enforced\n"
            && output.stderr.is_empty(),
        "this test requires Landlock ABI 5 or newer: probe must report Full; status={}, stdout={:?}, stderr={:?}",
        output.status,
        text(&output.stdout),
        text(&output.stderr)
    );
}

fn runtime_grants() -> Vec<(&'static str, &'static Path)> {
    ["/usr", "/bin", "/lib", "/lib64"]
        .into_iter()
        .map(Path::new)
        .filter(|path| path.exists())
        .map(|path| ("--ro", path))
        .collect()
}

fn python(work: &Path, grants: &[(&str, &Path)], script: &str, paths: &[&Path]) -> Output {
    let tool = Path::new("/usr/bin/python3");
    assert!(
        tool.is_file(),
        "python3 is required at /usr/bin/python3 for direct syscall tests"
    );
    let output = confined(work, grants)
        .arg(tool)
        .args(["-I", "-c", script])
        .args(paths)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "python3 syscall helper failed: {}",
        text(&output.stderr)
    );
    notice(&output.stderr, work);
    output
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
    notice(&output.stderr, work.path());
    let output = shell(work.path(), &[("--ro", Path::new("/"))], "exit 7");
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn child_125_is_not_a_launcher_failure() {
    let work = TempDir::new().unwrap();
    let output = shell(work.path(), &[("--ro", Path::new("/"))], "exit 125");
    assert_eq!(output.status.code(), Some(125));
    notice(&output.stderr, work.path());
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
    notice(&output.stderr, work.path());
}

#[test]
fn path_outside_every_grant_is_refused() {
    let work = TempDir::new().unwrap();
    fs::write(work.path().join("secret"), "secret").unwrap();
    // Only runtime directories are readable: the temporary tree has no grant.
    let output = shell(work.path(), &runtime_grants(), "cat secret; touch denied");
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
    let output = shell(
        work.path(),
        &[("--ro", Path::new("/")), ("--rw", Path::new("/dev/null"))],
        "echo ok > /dev/null; cat /dev/null",
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    notice(&output.stderr, work.path());
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
    let output = Command::new(launcher())
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

#[test]
#[ignore = "passes only for the static launcher: run scripts/build-landlock-run.sh"]
fn static_launcher_ignores_loader_variables() {
    let work = TempDir::new().unwrap();
    let output = Command::new(launcher())
        .arg("--probe")
        .env("LD_PRELOAD", "/nonexistent-hoh-preload.so")
        .env("LD_LIBRARY_PATH", "/nonexistent-hoh-libs")
        .env("LD_AUDIT", "/nonexistent-hoh-audit.so")
        .current_dir(work.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stderr), "");
    assert!(matches!(
        text(&output.stdout),
        "landlock: fully enforced\n" | "landlock: partially enforced (older ABI)\n"
    ));
}

#[test]
fn refer_allows_links_inside_read_write_grant_and_refuses_imports() {
    // REFER (ABI 2): cross-directory linking succeeds only if handled and granted.
    let work = TempDir::new().unwrap();
    require_full(work.path());
    let writable = work.path().join("W");
    fs::create_dir_all(writable.join("a")).unwrap();
    fs::create_dir(writable.join("b")).unwrap();
    fs::create_dir(work.path().join("R")).unwrap();
    fs::write(writable.join("a/f"), "inside").unwrap();
    fs::write(work.path().join("R/f"), "outside").unwrap();
    let output = python(
        work.path(),
        &[("--ro", Path::new("/")), ("--rw", &writable)],
        r#"
import os
os.link('W/a/f', 'W/b/g')
print('inside=0')
try:
    os.link('R/f', 'W/imported')
except OSError as error:
    print('outside=' + str(error.errno))
else:
    print('outside=0')
"#,
        &[],
    );
    assert_eq!(
        text(&output.stdout),
        format!(
            "inside=0\noutside={}\n",
            rustix::io::Errno::XDEV.raw_os_error()
        )
    );
    assert_eq!(fs::read_to_string(writable.join("b/g")).unwrap(), "inside");
    assert!(!writable.join("imported").exists());
}

#[test]
fn truncate_by_path_requires_the_truncate_right() {
    // TRUNCATE (ABI 3): os.truncate(path, 0) does not first open with WRITE_FILE.
    let work = TempDir::new().unwrap();
    require_full(work.path());
    let file = work.path().join("target");
    fs::write(&file, "unchanged").unwrap();
    let script = r#"
import os, sys
try:
    os.truncate(sys.argv[1], 0)
except OSError as error:
    print(error.errno)
else:
    print(0)
"#;
    let denied = python(work.path(), &[("--ro", Path::new("/"))], script, &[&file]);
    assert_eq!(
        text(&denied.stdout),
        format!("{}\n", rustix::io::Errno::ACCESS.raw_os_error())
    );
    assert_eq!(fs::metadata(&file).unwrap().len(), 9);
    let allowed = python(
        work.path(),
        &[("--ro", Path::new("/")), ("--rw", work.path())],
        script,
        &[&file],
    );
    assert_eq!(text(&allowed.stdout), "0\n");
    assert_eq!(fs::metadata(&file).unwrap().len(), 0);
}

#[test]
fn ioctl_on_devices_requires_the_ioctl_dev_right() {
    // IOCTL_DEV (ABI 5): FIONREAD is denied before the device can return ENOTTY.
    let work = TempDir::new().unwrap();
    require_full(work.path());
    let script = r#"
import array, fcntl, termios
with open('/dev/zero', 'rb') as device:
    try:
        fcntl.ioctl(device, termios.FIONREAD, array.array('i', [0]), True)
    except OSError as error:
        print(error.errno)
    else:
        print(0)
"#;
    let denied = python(work.path(), &[("--ro", Path::new("/"))], script, &[]);
    assert_eq!(
        text(&denied.stdout),
        format!("{}\n", rustix::io::Errno::ACCESS.raw_os_error())
    );
    let allowed = python(
        work.path(),
        &[("--ro", Path::new("/")), ("--rw", Path::new("/dev/zero"))],
        script,
        &[],
    );
    assert_eq!(
        text(&allowed.stdout),
        format!("{}\n", rustix::io::Errno::NOTTY.raw_os_error())
    );
}

#[test]
fn full_or_partial_notice_strictly_matches_the_probe() {
    let work = TempDir::new().unwrap();
    let expected = expected_notice(work.path());
    let output = run(&["--ro", "/", "--", "/bin/true"], work.path());
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stderr), expected);
}

#[test]
fn read_write_regular_file_grant_allows_read_overwrite_and_truncate_but_no_sibling() {
    let work = TempDir::new().unwrap();
    let file = work.path().join("target");
    fs::write(&file, "before").unwrap();
    let mut grants = runtime_grants();
    grants.push(("--rw", &file));
    let output = python(
        work.path(),
        &grants,
        r#"
import os
with open('target', 'r+b') as file:
    print(file.read().decode('ascii'))
    file.seek(0)
    file.write(b'after!')
os.truncate('target', 2)
try:
    with open('sibling', 'wb') as file:
        file.write(b'denied')
except OSError as error:
    print(error.errno)
else:
    print(0)
"#,
        &[],
    );
    assert_eq!(
        text(&output.stdout),
        format!("before\n{}\n", rustix::io::Errno::ACCESS.raw_os_error())
    );
    assert_eq!(fs::read(&file).unwrap(), b"af");
    assert!(!work.path().join("sibling").exists());
}

#[test]
fn read_only_regular_file_grant_allows_reading_but_not_writing() {
    let work = TempDir::new().unwrap();
    let file = work.path().join("target");
    fs::write(&file, "before").unwrap();
    let mut grants = runtime_grants();
    grants.push(("--ro", &file));
    let output = python(
        work.path(),
        &grants,
        r#"
with open('target', 'rb') as file:
    print(file.read().decode('ascii'))
try:
    with open('target', 'r+b') as file:
        file.write(b'denied')
except OSError as error:
    print(error.errno)
else:
    print(0)
"#,
        &[],
    );
    assert_eq!(
        text(&output.stdout),
        format!("before\n{}\n", rustix::io::Errno::ACCESS.raw_os_error())
    );
    assert_eq!(fs::read(&file).unwrap(), b"before");
}

#[test]
fn commands_without_execute_permission_or_naming_a_directory_fail() {
    let work = TempDir::new().unwrap();
    let file = work.path().join("non_executable");
    fs::write(&file, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    let expected = format!(
        "{}landlock-run: exec failed: Permission denied\n",
        expected_notice(work.path())
    );
    for path in [&file, work.path()] {
        let output = confined(work.path(), &[("--ro", Path::new("/"))])
            .arg(path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(125), "{}", path.display());
        assert!(output.stdout.is_empty());
        assert_eq!(text(&output.stderr), expected, "{}", path.display());
    }
}

#[test]
fn command_sees_sigpipe_with_default_disposition() {
    let work = TempDir::new().unwrap();
    let output = run(
        &["--ro", "/", "--", "/bin/cat", "/proc/self/status"],
        work.path(),
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    notice(&output.stderr, work.path());
    let ignored = text(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("SigIgn:"))
        .expect("/proc/self/status must contain SigIgn");
    let mask = u64::from_str_radix(ignored.trim(), 16).unwrap();
    assert_eq!(
        mask & (1 << (13 - 1)),
        0,
        "SIGPIPE must not be ignored: SigIgn={ignored}"
    );
}
