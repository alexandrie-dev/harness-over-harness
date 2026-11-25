//! Supervision of one harness process, the same for every harness: start in its own process
//! group, JSON lines on stdout (anything else is a contract violation), stdin as a bounded queue
//! of lines, stderr kept apart as a short masked tail, and the end of the road — SIGTERM to the
//! group and to every process under the harness, a bounded wait, then killing them all. A guard
//! process does the same killing when the engine itself is gone, even by SIGKILL.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

use crate::contract::{HarnessFailure, code};
use crate::redact::redact;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What to run.
#[derive(Clone, Default)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub cwd_identity: Option<[u64; 2]>,
    /// Added to the inherited environment.
    pub env: Vec<(String, String)>,
    /// Removed from the inherited environment.
    pub env_remove: Vec<String>,
}

impl std::fmt::Debug for ProcessSpec {
    /// Environment values may be keys: only their names are shown (review 221315, item 3).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessSpec")
            .field("program", &self.program)
            .field(
                "args",
                &self.args.iter().map(|arg| redact(arg)).collect::<Vec<_>>(),
            )
            .field("cwd", &self.cwd)
            .field(
                "env",
                &self.env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .field("env_remove", &self.env_remove)
            .finish()
    }
}

/// Bounds of one supervised process.
#[derive(Clone, Copy, Debug)]
pub struct SupervisorLimits {
    /// Longest stdout line; a longer one is a contract violation.
    pub max_frame_bytes: usize,
    /// Parsed stdout lines waiting for the reader; when full, stdout is not read further, so the
    /// process blocks on its own output.
    pub frame_queue: usize,
    /// Lines waiting to be written to stdin.
    pub stdin_queue: usize,
    /// stderr lines kept for diagnostics.
    pub stderr_lines: usize,
    /// Longest stderr line kept (the rest of a longer line is dropped).
    pub stderr_line_bytes: usize,
}

impl Default for SupervisorLimits {
    fn default() -> Self {
        Self {
            max_frame_bytes: 8 * 1024 * 1024,
            frame_queue: 256,
            stdin_queue: 64,
            stderr_lines: 50,
            stderr_line_bytes: 2048,
        }
    }
}

/// One unit read from stdout.
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    /// One line: a JSON object.
    Message(Value),
    /// The line is not a JSON object or is too long. Nothing more is read after it.
    Violation(String),
    /// stdout ended; `truncated` when it ended in the middle of a line.
    Closed { truncated: bool },
}

/// How the process ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl ExitInfo {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Receives each stderr line, already masked.
pub type StderrSink = Arc<dyn Fn(&str) + Send + Sync>;

/// One line for stdin and where its write is confirmed.
type StdinLine = (Vec<u8>, oneshot::Sender<std::io::Result<()>>);

pub struct Supervisor {
    pid: Option<u32>,
    stdin: Mutex<Option<mpsc::Sender<StdinLine>>>,
    frames: Mutex<Option<mpsc::Receiver<Frame>>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    stderr_done: watch::Receiver<bool>,
    exit: watch::Receiver<Option<ExitInfo>>,
    terminated: AtomicBool,
}

impl Supervisor {
    /// Start `spec` in a new process group. A program that cannot be found or executed fails
    /// with [`code::NOT_INSTALLED`].
    pub fn spawn(
        spec: &ProcessSpec,
        limits: SupervisorLimits,
        on_stderr: Option<StderrSink>,
    ) -> Result<Supervisor, HarnessFailure> {
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        for name in &spec.env_remove {
            command.env_remove(name);
        }
        command.envs(
            spec.env
                .iter()
                .filter(|(k, _)| !is_telegram_env(k))
                .map(|(k, v)| (k, v)),
        );
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("TELEGRAM_") {
                command.env_remove(name);
            }
        }
        if let (Some(cwd), Some(expected)) = (&spec.cwd, spec.cwd_identity) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let canonical = std::fs::canonicalize(cwd).map_err(|error| {
                    HarnessFailure::new(
                        code::IO,
                        format!("The conversation folder is unavailable: {error}"),
                    )
                })?;
                let metadata = std::fs::symlink_metadata(cwd).map_err(|error| {
                    HarnessFailure::new(
                        code::IO,
                        format!("The conversation folder is unavailable: {error}"),
                    )
                })?;
                if canonical != *cwd
                    || !metadata.is_dir()
                    || metadata.file_type().is_symlink()
                    || [metadata.dev(), metadata.ino()] != expected
                {
                    return Err(HarnessFailure::new(
                        code::IO,
                        "The conversation folder has changed. Select it again before starting the executor.",
                    ));
                }
            }
            #[cfg(not(unix))]
            return Err(HarnessFailure::new(
                code::IO,
                "The conversation folder identity cannot be verified on this platform.",
            ));
        }
        let mut child = command.spawn().map_err(|error| {
            let program = spec.program.display();
            match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                    HarnessFailure::new(
                        code::NOT_INSTALLED,
                        format!("harness program \"{program}\" is not installed or not executable"),
                    )
                }
                _ => HarnessFailure::new(
                    code::IO,
                    format!("cannot start harness program \"{program}\": {error}"),
                ),
            }
        })?;
        let pid = child.id();
        let (stdin, stdout, stderr) =
            match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
                (Some(stdin), Some(stdout), Some(stderr)) => (stdin, stdout, stderr),
                // All three were requested as pipes above.
                _ => return Err(HarnessFailure::new(code::IO, "harness pipes are missing")),
            };
        // Dropping `child` on a failure here kills the harness (`kill_on_drop`).
        let watched = pid.map(spawn_guard).transpose()?;

        let (exit_tx, exit) = watch::channel(None);
        tokio::spawn(async move {
            let info = match child.wait().await {
                Ok(status) => {
                    use std::os::unix::process::ExitStatusExt;
                    ExitInfo {
                        code: status.code(),
                        signal: status.signal(),
                    }
                }
                Err(_) => ExitInfo::default(),
            };
            // The harness is gone: its guard clears what it left behind and ends.
            drop(watched);
            exit_tx.send_replace(Some(info));
        });

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<StdinLine>(limits.stdin_queue.max(1));
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some((line, written)) = stdin_rx.recv().await {
                let result = match stdin.write_all(&line).await {
                    Ok(()) => stdin.flush().await,
                    Err(error) => Err(error),
                };
                let failed = result.is_err();
                // The sender learns how its own line went.
                let _ = written.send(result);
                if failed {
                    // Lines still queued are dropped unwritten: their senders see stdin closed.
                    break;
                }
            }
        });

        let (frames_tx, frames) = mpsc::channel(limits.frame_queue.max(1));
        let max = limits.max_frame_bytes;
        tokio::spawn(read_frames(BufReader::new(stdout), max, frames_tx));

        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let (stderr_done_tx, stderr_done) = watch::channel(false);
        let reader = read_stderr(BufReader::new(stderr), limits, tail.clone(), on_stderr);
        tokio::spawn(async move {
            reader.await;
            stderr_done_tx.send_replace(true);
        });

        Ok(Supervisor {
            pid,
            stdin: Mutex::new(Some(stdin_tx)),
            frames: Mutex::new(Some(frames)),
            stderr: tail,
            stderr_done,
            exit,
            terminated: AtomicBool::new(false),
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The stdout frames; only the first caller gets them.
    pub fn take_frames(&self) -> Option<mpsc::Receiver<Frame>> {
        lock(&self.frames).take()
    }

    /// Write one message as a line on stdin. `Ok` only once the line is written and flushed into
    /// the pipe (the process may not have read it yet); otherwise the error of this very write:
    /// [`code::CLOSED`] when stdin is closed (the process closed it or exited, a pipe broke, or
    /// [`terminate`](Self::terminate) ran), [`code::IO`] for another write failure. Lines are
    /// written in the order they are sent; this waits for the lines before it. Dropping the future
    /// after the line was queued still writes it.
    pub async fn send(&self, message: &Value) -> Result<(), HarnessFailure> {
        let closed = || HarnessFailure::new(code::CLOSED, "harness stdin is closed");
        let sender = lock(&self.stdin).clone();
        let Some(sender) = sender else {
            return Err(closed());
        };
        let mut line = hoh_util_values::js_json_stringify(message).into_bytes();
        line.push(b'\n');
        let (written, result) = oneshot::channel();
        sender.send((line, written)).await.map_err(|_| closed())?;
        match result.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => Err(
                HarnessFailure::new(code::CLOSED, "harness stdin is closed (broken pipe)"),
            ),
            Ok(Err(error)) => Err(HarnessFailure::new(
                code::IO,
                format!("cannot write to harness stdin: {error}"),
            )),
            // The writer stopped after an earlier failure.
            Err(_) => Err(closed()),
        }
    }

    /// Write one bounded, non-JSON input and close stdin after it. This is for isolated one-shot
    /// provider requests whose CLI takes a prompt on stdin rather than the harness JSON-lines
    /// protocol. Taking the sole sender makes the writer close the pipe after this input; a second
    /// call cannot append data or keep the child waiting indefinitely.
    pub async fn send_input_and_close(
        &self,
        input: &[u8],
        max_bytes: usize,
    ) -> Result<(), HarnessFailure> {
        if input.len() > max_bytes {
            return Err(HarnessFailure::new(
                code::PROTOCOL,
                format!("one-shot harness input exceeds {max_bytes} bytes"),
            ));
        }
        let closed = || HarnessFailure::new(code::CLOSED, "harness stdin is closed");
        let sender = lock(&self.stdin).take().ok_or_else(closed)?;
        let (written, result) = oneshot::channel();
        sender
            .send((input.to_vec(), written))
            .await
            .map_err(|_| closed())?;
        drop(sender);
        match result.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => Err(closed()),
            Ok(Err(error)) => Err(HarnessFailure::new(
                code::IO,
                format!("cannot write one-shot harness input: {error}"),
            )),
            Err(_) => Err(closed()),
        }
    }

    /// Wait, at most `timeout`, until stderr is read to its end (a process that exits may leave
    /// lines not yet read; a surviving descendant may keep the pipe open).
    pub async fn stderr_drained(&self, timeout: Duration) {
        let mut done = self.stderr_done.clone();
        let _ = tokio::time::timeout(timeout, done.wait_for(|done| *done)).await;
    }

    /// The last stderr lines, masked, one per line.
    pub fn stderr_tail(&self) -> String {
        lock(&self.stderr)
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn exit_status(&self) -> Option<ExitInfo> {
        *self.exit.borrow()
    }

    /// Wait for the process to exit.
    pub async fn wait(&self) -> ExitInfo {
        let mut exit = self.exit.clone();
        match exit.wait_for(Option::is_some).await {
            Ok(info) => info.unwrap_or_default(),
            // The waiter task only ends after sending the status.
            Err(_) => self.exit_status().unwrap_or_default(),
        }
    }

    /// Wait at most `timeout` for the process to exit.
    pub async fn wait_timeout(&self, timeout: Duration) -> Option<ExitInfo> {
        tokio::time::timeout(timeout, self.wait()).await.ok()
    }

    /// Send `signal` to the whole process group. Errors (the group is gone) are ignored.
    pub fn signal_group(&self, signal: rustix::process::Signal) {
        if let Some(pid) = self.pid.and_then(raw_pid) {
            let _ = rustix::process::kill_process_group(pid, signal);
        }
    }

    /// The processes under the harness now, read from `/proc` off the runtime's threads
    /// (PORTING.md §6): a command it started in a session of its own is out of its group, not out
    /// of this.
    async fn tree(&self) -> Vec<u32> {
        let Some(pid) = self.pid else {
            return Vec::new();
        };
        tokio::task::spawn_blocking(move || descendants(pid))
            .await
            .unwrap_or_default()
    }

    /// Kill the process group and every process under the harness now, without the graceful
    /// steps of [`terminate`](Self::terminate), unless it was terminated or has exited; stdin is
    /// closed. For an owner that gives up on the process (a start the caller stopped).
    /// Idempotent. The walk of `/proc` and the kill run on a blocking thread of the runtime, or
    /// on a thread of their own outside one: the caller may be a runtime's thread or a `Drop`.
    pub fn abandon(&self) {
        lock(&self.stdin).take();
        if self.terminated.swap(true, Ordering::SeqCst) || self.exit_status().is_some() {
            return;
        }
        let Some(pid) = self.pid else {
            return;
        };
        let kill = move || signal_tree(pid, &descendants(pid), rustix::process::Signal::KILL);
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(kill)),
            Err(_) => {
                if std::thread::Builder::new()
                    .name("hoh-harness-kill".into())
                    .spawn(kill)
                    .is_err()
                {
                    kill();
                }
            }
        }
    }

    /// Close stdin; SIGTERM to the group and to every process under the harness; wait at most
    /// `grace` for the process; kill them all (those that ignore SIGTERM included); wait for the
    /// process. A harness that exited before is not signalled: its pid and group may name others
    /// by now (its guard cleared its group when it exited). Dropped before its kill, this still
    /// kills what it found. Idempotent.
    pub async fn terminate(&self, grace: Duration) -> ExitInfo {
        lock(&self.stdin).take();
        if self.terminated.swap(true, Ordering::SeqCst) {
            return self.wait().await;
        }
        let Some(pid) = self.pid.filter(|_| self.exit_status().is_none()) else {
            return self.wait().await;
        };
        let mut end = KillOnDrop {
            pid,
            tree: Vec::new(),
        };
        // Taken before any signal: once the harness is gone, what it started is no longer under
        // it.
        end.tree = self.tree().await;
        signal_tree(pid, &end.tree, rustix::process::Signal::TERM);
        let _ = self.wait_timeout(grace).await;
        if self.exit_status().is_none() {
            let more = self.tree().await;
            end.tree.extend(more);
        }
        drop(end);
        self.wait().await
    }
}

/// The kill at the end of [`Supervisor::terminate`], also when it is dropped before.
struct KillOnDrop {
    pid: u32,
    tree: Vec<u32>,
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        signal_tree(self.pid, &self.tree, rustix::process::Signal::KILL);
    }
}

/// `signal` to the process group `pid` and to every process of `tree`. Errors (gone) are ignored.
fn signal_tree(pid: u32, tree: &[u32], signal: rustix::process::Signal) {
    if let Some(group) = raw_pid(pid) {
        let _ = rustix::process::kill_process_group(group, signal);
    }
    for pid in tree.iter().copied().filter_map(raw_pid) {
        let _ = rustix::process::kill_process(pid, signal);
    }
}

fn raw_pid(pid: u32) -> Option<rustix::process::Pid> {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
}

/// Every process under `root` — its children, theirs, and so on — as `/proc` shows them now.
fn descendants(root: u32) -> Vec<u32> {
    let parents: Vec<(u32, u32)> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read(entry.path().join("stat")).ok()?;
            Some((pid, parent_in_stat(&stat)?))
        })
        .collect();
    let mut tree = vec![root];
    let mut next = 0;
    while let Some(&parent) = tree.get(next) {
        tree.extend(
            parents
                .iter()
                .filter(|(_, ppid)| *ppid == parent)
                .map(|(pid, _)| *pid),
        );
        next += 1;
    }
    tree.remove(0);
    tree
}

/// The parent pid in `/proc/<pid>/stat`, `pid (name) state ppid …`. Read as bytes and split at
/// the last `)`: the name is up to 15 bytes of whatever the program was started as — `)`, spaces,
/// line breaks, and a character cut in half (`проверка.sh` is 19 bytes), which is no UTF-8.
fn parent_in_stat(stat: &[u8]) -> Option<u32> {
    let end_of_name = stat.iter().rposition(|&byte| byte == b')')?;
    let rest = std::str::from_utf8(&stat[end_of_name + 1..]).ok()?;
    rest.split_ascii_whitespace().nth(1)?.parse().ok()
}

/// The guard of a harness process (`$1`): it waits for the end of a pipe only the engine holds,
/// which comes when the engine stops watching the harness — the harness exited, or the engine is
/// gone, even killed by SIGKILL. It then finds every process under the harness in `/proc` (a
/// command in a session of its own included; the whole `stat` is read, a name may hold a line
/// break; the shell runs without a locale, so a name that is no UTF-8 is read as bytes) and kills
/// them, the harness and its group. A plain shell, so that nothing of the engine
/// has to outlive it. The harness is not stopped first: its
/// group, orphaned by the engine's death with a stopped member, would get SIGHUP from the kernel,
/// and a harness ended by it leaves its commands to another parent before they are found.
const GUARD: &str = r#"read -r _
tree=" $1 "
grown=1
while [ -n "$grown" ]; do
  grown=
  for stat in /proc/[0-9]*/stat; do
    pid=${stat#/proc/}
    pid=${pid%/stat}
    case $tree in *" $pid "*) continue ;; esac
    line=
    { while IFS= read -r part; do line="$line$part "; done < "$stat"; } 2>/dev/null
    rest=${line##*) }
    rest=${rest#* }
    case $tree in *" ${rest%% *} "*) tree="$tree$pid " grown=1 ;; esac
  done
done
kill -9 $tree 2>/dev/null
kill -9 -"$1" 2>/dev/null
"#;

/// Start the guard of the harness `pid`, in a process group of its own (a signal to the engine's
/// group or to the harness's must not take it away first); the returned end of its pipe is held
/// as long as the harness is watched.
fn spawn_guard(pid: u32) -> Result<std::io::PipeWriter, HarnessFailure> {
    let failure = |error: std::io::Error| {
        HarnessFailure::new(
            code::IO,
            format!("cannot start the guard of the harness process: {error}"),
        )
    };
    let (guard_end, engine_end) = std::io::pipe().map_err(failure)?;
    let mut guard = tokio::process::Command::new("/bin/sh")
        .args(["-c", GUARD, "hoh-harness-guard", &pid.to_string()])
        .stdin(guard_end)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear()
        .current_dir("/")
        .process_group(0)
        .spawn()
        .map_err(failure)?;
    tokio::spawn(async move {
        let _ = guard.wait().await;
    });
    Ok(engine_end)
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        // A supervisor dropped without `terminate` must not leave its process group behind.
        self.abandon();
    }
}

async fn read_frames<R: AsyncBufRead + Unpin>(
    mut reader: R,
    max_frame_bytes: usize,
    frames: mpsc::Sender<Frame>,
) {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let available = match reader.fill_buf().await {
            Ok(buffer) => buffer,
            Err(_) => {
                let _ = frames
                    .send(Frame::Closed {
                        truncated: !line.is_empty(),
                    })
                    .await;
                return;
            }
        };
        if available.is_empty() {
            let _ = frames
                .send(Frame::Closed {
                    truncated: !line.is_empty(),
                })
                .await;
            return;
        }
        let (taken, complete) = match available.iter().position(|&b| b == b'\n') {
            Some(position) => {
                line.extend_from_slice(&available[..position]);
                (position + 1, true)
            }
            None => {
                line.extend_from_slice(available);
                (available.len(), false)
            }
        };
        reader.consume(taken);
        if line.len() > max_frame_bytes {
            let _ = frames
                .send(Frame::Violation(format!(
                    "stdout line exceeds {max_frame_bytes} bytes"
                )))
                .await;
            return;
        }
        if !complete {
            continue;
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            line.clear();
            continue;
        }
        let frame = match serde_json::from_slice::<Value>(&line) {
            Ok(value) if value.is_object() => Frame::Message(value),
            _ => Frame::Violation("stdout line is not a JSON object".to_owned()),
        };
        line.clear();
        let violation = matches!(frame, Frame::Violation(_));
        if frames.send(frame).await.is_err() || violation {
            return;
        }
    }
}

async fn read_stderr<R: AsyncBufRead + Unpin>(
    mut reader: R,
    limits: SupervisorLimits,
    tail: Arc<Mutex<VecDeque<String>>>,
    sink: Option<StderrSink>,
) {
    let mut line: Vec<u8> = Vec::new();
    let mut overflow = false;
    loop {
        let available = match reader.fill_buf().await {
            Ok(buffer) if !buffer.is_empty() => buffer,
            _ => break,
        };
        let (taken, complete) = match available.iter().position(|&b| b == b'\n') {
            Some(position) => (position + 1, true),
            None => (available.len(), false),
        };
        let body = &available[..if complete { taken - 1 } else { taken }];
        let room = limits.stderr_line_bytes.saturating_sub(line.len());
        if body.len() > room {
            overflow = true;
        }
        line.extend_from_slice(&body[..body.len().min(room)]);
        reader.consume(taken);
        if complete {
            keep_stderr_line(&line, overflow, limits, &tail, sink.as_ref());
            line.clear();
            overflow = false;
        }
    }
    if !line.is_empty() {
        keep_stderr_line(&line, overflow, limits, &tail, sink.as_ref());
    }
}

fn keep_stderr_line(
    line: &[u8],
    overflow: bool,
    limits: SupervisorLimits,
    tail: &Mutex<VecDeque<String>>,
    sink: Option<&StderrSink>,
) {
    // A cut line may end inside a key, too short for the masks to know it: the word the cut
    // went through is dropped whole.
    let line = if overflow {
        line.iter()
            .rposition(u8::is_ascii_whitespace)
            .map_or(&line[..0], |end| &line[..end])
    } else {
        line
    };
    let mut text = redact(String::from_utf8_lossy(line).trim_end_matches('\r'));
    if overflow {
        text.push('…');
    }
    if let Some(sink) = sink {
        sink(&text);
    }
    let mut tail = lock(tail);
    tail.push_back(text);
    while tail.len() > limits.stderr_lines.max(1) {
        tail.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redact::REDACTED;
    use std::path::Path;

    #[cfg(unix)]
    #[test]
    fn refuses_a_replaced_or_missing_working_directory_before_spawn() {
        use std::os::unix::fs::{MetadataExt, symlink};
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("work");
        let moved = tmp.path().join("moved");
        let other = tmp.path().join("other");
        std::fs::create_dir(&cwd).unwrap();
        std::fs::create_dir(&other).unwrap();
        let metadata = std::fs::metadata(&cwd).unwrap();
        let spec = ProcessSpec {
            program: "sh".into(),
            cwd: Some(cwd.clone()),
            cwd_identity: Some([metadata.dev(), metadata.ino()]),
            ..Default::default()
        };
        std::fs::rename(&cwd, &moved).unwrap();
        symlink(&other, &cwd).unwrap();
        assert!(Supervisor::spawn(&spec, SupervisorLimits::default(), None).is_err());
        std::fs::remove_file(&cwd).unwrap();
        assert!(Supervisor::spawn(&spec, SupervisorLimits::default(), None).is_err());
    }

    /// Review 221315, item 3: printing a spec shows the names of its environment, not the values.
    #[test]
    fn a_spec_prints_no_environment_values() {
        let spec = ProcessSpec {
            program: PathBuf::from("claude"),
            args: vec!["--api-key=sk-ant-api03-ARGSECRETARGSECRET1".into()],
            env: vec![(
                "ANTHROPIC_API_KEY".into(),
                "sk-ant-api03-ENVSECRETENVSECRET12".into(),
            )],
            ..ProcessSpec::default()
        };
        let printed = format!("{spec:?}");
        assert!(printed.contains("ANTHROPIC_API_KEY"), "{printed}");
        assert!(!printed.contains("ENVSECRET"), "{printed}");
        assert!(!printed.contains("ARGSECRET"), "{printed}");
    }

    async fn stderr_lines(input: &[u8]) -> Vec<String> {
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        read_stderr(input, SupervisorLimits::default(), tail.clone(), None).await;
        lock(&tail).iter().cloned().collect()
    }

    /// Process starts in this binary, one at a time. A child forked by a concurrent test holds
    /// every pipe open in the parent until it execs (close-on-exec acts only then), so a test
    /// that needs no other reader on a stdin pipe holds this for its whole length.
    static SPAWNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn shell(script: &str) -> Supervisor {
        let _one = SPAWNING.lock().await;
        spawn_shell(script)
    }

    fn spawn_shell(script: &str) -> Supervisor {
        let spec = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_owned(), script.to_owned()],
            ..ProcessSpec::default()
        };
        Supervisor::spawn(&spec, SupervisorLimits::default(), None).expect("sh")
    }

    async fn next_frame(frames: &mut mpsc::Receiver<Frame>) -> Option<Frame> {
        tokio::time::timeout(Duration::from_secs(10), frames.recv())
            .await
            .expect("a frame in time")
    }

    #[tokio::test]
    async fn telegram_environment_is_removed_from_supervised_process() {
        let _alone = SPAWNING.lock().await;
        let spec = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".into(), r#"if env | grep -q '^TELEGRAM_'; then echo '{"clean":false}'; else echo '{"clean":true}'; fi"#.into()],
            env: vec![("TELEGRAM_U4_EXPLICIT".into(), "secret".into()), ("HOH_U4_ALLOWED".into(), "yes".into())],
            ..ProcessSpec::default()
        };
        let supervisor = Supervisor::spawn(&spec, SupervisorLimits::default(), None).unwrap();
        let mut frames = supervisor.take_frames().unwrap();
        assert!(
            matches!(next_frame(&mut frames).await, Some(Frame::Message(value)) if value["clean"] == true)
        );
    }

    #[tokio::test]
    async fn the_first_send_to_a_process_that_closed_its_stdin_fails() {
        // The process closes its stdin, says so, and lives on; no other start meanwhile.
        let _alone = SPAWNING.lock().await;
        let supervisor = spawn_shell("exec 0<&-; echo '{\"closed\":true}'; exec sleep 30");
        let mut frames = supervisor.take_frames().expect("frames");
        assert_eq!(
            next_frame(&mut frames).await,
            Some(Frame::Message(serde_json::json!({"closed": true})))
        );
        let failure = supervisor
            .send(&serde_json::json!({"answer": "allow"}))
            .await
            .expect_err("the very first write fails");
        assert_eq!(failure.code, code::CLOSED, "{failure}");
        // And every later one.
        let again = supervisor.send(&serde_json::json!({})).await;
        assert_eq!(again.expect_err("closed").code, code::CLOSED);
        supervisor.terminate(Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn a_send_returns_once_its_line_is_written() {
        let supervisor = shell("read line; printf '%s\\n' \"$line\"").await;
        let mut frames = supervisor.take_frames().expect("frames");
        let message = serde_json::json!({"answer": "allow", "id": 7});
        supervisor.send(&message).await.expect("written");
        assert_eq!(next_frame(&mut frames).await, Some(Frame::Message(message)));
        supervisor.wait_timeout(Duration::from_secs(10)).await;
        // The process is gone: a send now fails instead of vanishing.
        let gone = supervisor.send(&serde_json::json!({})).await;
        assert_eq!(gone.expect_err("exited").code, code::CLOSED);
    }

    #[tokio::test]
    async fn one_shot_input_is_bounded_and_closes_stdin() {
        let supervisor = shell(
            "payload=$(cat); printf '{\"bytes\":%s}\\n' \"$(printf %s \"$payload\" | wc -c)\"",
        )
        .await;
        let mut frames = supervisor.take_frames().expect("frames");
        supervisor
            .send_input_and_close(b"title prompt", 32)
            .await
            .expect("bounded input");
        assert_eq!(
            next_frame(&mut frames).await,
            Some(Frame::Message(serde_json::json!({"bytes": 12})))
        );
        supervisor.wait_timeout(Duration::from_secs(10)).await;

        let refused = spawn_shell("exec sleep 30");
        let error = refused
            .send_input_and_close(b"too long", 3)
            .await
            .expect_err("oversized one-shot input");
        assert_eq!(error.code, code::PROTOCOL);
        refused.terminate(Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn abandoning_kills_the_group() {
        let supervisor = shell("exec sleep 30").await;
        let pid = supervisor.pid().expect("pid");
        supervisor.abandon();
        let exit = supervisor
            .wait_timeout(Duration::from_secs(10))
            .await
            .expect("killed");
        assert_eq!(exit.signal, Some(9), "{pid}: {exit:?}");
    }

    fn alive(pid: u32) -> bool {
        std::fs::read(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let end_of_name = stat.iter().rposition(|&byte| byte == b')')?;
                stat.get(end_of_name + 2).copied()
            })
            .is_some_and(|state| state != b'Z')
    }

    /// Waits until `pid` runs as `program`: the kernel names a process by the first 15 bytes of
    /// the file name it was started by.
    fn until_named(pid: u32, program: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let name = program.file_name().expect("a file name").as_bytes();
        let comm = [&name[..name.len().min(15)], b"\n"].concat();
        for _ in 0..100 {
            if std::fs::read(format!("/proc/{pid}/comm")).is_ok_and(|now| now == comm) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("{pid} does not run as {}", program.display());
    }

    /// `/bin/sleep` under the name `name`, in a folder of its own.
    fn sleep_named(name: &str) -> (tempfile::TempDir, PathBuf) {
        let folder = tempfile::tempdir().expect("folder");
        let program = folder.path().join(name);
        std::os::unix::fs::symlink("/bin/sleep", &program).expect("a strange name");
        (folder, program)
    }

    /// Pilot 22.11 («Стоп»): a command the harness runs in a session of its own, as Claude Code
    /// runs its commands, is out of the harness's group and still ends with it.
    #[tokio::test]
    async fn terminating_ends_a_command_in_a_session_of_its_own() {
        terminating_ends_the_command(Path::new("sleep")).await;
    }

    /// Ревью Fable 174941: a command named `проверка.sh` (19 bytes, the kernel keeps 15 and cuts
    /// the `а` in half) has a `stat` that is no UTF-8, and still ends with its harness.
    #[tokio::test]
    async fn terminating_ends_a_command_whose_name_is_cut_in_a_character() {
        let (_folder, program) = sleep_named("проверка.sh");
        terminating_ends_the_command(&program).await;
    }

    async fn terminating_ends_the_command(program: &Path) {
        let supervisor = shell(&format!(
            "setsid '{}' 60 & echo \"{{\\\"command\\\":$!}}\"; exec sleep 60",
            program.display()
        ))
        .await;
        let mut frames = supervisor.take_frames().expect("frames");
        let Some(Frame::Message(said)) = next_frame(&mut frames).await else {
            panic!("no pid of the command");
        };
        let command = u32::try_from(said["command"].as_u64().expect("pid")).expect("pid");
        until_named(command, program);
        assert_eq!(descendants(supervisor.pid().expect("pid")), [command]);
        supervisor.terminate(Duration::from_millis(200)).await;
        for _ in 0..100 {
            if !alive(command) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("command {command} outlived its harness");
    }

    /// Ревью Fable 173026: a `terminate` dropped between its SIGTERM and its kill (a caller's
    /// timeout) still kills the process that ignores SIGTERM.
    #[tokio::test]
    async fn a_terminate_dropped_in_its_grace_still_kills() {
        let supervisor = shell("trap '' TERM; echo '{}'; while :; do sleep 1; done").await;
        let mut frames = supervisor.take_frames().expect("frames");
        assert!(
            next_frame(&mut frames).await.is_some(),
            "SIGTERM is ignored"
        );
        let stop = supervisor.terminate(Duration::from_secs(60));
        let cut = tokio::time::timeout(Duration::from_millis(300), stop).await;
        assert!(cut.is_err(), "the grace was not over");
        let exit = supervisor
            .wait_timeout(Duration::from_secs(10))
            .await
            .expect("killed");
        assert_eq!(exit.signal, Some(9), "{exit:?}");
    }

    /// PORTING.md §6: abandoned outside a runtime (a `Drop` on another thread), the process is
    /// killed by a thread of its own.
    #[test]
    fn abandoning_outside_a_runtime_kills_the_group() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let supervisor = runtime.block_on(shell("exec sleep 30"));
        supervisor.abandon();
        let exit = runtime
            .block_on(supervisor.wait_timeout(Duration::from_secs(10)))
            .expect("killed");
        assert_eq!(exit.signal, Some(9), "{exit:?}");
    }

    /// Ревью Fable 173026: the name of a process (`comm`, the file name it was started by) may
    /// hold a line break, and its `/proc/<pid>/stat` is then two lines: the guard and the engine
    /// still find it under the harness.
    #[test]
    fn a_name_with_a_line_break_does_not_hide_a_process() {
        the_guard_ends_a_command_named("x\n) S 1");
    }

    /// Ревью Fable 174941: nor does a name cut in the middle of a character (no UTF-8).
    #[test]
    fn a_name_cut_in_a_character_does_not_hide_a_process() {
        the_guard_ends_a_command_named("проверка.sh");
    }

    fn the_guard_ends_a_command_named(name: &str) {
        use std::os::unix::process::CommandExt;
        let (_folder, program) = sleep_named(name);
        // The harness in a group of its own, its command in a session of its own: only the walk
        // finds the command.
        let mut root = std::process::Command::new("/bin/sh")
            .args(["-c", "setsid \"$1\" 60 & echo $!; wait", "sh"])
            .arg(&program)
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("sh");
        let mut said = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(root.stdout.take().expect("stdout")),
            &mut said,
        )
        .expect("the pid of the child");
        let child: u32 = said.trim().parse().expect("a pid");
        until_named(child, &program);
        assert_eq!(descendants(root.id()), [child]);
        // As `spawn_guard` starts it: no environment, so no locale.
        std::process::Command::new("/bin/sh")
            .args(["-c", GUARD, "hoh-harness-guard", &root.id().to_string()])
            .stdin(Stdio::null())
            .env_clear()
            .status()
            .expect("the guard");
        root.wait().expect("the harness is collected");
        for _ in 0..100 {
            if !alive(child) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("{child} outlived the guard");
    }

    #[tokio::test]
    async fn a_key_cut_by_the_line_limit_is_dropped_whole() {
        let limit = SupervisorLimits::default().stderr_line_bytes;
        let key = "sk-ant-api03-SECRETSECRETSECRETSECRET1234567890";
        // The key starts 7 bytes before the limit: the cut leaves `sk-ant-`, too short for a mask.
        let mut line = "a".repeat(limit - 8);
        line.push(' ');
        line.push_str(key);
        line.push_str(" tail\nnext line\n");
        let lines = stderr_lines(line.as_bytes()).await;
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(!lines[0].contains("sk-"), "{}", &lines[0][limit - 40..]);
        assert!(lines[0].ends_with('…'));
        assert_eq!(lines[1], "next line");
    }

    #[tokio::test]
    async fn a_line_within_the_limit_is_only_masked() {
        let lines = stderr_lines(b"token=abc123def456 failed\n").await;
        assert_eq!(lines, [format!("token={REDACTED} failed")]);
    }
}

/// Telegram credentials and state are never inherited by external harnesses.
pub fn is_telegram_env(name: &str) -> bool {
    name.starts_with("TELEGRAM_")
}
