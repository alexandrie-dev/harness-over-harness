#![cfg(target_os = "linux")]

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use hoh_node_addon_system::try_lock_exclusive;
use tempfile::TempDir;

fn open(path: &std::path::Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap()
}

fn busy(error: std::io::Error) {
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::AGAIN.raw_os_error())
    );
    assert!(error.raw_os_error().unwrap() > 0);
}

struct Fixture {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Fixture {
    fn start(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut fixture = Self {
            child,
            input,
            output,
        };
        assert_eq!(fixture.line(), "ready");
        fixture
    }

    fn line(&mut self) -> String {
        loop {
            let mut line = String::new();
            assert_ne!(
                self.output.read_line(&mut line).unwrap(),
                0,
                "fixture exited before replying"
            );
            if let Some((_, message)) = line.trim_end().split_once('@') {
                return message.to_owned();
            }
        }
    }

    fn exchange(&mut self, message: &str) -> String {
        writeln!(self.input.as_mut().unwrap(), "{message}").unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
        self.line()
    }

    fn quit(&mut self) {
        assert_eq!(self.exchange("quit"), "bye");
        self.input.take();
        assert!(self.child.wait().unwrap().success());
    }

    fn rust(path: &std::path::Path, inherited: Option<&File>) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "flock_child", "--nocapture", "--quiet"])
            .env("HOH_FLOCK_CHILD", path);
        if let Some(file) = inherited {
            command
                .env("HOH_FLOCK_INHERITED", "1")
                .stderr(Stdio::from(file.try_clone().unwrap()));
        } else {
            command.stderr(Stdio::piped());
        }
        Self::start(command)
    }

    // util-linux flock is a separate syscall implementation, replacing the C oracle.
    fn oracle(path: &std::path::Path, shared: bool) -> Self {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                r#"
exec 3<>"$1"
printf '@ready\n'
while IFS= read -r action; do
    case "$action" in
        try) if /usr/bin/flock -n "$2" 3; then printf '@locked\n'; else printf '@busy\n'; fi ;;
        unlock) /usr/bin/flock -u 3 || exit 3; printf '@unlocked\n' ;;
        quit) printf '@bye\n'; exit 0 ;;
    esac
done
"#,
                "flock-oracle",
            ])
            .arg(path)
            .arg(if shared { "-s" } else { "-x" });
        Self::start(command)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn flock_child() {
    let Some(path) = std::env::var_os("HOH_FLOCK_CHILD") else {
        return;
    };
    let inherited = std::env::var_os("HOH_FLOCK_INHERITED").is_some();
    let mut file = Some(if inherited {
        File::from(rustix::io::dup(std::io::stderr()).unwrap())
    } else {
        open(std::path::Path::new(&path))
    });
    if inherited {
        // stderr was solely the transport of the inherited open file description.
        let null = open(std::path::Path::new("/dev/null"));
        rustix::stdio::dup2_stderr(&null).unwrap();
    }
    println!("@ready");
    std::io::stdout().flush().unwrap();
    for line in std::io::stdin().lock().lines() {
        match line.unwrap().as_str() {
            "try" => match try_lock_exclusive(file.as_ref().unwrap()) {
                Ok(()) => println!("@locked"),
                Err(error) => println!("@errno:{}", error.raw_os_error().unwrap()),
            },
            "close" => {
                file.take();
                println!("@closed");
            }
            "quit" => {
                println!("@bye");
                std::io::stdout().flush().unwrap();
                return;
            }
            command => panic!("unknown command {command}"),
        }
        std::io::stdout().flush().unwrap();
    }
}

#[test]
fn acquisition_resolves_to_void_and_the_same_fd_can_reacquire() {
    let root = TempDir::new().unwrap();
    let owner = open(&root.path().join("lock"));
    try_lock_exclusive(&owner).unwrap();
    try_lock_exclusive(&owner).unwrap();
}

#[test]
fn separate_opens_of_one_file_contend() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    let owner = open(&path);
    let contender = open(&path);
    try_lock_exclusive(&owner).unwrap();
    busy(try_lock_exclusive(&contender).unwrap_err());
}

#[test]
fn different_files_can_be_locked_concurrently() {
    let root = TempDir::new().unwrap();
    let first = open(&root.path().join("first"));
    let second = open(&root.path().join("second"));
    std::thread::scope(|scope| {
        scope.spawn(|| try_lock_exclusive(&first).unwrap());
        scope.spawn(|| try_lock_exclusive(&second).unwrap());
    });
}

#[test]
fn closing_the_locked_fd_allows_an_already_open_contender_to_acquire() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    let owner = open(&path);
    let contender = open(&path);
    try_lock_exclusive(&owner).unwrap();
    busy(try_lock_exclusive(&contender).unwrap_err());
    drop(owner);
    try_lock_exclusive(&contender).unwrap();
}

#[test]
fn closing_another_fd_for_the_same_file_does_not_release_the_lock() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    let owner = open(&path);
    let other = open(&path);
    let contender = open(&path);
    try_lock_exclusive(&owner).unwrap();
    drop(other);
    busy(try_lock_exclusive(&contender).unwrap_err());
    drop(owner);
    try_lock_exclusive(&contender).unwrap();
}

#[test]
fn invalid_fd_rejects_with_ebadf_and_positive_errno() {
    let root = TempDir::new().unwrap();
    // An O_PATH fd is safely constructible but invalid for flock (EBADF).
    let fd = rustix::fs::open(
        root.path(),
        rustix::fs::OFlags::PATH,
        rustix::fs::Mode::empty(),
    )
    .unwrap();
    let error = try_lock_exclusive(&fd).unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::BADF.raw_os_error())
    );
    assert!(error.raw_os_error().unwrap() > 0);
}

#[test]
fn concurrent_calls_retain_their_own_syscall_errno() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    let owner = open(&path);
    let contender = open(&path);
    let bad = rustix::fs::open(
        root.path(),
        rustix::fs::OFlags::PATH,
        rustix::fs::Mode::empty(),
    )
    .unwrap();
    try_lock_exclusive(&owner).unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| busy(try_lock_exclusive(&contender).unwrap_err()));
            scope.spawn(|| {
                assert_eq!(
                    try_lock_exclusive(&bad).unwrap_err().raw_os_error(),
                    Some(rustix::io::Errno::BADF.raw_os_error())
                )
            });
            scope.spawn(|| try_lock_exclusive(&owner).unwrap());
        }
    });
}

#[test]
fn two_child_processes_exclude_each_other_and_normal_close_transfers_ownership() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    let observer = open(&path);
    let mut first = Fixture::rust(&path, None);
    let mut second = Fixture::rust(&path, None);
    assert_eq!(first.exchange("try"), "locked");
    assert_eq!(
        second.exchange("try"),
        format!("errno:{}", rustix::io::Errno::AGAIN.raw_os_error())
    );
    busy(try_lock_exclusive(&observer).unwrap_err());
    assert_eq!(first.exchange("close"), "closed");
    first.quit();
    assert_eq!(second.exchange("try"), "locked");
    busy(try_lock_exclusive(&observer).unwrap_err());
    assert_eq!(second.exchange("close"), "closed");
    second.quit();
    try_lock_exclusive(&observer).unwrap();
}

#[test]
fn sigkill_releases_a_child_lock_after_exit() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    let observer = open(&path);
    let mut owner = Fixture::rust(&path, None);
    let mut contender = Fixture::rust(&path, None);
    assert_eq!(owner.exchange("try"), "locked");
    assert_eq!(
        contender.exchange("try"),
        format!("errno:{}", rustix::io::Errno::AGAIN.raw_os_error())
    );
    busy(try_lock_exclusive(&observer).unwrap_err());
    owner.child.kill().unwrap();
    let status = owner.child.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(9));
    assert_eq!(contender.exchange("try"), "locked");
    contender.quit();
    try_lock_exclusive(&observer).unwrap();
}

#[test]
fn an_addon_exclusive_lock_blocks_an_independent_exclusive_flock_until_its_fd_closes() {
    independent_contender(false);
}
#[test]
fn an_addon_exclusive_lock_blocks_an_independent_shared_flock_until_its_fd_closes() {
    independent_contender(true);
}
fn independent_contender(shared: bool) {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    let owner = open(&path);
    try_lock_exclusive(&owner).unwrap();
    let mut oracle = Fixture::oracle(&path, shared);
    assert_eq!(oracle.exchange("try"), "busy");
    drop(owner);
    assert_eq!(oracle.exchange("try"), "locked");
    oracle.quit();
}

#[test]
fn an_independent_exclusive_flock_blocks_the_addon_until_explicit_unlock() {
    independent_holder(false);
}
#[test]
fn an_independent_shared_flock_blocks_the_addon_until_explicit_unlock() {
    independent_holder(true);
}
fn independent_holder(shared: bool) {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    let contender = open(&path);
    let mut oracle = Fixture::oracle(&path, shared);
    assert_eq!(oracle.exchange("try"), "locked");
    // A reply arrives while the holder remains alive: no timing threshold or sleep.
    busy(try_lock_exclusive(&contender).unwrap_err());
    assert!(oracle.child.try_wait().unwrap().is_none());
    assert_eq!(oracle.exchange("unlock"), "unlocked");
    try_lock_exclusive(&contender).unwrap();
    oracle.quit();
}

#[test]
fn an_advisory_exclusive_lock_permits_another_process_to_read_and_write_without_locking() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    fs::write(&path, "written before locking").unwrap();
    let owner = open(&path);
    let contender = open(&path);
    try_lock_exclusive(&owner).unwrap();
    busy(try_lock_exclusive(&contender).unwrap_err());
    let output = Command::new("/bin/sh")
        .args([
            "-c",
            "cat \"$1\"; printf 'written without acquiring a lock' > \"$1\"",
            "io-child",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"written before locking");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "written without acquiring a lock"
    );
    busy(try_lock_exclusive(&contender).unwrap_err());
}

#[test]
fn an_inherited_fd_shares_the_lock_after_parent_close_until_the_child_closes_its_last_reference() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("lock");
    let owner = open(&path);
    let contender = open(&path);
    try_lock_exclusive(&owner).unwrap();
    let mut child = Fixture::rust(&path, Some(&owner));
    assert_eq!(child.exchange("try"), "locked");
    drop(owner);
    busy(try_lock_exclusive(&contender).unwrap_err());
    assert_eq!(child.exchange("close"), "closed");
    try_lock_exclusive(&contender).unwrap();
    assert!(child.child.try_wait().unwrap().is_none());
    child.quit();
}
