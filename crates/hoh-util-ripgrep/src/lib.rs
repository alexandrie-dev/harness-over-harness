//! Packaged ripgrep resolution and subprocess streaming helpers.

mod path;
mod spawn;

use futures_util::future::{Either, join, select};
use hoh_error::{BoxError, SharedError};
use hoh_subprocess::{
    SubprocessDone, SubprocessHandle, SubprocessOutcome, SubprocessReadable, SubprocessRuntime,
    SubprocessSpawnSpec,
};
use std::{path::Path, sync::OnceLock};
use tokio::io::AsyncReadExt;

use path::{Resolution, resolve_from};

static RG_PATH: OnceLock<Result<String, SharedError>> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
enum RipgrepError {
    #[error("ripgrep requires piped stdout")]
    MissingStdout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RipgrepSeparator {
    Newline,
    Nul,
}

impl RipgrepSeparator {
    fn byte(self) -> u8 {
        match self {
            Self::Newline => b'\n',
            Self::Nul => b'\0',
        }
    }
}

/// Exit facts and whether the line consumer requested early termination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RipgrepLinesResult {
    pub outcome: SubprocessOutcome,
    pub stopped: bool,
}

fn resolve_current_path() -> Result<String, BoxError> {
    let executable = std::env::current_exe()?;
    let executable = executable.to_string_lossy();
    resolve_from(
        Resolution {
            executable: &executable,
            windows: cfg!(windows),
            packaged: true,
            electron: false,
            dependency: None,
        },
        |candidate| Path::new(candidate).exists(),
    )
}

/// Resolve the packaged executable lazily, once per process.
///
/// HoH packages ripgrep as an adjacent `<current executable>-rg` sidecar (or
/// `<name>-rg.exe` on Windows). No system executable or `PATH` fallback is used.
pub async fn resolve_rg_path() -> Result<String, BoxError> {
    match RG_PATH.get_or_init(|| resolve_current_path().map_err(SharedError::new)) {
        Ok(path) => Ok(path.clone()),
        Err(error) => Err(Box::new(error.clone())),
    }
}

/// Start packaged ripgrep with `--no-config` prepended to the caller's arguments.
pub async fn spawn_ripgrep(
    subprocess: &dyn SubprocessRuntime,
    spec: SubprocessSpawnSpec,
) -> Result<SubprocessHandle, BoxError> {
    let executable = resolve_rg_path().await?;
    spawn::spawn_ripgrep_at(subprocess, spec, executable)
}

async fn read_records_inner<F>(
    stdout: &mut SubprocessReadable,
    separator: u8,
    lifecycle: std::sync::Arc<dyn hoh_subprocess::SubprocessLifecycle>,
    on_line: &mut F,
) -> Result<bool, BoxError>
where
    F: FnMut(&str) -> Result<bool, BoxError>,
{
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 8192];
    let mut stopped = false;
    loop {
        let count = stdout.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        if stopped {
            continue;
        }
        pending.extend_from_slice(&chunk[..count]);
        let mut consumed = 0;
        while let Some(relative) = pending[consumed..]
            .iter()
            .position(|byte| *byte == separator)
        {
            let end = consumed + relative;
            let line = String::from_utf8_lossy(&pending[consumed..end]);
            consumed = end + 1;
            if !on_line(&line)? {
                stopped = true;
                pending.clear();
                lifecycle.terminate();
                break;
            }
        }
        if !stopped && consumed != 0 {
            pending.drain(..consumed);
        }
    }
    if !stopped && !pending.is_empty() {
        let line = String::from_utf8_lossy(&pending);
        if !on_line(&line)? {
            stopped = true;
            lifecycle.terminate();
        }
    }
    Ok(stopped)
}

async fn read_records<F>(
    mut stdout: SubprocessReadable,
    separator: u8,
    lifecycle: std::sync::Arc<dyn hoh_subprocess::SubprocessLifecycle>,
    on_line: &mut F,
) -> (SubprocessReadable, Result<bool, BoxError>)
where
    F: FnMut(&str) -> Result<bool, BoxError>,
{
    let result = read_records_inner(&mut stdout, separator, lifecycle, on_line).await;
    (stdout, result)
}

async fn settle_ignoring_failures(
    done: SubprocessDone,
    lifecycle: std::sync::Arc<dyn hoh_subprocess::SubprocessLifecycle>,
) {
    let _ = join(done, lifecycle.wait_for_exit(None)).await;
}

async fn settle_after_failure(
    done: SubprocessDone,
    lifecycle: std::sync::Arc<dyn hoh_subprocess::SubprocessLifecycle>,
) -> Result<(), BoxError> {
    let _ = done.await;
    lifecycle.wait_for_exit(None).await?;
    Ok(())
}

/// Read UTF-8 stdout records without retaining the complete output.
///
/// Returning `Ok(false)` terminates ripgrep and suppresses subsequent records.
/// Callback, stream and provider failures are returned after process cleanup.
pub async fn read_ripgrep_lines<F>(
    handle: SubprocessHandle,
    separator: RipgrepSeparator,
    mut on_line: F,
) -> Result<RipgrepLinesResult, BoxError>
where
    F: FnMut(&str) -> Result<bool, BoxError>,
{
    let lifecycle = handle.lifecycle.clone();
    let done = handle.done.clone();
    let Some(stdout) = handle.stdout else {
        lifecycle.terminate();
        settle_ignoring_failures(done, lifecycle).await;
        return Err(Box::new(RipgrepError::MissingStdout));
    };
    let reading = Box::pin(read_records(
        stdout,
        separator.byte(),
        lifecycle.clone(),
        &mut on_line,
    ));
    let completion = Box::pin(done.clone());
    let selected = select(reading, completion).await;
    let (outcome, stopped) = match selected {
        Either::Left(((stdout, reading_result), completion)) => match reading_result {
            Ok(stopped) => match completion.await {
                Ok(outcome) => (outcome, stopped),
                Err(error) => {
                    lifecycle.terminate();
                    drop(stdout);
                    settle_after_failure(done, lifecycle).await?;
                    return Err(Box::new(error));
                }
            },
            Err(error) => {
                lifecycle.terminate();
                drop(stdout);
                settle_after_failure(done, lifecycle).await?;
                return Err(error);
            }
        },
        Either::Right((completion_result, reading)) => match completion_result {
            Ok(outcome) => {
                let (stdout, reading_result) = reading.await;
                match reading_result {
                    Ok(stopped) => (outcome, stopped),
                    Err(error) => {
                        lifecycle.terminate();
                        drop(stdout);
                        settle_after_failure(done, lifecycle).await?;
                        return Err(error);
                    }
                }
            }
            Err(error) => {
                lifecycle.terminate();
                drop(reading);
                settle_after_failure(done, lifecycle).await?;
                return Err(Box::new(error));
            }
        },
    };
    lifecycle.wait_for_exit(None).await?;
    Ok(RipgrepLinesResult { outcome, stopped })
}
