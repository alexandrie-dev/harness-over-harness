use hoh_subprocess::SubprocessTerminalSignal;
use hoh_subprocess_local::process_inspector::*;
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Files {
    files: HashMap<String, String>,
    dirs: HashMap<String, Vec<String>>,
    links: HashMap<String, String>,
    devices: HashMap<String, (u64, bool)>,
    memory: HashMap<u32, Vec<u8>>,
    kills: Vec<(i32, rustix::process::Signal)>,
    errors: HashMap<String, i32>,
}
#[derive(Default)]
struct Fake(Mutex<Files>);
fn missing() -> io::Error {
    io::Error::from(io::ErrorKind::NotFound)
}
impl ProcessInspectorInternals for Fake {
    fn read_file(&self, path: &str) -> io::Result<String> {
        self.0
            .lock()
            .unwrap()
            .files
            .get(path)
            .cloned()
            .ok_or_else(missing)
    }
    fn read_dir(&self, path: &str) -> io::Result<Vec<String>> {
        let files = self.0.lock().unwrap();
        if let Some(code) = files.errors.get(path) {
            return Err(io::Error::from_raw_os_error(*code));
        }
        files.dirs.get(path).cloned().ok_or_else(missing)
    }
    fn read_link(&self, path: &str) -> io::Result<String> {
        self.0
            .lock()
            .unwrap()
            .links
            .get(path)
            .cloned()
            .ok_or_else(missing)
    }
    fn stat(&self, path: &str) -> io::Result<FileStatus> {
        self.0
            .lock()
            .unwrap()
            .devices
            .get(path)
            .map(|(rdev, character_device)| FileStatus {
                rdev: *rdev,
                character_device: *character_device,
            })
            .ok_or_else(missing)
    }
    fn read_memory(&self, pid: u32, address: u64, length: usize) -> io::Result<Vec<u8>> {
        let files = self.0.lock().unwrap();
        let bytes = files.memory.get(&pid).ok_or_else(missing)?;
        let address = usize::try_from(address).map_err(|_| missing())?;
        Ok(bytes
            .get(address..)
            .unwrap_or_default()
            .iter()
            .take(length)
            .copied()
            .collect())
    }
    fn signal(&self, pid: i32, signal: rustix::process::Signal) -> io::Result<()> {
        self.0.lock().unwrap().kills.push((pid, signal));
        Ok(())
    }
}
fn stat(
    pid: u32,
    group: i32,
    session: i32,
    foreground: i32,
    started: &str,
    parent: u32,
    state: &str,
) -> String {
    let mut fields = vec![
        state.into(),
        parent.to_string(),
        group.to_string(),
        session.to_string(),
        "99".into(),
        foreground.to_string(),
    ];
    fields.resize(19, "0".into());
    fields.push(started.into());
    format!("{pid} (command with space) {}", fields.join(" "))
}
fn inspector(fake: &Arc<Fake>) -> LinuxProcessInspector {
    LinuxProcessInspector {
        arch: "x64".into(),
        internals: fake.clone(),
    }
}
#[test]
fn refuses_to_publish_an_empty_snapshot_when_proc_enumeration_fails() {
    let fake = Arc::new(Fake::default());
    let inspector = inspector(&fake);
    assert!(
        inspector
            .snapshot()
            .err()
            .unwrap()
            .to_string()
            .contains("/proc directory is unreadable")
    );
    {
        let mut files = fake.0.lock().unwrap();
        files.dirs.insert("/proc".into(), vec!["10".into()]);
        files
            .files
            .insert("/proc/10/stat".into(), stat(10, 10, 10, 10, "500", 1, "S"));
    }
    let observed = inspector.snapshot().unwrap();
    assert!(observed.complete);
    assert_eq!(
        observed.tree(10),
        [ProcessIdentity {
            pid: 10,
            started: "500".into()
        }]
    );
}

#[test]
fn refuses_to_publish_an_empty_snapshot_on_eacces_or_emfile() {
    for code in [13, 24] {
        let fake = Arc::new(Fake::default());
        fake.0.lock().unwrap().errors.insert("/proc".into(), code);
        assert_eq!(
            inspector(&fake).snapshot().err().unwrap().to_string(),
            "Cannot inspect processes: /proc directory is unreadable"
        );
    }
}

#[test]
fn contains_unreadable_syscall_memory_and_fdinfo_boundaries() {
    let fake = Arc::new(Fake::default());
    let inspector = inspector(&fake);
    {
        let mut files = fake.0.lock().unwrap();
        files.dirs.insert("/proc".into(), vec!["100".into()]);
        files
            .dirs
            .insert("/proc/100/task".into(), vec!["100".into()]);
    }
    assert!(!inspector.is_stdin_waiting(77, 100));
    {
        let mut files = fake.0.lock().unwrap();
        files
            .files
            .insert("/proc/100/stat".into(), stat(100, 77, 100, 77, "1", 1, "S"));
    }
    assert!(!inspector.is_stdin_waiting(77, 100));
    {
        let mut files = fake.0.lock().unwrap();
        for path in ["/proc/100/fd/0", "/proc/100/task/100/fd/0"] {
            files.links.insert(path.into(), "/dev/pts/1".into());
            files.devices.insert(path.into(), (99, true));
        }
    }
    assert!(!inspector.is_stdin_waiting(77, 100));
    for syscall in [
        "270 0x1 0x10 0x0 0x0 0x0 0x0 0x0 0x0",
        "232 0x5 0x0 0x1 0x0 0x0 0x0 0x0 0x0",
        "7 0x20 0x1 0x0 0x0 0x0 0x0 0x0 0x0",
    ] {
        fake.0
            .lock()
            .unwrap()
            .files
            .insert("/proc/100/task/100/syscall".into(), syscall.into());
        assert!(!inspector.is_stdin_waiting(77, 100));
    }
    let mut bytes = vec![0; 0x28];
    bytes[0x20..0x24].copy_from_slice(&2i32.to_le_bytes());
    bytes[0x24..0x26].copy_from_slice(&1i16.to_le_bytes());
    fake.0.lock().unwrap().memory.insert(100, bytes);
    assert!(!inspector.is_stdin_waiting(77, 100));
}
#[test]
fn makes_no_stdin_wait_claim_when_process_or_task_directories_cannot_be_enumerated() {
    let fake = Arc::new(Fake::default());
    {
        let mut files = fake.0.lock().unwrap();
        files
            .files
            .insert("/proc/10/stat".into(), stat(10, 10, 10, 10, "500", 1, "S"));
        files
            .links
            .insert("/proc/10/fd/0".into(), "/dev/pts/1".into());
        files.devices.insert("/proc/10/fd/0".into(), (99, true));
    }
    let inspector = inspector(&fake);
    assert!(!inspector.is_stdin_waiting(10, 10));
    fake.0
        .lock()
        .unwrap()
        .dirs
        .insert("/proc".into(), vec!["10".into()]);
    assert!(!inspector.is_stdin_waiting(10, 10));
}
#[test]
fn treats_zombie_only_process_groups_as_quiescent_and_fails_closed_when_unobservable() {
    let fake = Arc::new(Fake::default());
    assert_eq!(
        linux_process_group_live_member(77, None, fake.clone()),
        None
    );
    {
        let mut files = fake.0.lock().unwrap();
        files.dirs.insert(
            "/proc".into(),
            vec!["self".into(), "10".into(), "11".into(), "12".into()],
        );
        for (pid, group, state) in [(10, 77, "Z"), (11, 77, "X"), (12, 88, "S")] {
            files.files.insert(
                format!("/proc/{pid}/stat"),
                stat(pid, group, pid as i32, -1, "500", 1, state),
            );
        }
    }
    assert_eq!(
        linux_process_group_live_member(77, None, fake.clone()),
        Some(None)
    );
    assert_eq!(
        linux_process_group_live_member(99, None, fake.clone()),
        None
    );
    fake.0
        .lock()
        .unwrap()
        .files
        .insert("/proc/11/stat".into(), stat(11, 77, 11, -1, "501", 1, "S"));
    assert_eq!(
        linux_process_group_live_member(77, None, fake),
        Some(Some(11))
    );
}
#[test]
fn rechecks_a_known_live_group_member_without_enumerating_proc() {
    let fake = Arc::new(Fake::default());
    fake.0
        .lock()
        .unwrap()
        .files
        .insert("/proc/11/stat".into(), stat(11, 77, 11, -1, "501", 1, "S"));
    assert_eq!(
        linux_process_group_live_member(77, Some(11), fake.clone()),
        Some(Some(11))
    );
    fake.0
        .lock()
        .unwrap()
        .files
        .insert("/proc/11/stat".into(), stat(11, 77, 11, -1, "501", 1, "Z"));
    assert_eq!(linux_process_group_live_member(77, Some(11), fake), None);
}
#[test]
fn parses_stat_safely_captures_only_the_rooted_process_tree_and_signals_identities() {
    for text in [
        "bad".into(),
        "1 () ".into(),
        "1 () S".into(),
        stat(10, 20, 30, 40, "500", 1, "SS"),
    ] {
        assert!(parse_proc_stat(&text).is_none());
    }
    let parsed = parse_proc_stat(&stat(10, 20, 30, 40, "500", 1, "S")).unwrap();
    assert_eq!(
        (
            parsed.pid,
            parsed.parent_pid,
            parsed.pgrp,
            parsed.session,
            parsed.tty_device,
            parsed.tpgid,
            parsed.started
        ),
        (10, 1, 20, 30, 99, 40, "500".into())
    );
    let fake = Arc::new(Fake::default());
    {
        let mut files = fake.0.lock().unwrap();
        files.dirs.insert(
            "/proc".into(),
            ["x", "10", "11", "12", "13", "14"].map(Into::into).to_vec(),
        );
        for (pid, group, parent, fg) in [
            (10, 20, 1, 40),
            (11, 21, 1, -1),
            (12, 22, 10, -1),
            (13, 23, 12, -1),
        ] {
            files.files.insert(
                format!("/proc/{pid}/stat"),
                stat(pid, group, 30, fg, &(pid + 490).to_string(), parent, "S"),
            );
        }
    }
    let inspector = inspector(&fake);
    assert_eq!(inspector.foreground_pgid(10), Some(40));
    assert_eq!(inspector.foreground_pgid(11), None);
    assert_eq!(inspector.foreground_pgid(99), None);
    let observed = inspector.snapshot().unwrap();
    assert!(!observed.complete);
    assert_eq!(
        observed.tree(10).iter().map(|p| p.pid).collect::<Vec<_>>(),
        [13, 12, 10]
    );
    assert!(observed.tree(99).is_empty());
    assert_eq!(
        observed
            .session(30)
            .iter()
            .map(|p| p.pid)
            .collect::<Vec<_>>(),
        [10, 11, 12, 13]
    );
    assert!(observed.session(99).is_empty());
    let identity = ProcessIdentity {
        pid: 10,
        started: "500".into(),
    };
    let old = ProcessIdentity {
        pid: 10,
        started: "old".into(),
    };
    assert!(observed.alive(&identity));
    assert!(!observed.alive(&old));
    inspector
        .signal_group(40, SubprocessTerminalSignal::Sigint)
        .unwrap();
    inspector
        .signal_process(&identity, rustix::process::Signal::TERM)
        .unwrap();
    inspector
        .signal_process(&old, rustix::process::Signal::KILL)
        .unwrap();
    assert_eq!(
        fake.0.lock().unwrap().kills,
        [
            (-40, rustix::process::Signal::INT),
            (10, rustix::process::Signal::TERM)
        ]
    );
    fake.0
        .lock()
        .unwrap()
        .files
        .insert("/proc/10/stat".into(), stat(10, 20, 30, 40, "500", 1, "Z"));
    assert!(!inspector.snapshot().unwrap().alive(&identity));
    assert!(!inspector.is_alive(&identity));
    inspector
        .signal_process(&identity, rustix::process::Signal::KILL)
        .unwrap();
    assert_eq!(fake.0.lock().unwrap().kills.len(), 2);
}
fn waiting() -> Arc<Fake> {
    let fake = Arc::new(Fake::default());
    {
        let mut files = fake.0.lock().unwrap();
        files
            .dirs
            .insert("/proc".into(), ["100", "101"].map(Into::into).to_vec());
        for pid in [100, 101] {
            files.files.insert(
                format!("/proc/{pid}/stat"),
                stat(pid, 77, 100, 77, &pid.to_string(), 1, "S"),
            );
        }
        files
            .dirs
            .insert("/proc/100/task".into(), vec!["100".into()]);
        files
            .dirs
            .insert("/proc/101/task".into(), vec!["101".into(), "102".into()]);
        for path in ["/proc/100/fd/0", "/proc/101/task/102/fd/0"] {
            files.links.insert(path.into(), "/dev/pts/1".into());
            files.devices.insert(path.into(), (99, true));
        }
        files
            .files
            .insert("/proc/100/task/100/syscall".into(), "running".into());
        files
            .files
            .insert("/proc/101/task/101/syscall".into(), "-1 0x0".into());
    }
    fake
}
fn syscall(number: u64, args: &[u64]) -> String {
    format!(
        "{number} {}",
        (0..6)
            .map(|n| format!("0x{:x}", args.get(n).copied().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join(" ")
    )
}
#[test]
fn detects_supported_kernel_abi_waits_across_non_leader_threads() {
    let fake = waiting();
    let inspector = inspector(&fake);
    for call in [syscall(0, &[0]), syscall(63, &[0])] {
        fake.0
            .lock()
            .unwrap()
            .files
            .insert("/proc/101/task/102/syscall".into(), call);
        assert!(inspector.is_stdin_waiting(77, 100));
    }
    let mut fdset = vec![0; 0x11];
    fdset[0x10] = 1;
    fake.0.lock().unwrap().memory.insert(101, fdset);
    fake.0.lock().unwrap().files.insert(
        "/proc/101/task/102/syscall".into(),
        syscall(270, &[1, 0x10]),
    );
    assert!(inspector.is_stdin_waiting(77, 100));
    let mut poll = vec![0; 0x28];
    poll[0x24] = 1;
    fake.0.lock().unwrap().memory.insert(101, poll);
    fake.0
        .lock()
        .unwrap()
        .files
        .insert("/proc/101/task/102/syscall".into(), syscall(7, &[0x20, 1]));
    assert!(inspector.is_stdin_waiting(77, 100));
    {
        let mut files = fake.0.lock().unwrap();
        files.files.insert(
            "/proc/101/task/102/syscall".into(),
            syscall(232, &[5, 0, 1]),
        );
        files.files.insert(
            "/proc/101/task/102/fdinfo/5".into(),
            "pos: 0\ntfd: 0 events: 19\n".into(),
        );
    }
    assert!(inspector.is_stdin_waiting(77, 100));
}
#[test]
fn uses_the_waiting_thread_fd_table_and_recognizes_the_controlling_terminal_alias() {
    let fake = waiting();
    fake.0
        .lock()
        .unwrap()
        .files
        .insert("/proc/101/task/102/syscall".into(), syscall(0, &[0]));
    let inspector = inspector(&fake);
    for (link, device, expected) in [
        ("pipe:[123]", (0, false), false),
        ("/dev/tty", (0, false), true),
        ("/dev/pts/2", (100, true), false),
    ] {
        let mut files = fake.0.lock().unwrap();
        files
            .links
            .insert("/proc/101/task/102/fd/0".into(), link.into());
        files
            .devices
            .insert("/proc/101/task/102/fd/0".into(), device);
        drop(files);
        assert_eq!(inspector.is_stdin_waiting(77, 100), expected);
    }
    fake.0
        .lock()
        .unwrap()
        .links
        .remove("/proc/101/task/102/fd/0");
    assert!(!inspector.is_stdin_waiting(77, 100));
}
#[test]
fn fails_closed_on_unsupported_malformed_unreadable_or_non_stdin_waits() {
    let fake = waiting();
    let inspector = inspector(&fake);
    for call in [
        syscall(0, &[2]),
        syscall(270, &[1, 0]),
        syscall(7, &[0, 0]),
        syscall(7, &[0, 1]),
        syscall(7, &[0x20, 1]),
        syscall(232, &[9, 0, 1]),
        syscall(999, &[]),
        "not-a-number 0x0".into(),
        "0".into(),
        syscall(0, &[0, 1 << 53]),
    ] {
        fake.0
            .lock()
            .unwrap()
            .files
            .insert("/proc/101/task/102/syscall".into(), call);
        assert!(!inspector.is_stdin_waiting(77, 100));
    }
    fake.0
        .lock()
        .unwrap()
        .files
        .insert("/proc/101/task/102/syscall".into(), syscall(0, &[0]));
    let unsupported = LinuxProcessInspector {
        arch: "mips".into(),
        internals: fake,
    };
    assert!(!unsupported.is_stdin_waiting(77, 100));
}
