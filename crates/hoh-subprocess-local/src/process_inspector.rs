//! Linux process identities, session ownership and exact terminal-input probes.
use crate::error;
use hoh_error::BoxError;
use hoh_subprocess::SubprocessTerminalSignal;
use hoh_util_values::{is_js_white_space, js_trim};
use indexmap::{IndexMap, IndexSet};
use std::{
    fs::File,
    io,
    os::unix::fs::{FileExt, FileTypeExt, MetadataExt},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started: String,
}
#[derive(Clone, Debug)]
pub struct ProcStat {
    pub pid: u32,
    pub parent_pid: u32,
    pub pgrp: i32,
    pub session: i32,
    pub state: String,
    pub tty_device: i64,
    pub tpgid: i32,
    pub started: String,
}
pub fn parse_proc_stat(text: &str) -> Option<ProcStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if open == 0 || close <= open {
        return None;
    }
    let pid = js_trim(&text[..open]).parse().ok()?;
    let fields = js_trim(text.get(close + 2..)?)
        .split(is_js_white_space)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let state = *fields.first()?;
    if state.encode_utf16().count() != 1 {
        return None;
    }
    Some(ProcStat {
        pid,
        parent_pid: fields.get(1)?.parse().ok()?,
        pgrp: fields.get(2)?.parse().ok()?,
        session: fields.get(3)?.parse().ok()?,
        state: state.into(),
        tty_device: fields.get(4)?.parse().ok()?,
        tpgid: fields.get(5)?.parse().ok()?,
        started: fields.get(19)?.to_string(),
    })
}
pub struct FileStatus {
    pub rdev: u64,
    pub character_device: bool,
}
pub trait ProcessInspectorInternals: Send + Sync {
    fn read_file(&self, path: &str) -> io::Result<String>;
    fn read_dir(&self, path: &str) -> io::Result<Vec<String>>;
    fn read_link(&self, path: &str) -> io::Result<String>;
    fn stat(&self, path: &str) -> io::Result<FileStatus>;
    fn read_memory(&self, pid: u32, address: u64, length: usize) -> io::Result<Vec<u8>>;
    fn signal(&self, pid: i32, signal: rustix::process::Signal) -> io::Result<()>;
}
pub struct NativeProcessInspector;
impl ProcessInspectorInternals for NativeProcessInspector {
    fn read_file(&self, path: &str) -> io::Result<String> {
        crate::read_utf8(path)
    }
    fn read_dir(&self, path: &str) -> io::Result<Vec<String>> {
        let mut names = std::fs::read_dir(path)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        Ok(names)
    }
    fn read_link(&self, path: &str) -> io::Result<String> {
        Ok(std::fs::read_link(path)?.to_string_lossy().into_owned())
    }
    fn stat(&self, path: &str) -> io::Result<FileStatus> {
        let stat = std::fs::metadata(path)?;
        Ok(FileStatus {
            rdev: stat.rdev(),
            character_device: stat.file_type().is_char_device(),
        })
    }
    fn read_memory(&self, pid: u32, address: u64, length: usize) -> io::Result<Vec<u8>> {
        let file = File::open(format!("/proc/{pid}/mem"))?;
        let mut bytes = vec![0; length];
        let count = file.read_at(&mut bytes, address)?;
        bytes.truncate(count);
        Ok(bytes)
    }
    fn signal(&self, pid: i32, signal: rustix::process::Signal) -> io::Result<()> {
        let target = rustix::process::Pid::from_raw(pid.unsigned_abs() as i32)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid process id"))?;
        if pid < 0 {
            rustix::process::kill_process_group(target, signal)
        } else {
            rustix::process::kill_process(target, signal)
        }
        .map_err(Into::into)
    }
}
pub fn quiescent(state: &str) -> bool {
    matches!(state, "Z" | "X" | "x")
}
#[derive(Clone)]
pub struct ProcessSnapshot {
    pub complete: bool,
    rows: Vec<ProcStat>,
}
impl ProcessSnapshot {
    pub fn from_rows(rows: Vec<ProcStat>, complete: bool) -> Self {
        Self { rows, complete }
    }
    pub fn tree(&self, root: u32) -> Vec<ProcessIdentity> {
        let rows = self
            .rows
            .iter()
            .map(|r| (r.pid, r))
            .collect::<IndexMap<_, _>>();
        if !rows.contains_key(&root) {
            return vec![];
        }
        let mut by_parent: IndexMap<u32, Vec<u32>> = IndexMap::new();
        for row in &self.rows {
            by_parent.entry(row.parent_pid).or_default().push(row.pid);
        }
        let mut stack = vec![(root, false)];
        let mut seen = IndexSet::new();
        let mut result = vec![];
        while let Some((pid, emit)) = stack.pop() {
            if emit {
                if let Some(row) = rows.get(&pid) {
                    result.push(ProcessIdentity {
                        pid,
                        started: row.started.clone(),
                    });
                }
                continue;
            }
            if !seen.insert(pid) {
                continue;
            }
            stack.push((pid, true));
            if let Some(children) = by_parent.get(&pid) {
                stack.extend(children.iter().rev().map(|pid| (*pid, false)));
            }
        }
        result
    }
    pub fn session(&self, session: u32) -> Vec<ProcessIdentity> {
        self.rows
            .iter()
            .filter(|r| i64::from(r.session) == i64::from(session))
            .map(|r| ProcessIdentity {
                pid: r.pid,
                started: r.started.clone(),
            })
            .collect()
    }
    pub fn alive(&self, identity: &ProcessIdentity) -> bool {
        self.rows
            .iter()
            .any(|r| r.pid == identity.pid && r.started == identity.started && !quiescent(&r.state))
    }
}
pub trait ProcessInspector: Send + Sync {
    fn foreground_pgid(&self, shell_pid: u32) -> Option<u32>;
    fn is_stdin_waiting(&self, pgid: u32, shell_pid: u32) -> bool;
    fn snapshot(&self) -> Result<ProcessSnapshot, BoxError>;
    fn is_alive(&self, identity: &ProcessIdentity) -> bool;
    fn signal_group(&self, pgid: u32, signal: SubprocessTerminalSignal) -> Result<(), BoxError>;
    fn signal_process(
        &self,
        identity: &ProcessIdentity,
        signal: rustix::process::Signal,
    ) -> Result<(), BoxError>;
}
pub struct LinuxProcessInspector {
    pub arch: String,
    pub internals: Arc<dyn ProcessInspectorInternals>,
}
impl LinuxProcessInspector {
    fn stat(&self, pid: u32) -> Option<ProcStat> {
        parse_proc_stat(
            &self
                .internals
                .read_file(&format!("/proc/{pid}/stat"))
                .ok()?,
        )
    }
    fn numeric(&self, path: &str) -> Option<Vec<u32>> {
        Some(
            self.internals
                .read_dir(path)
                .ok()?
                .into_iter()
                .filter(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
                .filter_map(|s| s.parse().ok())
                .collect(),
        )
    }
    fn terminal_device(&self, pid: u32, tty: i64, tid: Option<u32>) -> Option<u32> {
        let device = tty as u32;
        if device == 0 {
            return None;
        }
        let path = match tid {
            None => format!("/proc/{pid}/fd/0"),
            Some(tid) => format!("/proc/{pid}/task/{tid}/fd/0"),
        };
        if self.internals.read_link(&path).ok()? == "/dev/tty" {
            return Some(device);
        }
        let stat = self.internals.stat(&path).ok()?;
        (stat.character_device && stat.rdev as u32 == device).then_some(device)
    }
    fn syscall(&self, pid: u32, tid: u32) -> Option<(u64, Vec<u64>)> {
        let text = self
            .internals
            .read_file(&format!("/proc/{pid}/task/{tid}/syscall"))
            .ok()?;
        let text = js_trim(&text);
        if text == "running" || text.starts_with("-1 ") {
            return None;
        }
        let fields = text
            .split(is_js_white_space)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let number: u64 = fields.first()?.parse().ok()?;
        let args = fields
            .iter()
            .skip(1)
            .take(6)
            .map(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .collect::<Option<Vec<_>>>()?;
        const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
        if number > MAX_SAFE_INTEGER || args.iter().any(|value| *value > MAX_SAFE_INTEGER) {
            return None;
        }
        Some((number, args))
    }
    fn waits(&self, pid: u32, tid: u32, number: u64, args: &[u64]) -> bool {
        let a0 = args.first().copied().unwrap_or(0);
        let a1 = args.get(1).copied().unwrap_or(0);
        let a2 = args.get(2).copied().unwrap_or(0);
        let tables: Vec<[Option<u64>; 7]> = match self.arch.as_str() {
            "x64" | "x86_64" => vec![
                [
                    Some(0),
                    Some(23),
                    Some(270),
                    Some(7),
                    Some(271),
                    Some(232),
                    Some(281),
                ],
                [Some(63), None, Some(72), None, Some(73), None, Some(22)],
            ],
            "arm64" | "aarch64" => vec![
                [Some(63), None, Some(72), None, Some(73), None, Some(22)],
                [
                    Some(0),
                    Some(23),
                    Some(270),
                    Some(7),
                    Some(271),
                    Some(232),
                    Some(281),
                ],
            ],
            _ => return false,
        };
        for table in tables {
            if table[0] == Some(number) {
                return args.first() == Some(&0);
            }
            if table[1] == Some(number) || table[2] == Some(number) {
                return a0 >= 1
                    && a1 != 0
                    && self
                        .internals
                        .read_memory(pid, a1, 8)
                        .ok()
                        .and_then(|v| v.first().copied())
                        .is_some_and(|b| b & 1 != 0);
            }
            if table[3] == Some(number) || table[4] == Some(number) {
                if a0 == 0 || a1 < 1 {
                    return false;
                }
                let bytes = self
                    .internals
                    .read_memory(pid, a0, (a1.min(1024) * 8) as usize)
                    .unwrap_or_default();
                return bytes.chunks_exact(8).any(|b| {
                    i32::from_le_bytes([b[0], b[1], b[2], b[3]]) == 0
                        && i16::from_le_bytes([b[4], b[5]]) & 1 != 0
                });
            }
            if table[5] == Some(number) || table[6] == Some(number) {
                return a2 >= 1
                    && self
                        .internals
                        .read_file(&format!("/proc/{pid}/task/{tid}/fdinfo/{a0}"))
                        .is_ok_and(|text| {
                            text.lines().any(|line| {
                                let line = js_trim(line);
                                line.strip_prefix("tfd:").is_some_and(|rest| {
                                    let rest = rest.trim_start_matches(is_js_white_space);
                                    rest.starts_with('0')
                                        && rest[1..]
                                            .chars()
                                            .next()
                                            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
                                })
                            })
                        });
            }
        }
        false
    }
}
impl ProcessInspector for LinuxProcessInspector {
    fn foreground_pgid(&self, pid: u32) -> Option<u32> {
        u32::try_from(self.stat(pid)?.tpgid)
            .ok()
            .filter(|id| *id > 0)
    }
    fn is_stdin_waiting(&self, pgid: u32, shell_pid: u32) -> bool {
        let Some(shell) = self.stat(shell_pid) else {
            return false;
        };
        let Some(device) = self.terminal_device(shell_pid, shell.tty_device, None) else {
            return false;
        };
        for pid in self.numeric("/proc").unwrap_or_default() {
            let Some(process) = self.stat(pid) else {
                continue;
            };
            if u32::try_from(process.pgrp).ok() != Some(pgid) {
                continue;
            }
            for tid in self
                .numeric(&format!("/proc/{pid}/task"))
                .unwrap_or_default()
            {
                if let Some((number, args)) = self.syscall(pid, tid)
                    && self.waits(pid, tid, number, &args)
                    && self.terminal_device(pid, process.tty_device, Some(tid)) == Some(device)
                {
                    return true;
                }
            }
        }
        false
    }
    fn snapshot(&self) -> Result<ProcessSnapshot, BoxError> {
        let pids = self
            .numeric("/proc")
            .ok_or_else(|| error("Cannot inspect processes: /proc directory is unreadable"))?;
        let mut complete = true;
        let mut rows = vec![];
        for pid in pids {
            if let Some(row) = self.stat(pid) {
                rows.push(row);
            } else {
                complete = false;
            }
        }
        Ok(ProcessSnapshot { complete, rows })
    }
    fn is_alive(&self, id: &ProcessIdentity) -> bool {
        self.stat(id.pid)
            .is_some_and(|row| row.started == id.started && !quiescent(&row.state))
    }
    fn signal_group(&self, pgid: u32, signal: SubprocessTerminalSignal) -> Result<(), BoxError> {
        self.internals
            .signal(-i32::try_from(pgid)?, terminal_signal(signal))?;
        Ok(())
    }
    fn signal_process(
        &self,
        id: &ProcessIdentity,
        signal: rustix::process::Signal,
    ) -> Result<(), BoxError> {
        if self.is_alive(id) {
            self.internals.signal(i32::try_from(id.pid)?, signal)?;
        }
        Ok(())
    }
}
pub fn terminal_signal(signal: SubprocessTerminalSignal) -> rustix::process::Signal {
    use rustix::process::Signal;
    match signal {
        SubprocessTerminalSignal::Sigint => Signal::INT,
        SubprocessTerminalSignal::Sigterm => Signal::TERM,
        SubprocessTerminalSignal::Sigkill => Signal::KILL,
        SubprocessTerminalSignal::Sigtstp => Signal::TSTP,
        SubprocessTerminalSignal::Sighup => Signal::HUP,
    }
}
pub fn create_process_inspector() -> Arc<dyn ProcessInspector> {
    Arc::new(LinuxProcessInspector {
        arch: std::env::consts::ARCH.into(),
        internals: Arc::new(NativeProcessInspector),
    })
}
pub fn linux_process_group_live_member(
    group: u32,
    known: Option<u32>,
    internals: Arc<dyn ProcessInspectorInternals>,
) -> Option<Option<u32>> {
    let inspector = LinuxProcessInspector {
        arch: std::env::consts::ARCH.into(),
        internals,
    };
    if let Some(pid) = known
        && inspector.stat(pid).is_some_and(|row| {
            u32::try_from(row.pgrp).ok() == Some(group) && !quiescent(&row.state)
        })
    {
        return Some(Some(pid));
    }
    let pids = inspector.numeric("/proc")?;
    let mut matched = false;
    for pid in pids {
        if let Some(row) = inspector.stat(pid)
            && u32::try_from(row.pgrp).ok() == Some(group)
        {
            matched = true;
            if !quiescent(&row.state) {
                return Some(Some(pid));
            }
        }
    }
    matched.then_some(None)
}
