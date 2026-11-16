use std::{future::Future, pin::Pin, process::Stdio};
use tokio::io::AsyncReadExt;

pub type CommandFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Dependency-free boundary for the caller's AbortSignal, including its reason.
/// Implement using the application's shared signal, rather than a second cancellation state.
pub trait CommandSignal: Send + Sync {
    fn throw_if_aborted(&self) -> Result<(), NativeCommandError>;
    fn cancelled(&self) -> CommandFuture<'_, NativeCommandError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Hidden,
    Visible,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandCode {
    Exit(i32),
    Name(String),
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct NativeCommandError {
    pub message: String,
    pub code: Option<CommandCode>,
    pub stdout: String,
    pub stderr: String,
    #[source]
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}
impl NativeCommandError {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            stdout: String::new(),
            stderr: String::new(),
            cause: None,
        }
    }
    pub fn io(error: std::io::Error) -> Self {
        let message = error.to_string();
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => "ENOENT".into(),
            std::io::ErrorKind::PermissionDenied => "EACCES".into(),
            _ => error
                .raw_os_error()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "UNKNOWN".into()),
        };
        Self {
            code: Some(CommandCode::Name(code)),
            cause: Some(Box::new(error)),
            ..Self::message(message)
        }
    }
}
impl From<std::io::Error> for NativeCommandError {
    fn from(error: std::io::Error) -> Self {
        Self::io(error)
    }
}

pub trait NativeCommandRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        command: &'a str,
        args: &'a [String],
        signal: &'a dyn CommandSignal,
        window: Window,
    ) -> CommandFuture<'a, Result<CommandOutput, NativeCommandError>>;
}

pub struct SystemCommandRunner;
impl NativeCommandRunner for SystemCommandRunner {
    fn run<'a>(
        &'a self,
        command: &'a str,
        args: &'a [String],
        signal: &'a dyn CommandSignal,
        window: Window,
    ) -> CommandFuture<'a, Result<CommandOutput, NativeCommandError>> {
        Box::pin(run_native_command(command, args, signal, window))
    }
}

pub async fn run_native_command(
    command: &str,
    args: &[String],
    signal: &dyn CommandSignal,
    window: Window,
) -> Result<CommandOutput, NativeCommandError> {
    if let Err(reason) = signal.throw_if_aborted() {
        return Err(abort_error(reason));
    }
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    if window == Window::Hidden {
        cmd.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    let _ = window;
    let mut child = cmd.spawn().map_err(|error| {
        let message = format!(
            "spawn {command} {}",
            if error.kind() == std::io::ErrorKind::NotFound {
                "ENOENT"
            } else {
                "failed"
            }
        );
        NativeCommandError {
            message,
            ..NativeCommandError::io(error)
        }
    })?;
    // Piped streams are present because both were configured above.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let capture = async {
        let read_out = read_limited(&mut stdout, &mut out, "stdout");
        let read_err = read_limited(&mut stderr, &mut err, "stderr");
        let (status, _, _) = tokio::try_join!(
            async { child.wait().await.map_err(NativeCommandError::io) },
            read_out,
            read_err
        )?;
        Ok::<_, NativeCommandError>(status)
    };
    let status = tokio::select! {
        biased;
        reason = signal.cancelled() => {
            Err(abort_error(reason))
        },
        result = capture => result,
    };
    if status.is_err() {
        #[cfg(unix)]
        if let Some(pid) = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        #[cfg(windows)]
        child.start_kill().ok();
        let _ = tokio::join!(
            child.wait(),
            read_limited(&mut stdout, &mut out, "stdout"),
            read_limited(&mut stderr, &mut err, "stderr")
        );
    }
    let output = CommandOutput {
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    };
    match status {
        Ok(status) if status.success() => Ok(output),
        Ok(status) => {
            let message = format!(
                "Command failed: {command}{}\n{}",
                args.iter().map(|s| format!(" {s}")).collect::<String>(),
                output.stderr
            );
            let cause = std::io::Error::other(message.clone());
            Err(NativeCommandError {
                message,
                code: status.code().map(CommandCode::Exit),
                stdout: output.stdout,
                stderr: output.stderr,
                cause: Some(Box::new(cause)),
            })
        }
        Err(error) => Err(NativeCommandError {
            stdout: output.stdout,
            stderr: output.stderr,
            ..error
        }),
    }
}

fn abort_error(reason: NativeCommandError) -> NativeCommandError {
    NativeCommandError {
        code: Some(CommandCode::Name("ABORT_ERR".into())),
        cause: Some(Box::new(reason)),
        ..NativeCommandError::message("The operation was aborted")
    }
}

async fn read_limited(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    output: &mut Vec<u8>,
    stream: &str,
) -> Result<(), NativeCommandError> {
    const MAX_BUFFER: usize = 1024 * 1024;
    let mut buffer = [0; 8192];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        let remaining = MAX_BUFFER.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        if count > remaining {
            let mut error =
                NativeCommandError::message(format!("{stream} maxBuffer length exceeded"));
            error.code = Some(CommandCode::Name(
                "ERR_CHILD_PROCESS_STDIO_MAXBUFFER".into(),
            ));
            return Err(error);
        }
    }
}
