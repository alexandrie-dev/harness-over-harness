//! Supervision of one harness process, the same for every harness: start in its own process
//! group, JSON lines on stdout (anything else is a contract violation), stdin as a bounded queue
//! of lines, stderr kept apart as a short masked tail, and the end of the road — SIGTERM to the
//! group, a bounded wait, then killing the group.

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
        command.envs(spec.env.iter().map(|(k, v)| (k, v)));
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
        if let Some(pid) = self
            .pid
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, signal);
        }
    }

    /// Kill the process group now, without the graceful steps of [`terminate`](Self::terminate),
    /// unless it was terminated or has exited; stdin is closed. For an owner that gives up on the
    /// process (a start the caller stopped). Idempotent.
    pub fn abandon(&self) {
        lock(&self.stdin).take();
        if !self.terminated.swap(true, Ordering::SeqCst) && self.exit_status().is_none() {
            self.signal_group(rustix::process::Signal::KILL);
        }
    }

    /// Close stdin; SIGTERM to the group; wait at most `grace` for the process; kill the group
    /// (members that ignore SIGTERM included); wait for the process. Idempotent.
    pub async fn terminate(&self, grace: Duration) -> ExitInfo {
        lock(&self.stdin).take();
        if self.terminated.swap(true, Ordering::SeqCst) {
            return self.wait().await;
        }
        self.signal_group(rustix::process::Signal::TERM);
        let _ = self.wait_timeout(grace).await;
        self.signal_group(rustix::process::Signal::KILL);
        self.wait().await
    }
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
