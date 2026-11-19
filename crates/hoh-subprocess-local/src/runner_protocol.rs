//! Strict one-shot Linux runner files. Windows transports wait for stage 5.
use crate::{error, quoted};
use hoh_error::BoxError;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinuxLaunchRequest {
    pub cwd: String,
    pub env: IndexMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipe_fds: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<hoh_subprocess::SubprocessControlMode>,
    /// Private bootstrap transport, filled by `create_linux_launch_files`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_path: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SerializedRunnerError {
    pub name: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syscall: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}
impl std::fmt::Display for SerializedRunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
#[derive(Debug, thiserror::Error)]
pub enum RunnerFailure {
    #[error("{0}")]
    Error(SerializedRunnerError),
}
impl RunnerFailure {
    pub fn diagnostic(&self) -> &SerializedRunnerError {
        match self {
            Self::Error(error) => error,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum LinuxStartupError {
    Error { error: SerializedRunnerError },
}
#[derive(Clone, Debug)]
pub struct LinuxLaunchFiles {
    pub directory: PathBuf,
    pub request_path: PathBuf,
    pub startup_error_path: PathBuf,
    pub control_directory: Option<PathBuf>,
    pub control_path: Option<PathBuf>,
}
pub(crate) const EXEC_STATUS_FIFO: &str = "exec-status.fifo";
const DETACHED_EXEC_STATUS_FIFO: &str = "detached-exec-status.fifo";

/// Node's uv_spawn does not return before exec succeeds or its error is known.
/// The extra Rust bootstrap needs the same barrier, without adding target fds.
pub(crate) struct ExecAcknowledgement(std::fs::File);
impl ExecAcknowledgement {
    pub(crate) fn create(files: &LinuxLaunchFiles) -> Result<Self, BoxError> {
        Self::create_named(files, EXEC_STATUS_FIFO)
    }
    pub(crate) fn create_detached(files: &LinuxLaunchFiles) -> Result<Self, BoxError> {
        Self::create_named(files, DETACHED_EXEC_STATUS_FIFO)
    }
    fn create_named(files: &LinuxLaunchFiles, name: &str) -> Result<Self, BoxError> {
        use rustix::fs::{Mode, OFlags};
        let path = files.directory.join(name);
        rustix::fs::mkfifoat(rustix::fs::CWD, &path, Mode::from_bits_truncate(0o600))?;
        let descriptor = rustix::fs::open(
            &path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(Self(descriptor.into()))
    }

    pub(crate) async fn wait(&mut self, child: &mut tokio::process::Child) -> Result<(), BoxError> {
        use std::io::Read;
        let mut connected = false;
        loop {
            match self.0.read(&mut [0]) {
                Ok(0) if connected => return Ok(()),
                Ok(0) => {}
                Ok(_) => connected = true,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            // Also handles a bootstrap that exits before it opens the FIFO.
            if child.try_wait()?.is_some() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }
}

pub(crate) fn acknowledge_exec(
    files: &LinuxLaunchFiles,
) -> Result<Option<std::fs::File>, BoxError> {
    acknowledge_exec_named(files, EXEC_STATUS_FIFO)
}
pub(crate) fn acknowledge_detached_exec(
    files: &LinuxLaunchFiles,
) -> Result<Option<std::fs::File>, BoxError> {
    acknowledge_exec_named(files, DETACHED_EXEC_STATUS_FIFO)
}
fn acknowledge_exec_named(
    files: &LinuxLaunchFiles,
    name: &str,
) -> Result<Option<std::fs::File>, BoxError> {
    use std::os::unix::fs::FileTypeExt;
    let path = files.directory.join(name);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.file_type().is_fifo() => {
            return Err(crate::error(
                "subprocess runner received an invalid exec-status FIFO",
            ));
        }
        Ok(_) => {}
    }
    let mut file = std::fs::File::from(rustix::fs::open(
        &path,
        rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )?);
    file.write_all(&[1])?;
    // Kept open until exec (CLOEXEC) or until the startup diagnostic is written.
    Ok(Some(file))
}
fn write_new(path: &std::path::Path, value: &impl Serialize) -> Result<(), BoxError> {
    let value = serde_json::to_value(value)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            crate::linux_execve::system_error(error, "open", Some(&path.to_string_lossy()))
        })?
        .write_all(hoh_util_values::js_json_stringify(&value).as_bytes())
        .map_err(|error| crate::linux_execve::system_error(error, "write", None))?;
    Ok(())
}
pub fn create_linux_launch_files(
    request: &LinuxLaunchRequest,
) -> Result<LinuxLaunchFiles, BoxError> {
    let directory = crate::shim::normalize_temporary_path(
        &tempfile::Builder::new()
            .prefix("dsh-subprocess-launch-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(crate::shim::temporary_directory())?
            .keep(),
    );
    let mut files = LinuxLaunchFiles {
        request_path: directory.join("launch-request.json"),
        startup_error_path: directory.join("startup-error.json"),
        directory,
        control_directory: None,
        control_path: None,
    };
    if request.control.is_some() {
        let control_directory = match tempfile::Builder::new()
            .prefix("dsh-ctl-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
        {
            Ok(directory) => directory.keep(),
            Err(error) => {
                cleanup_linux_launch_files(&files);
                return Err(error.into());
            }
        };
        files.control_path = Some(control_directory.join("control.sock"));
        files.control_directory = Some(control_directory);
    }
    let mut request = request.clone();
    request.control_path = files
        .control_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());
    if let Err(error) = (|| {
        std::fs::set_permissions(&files.directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| {
                crate::linux_execve::system_error(
                    error,
                    "chmod",
                    Some(&files.directory.to_string_lossy()),
                )
            })?;
        if let Some(directory) = &files.control_directory {
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).map_err(
                |error| {
                    crate::linux_execve::system_error(
                        error,
                        "chmod",
                        Some(&directory.to_string_lossy()),
                    )
                },
            )?;
        }
        write_new(&files.request_path, &request)
    })() {
        cleanup_linux_launch_files(&files);
        return Err(error);
    }
    Ok(files)
}
pub fn linux_launch_files_from_locator(locator: &str) -> Result<LinuxLaunchFiles, BoxError> {
    use hoh_util_workspace_path::node_path;
    if !node_path::is_absolute(locator) || node_path::basename(locator) != "launch-request.json" {
        return Err(error(
            "subprocess runner received an invalid Linux launch-request locator",
        ));
    }
    let directory = node_path::dirname(locator);
    Ok(LinuxLaunchFiles {
        directory: PathBuf::from(&directory),
        request_path: PathBuf::from(locator),
        startup_error_path: PathBuf::from(node_path::join(&[&directory, "startup-error.json"])),
        control_directory: None,
        control_path: None,
    })
}
pub fn consume_linux_launch_request(
    path: &std::path::Path,
) -> Result<LinuxLaunchRequest, BoxError> {
    let text = crate::read_utf8(path).map_err(|error| read_error(error, path))?;
    std::fs::remove_file(path).map_err(|error| {
        crate::linux_execve::system_error(error, "unlink", Some(&path.to_string_lossy()))
    })?;
    let mut value = hoh_util_values::js_json_parse(&text)?;
    if value.get("pipeFds").is_some_and(serde_json::Value::is_null)
        || value.get("control").is_some_and(serde_json::Value::is_null)
        || value
            .get("controlPath")
            .is_some_and(serde_json::Value::is_null)
    {
        return Err(error(
            "subprocess runner received an invalid Linux launch request",
        ));
    }
    if let Some(serde_json::Value::Array(fds)) = value.get_mut("pipeFds") {
        for fd in fds {
            if let Some(number) = fd.as_f64()
                && matches!(number, 0.0 | 1.0 | 2.0)
            {
                *fd = serde_json::Value::from(number as u64);
            }
        }
    }
    let request: LinuxLaunchRequest = serde_json::from_value(value)
        .map_err(|_| error("subprocess runner received an invalid Linux launch request"))?;
    if request
        .pipe_fds
        .as_ref()
        .is_some_and(|fds| fds.iter().any(|fd| *fd > 2))
    {
        return Err(error(
            "subprocess runner received an invalid Linux launch request",
        ));
    }
    if request.control.is_some() != request.control_path.is_some()
        || request.control_path.as_deref().is_some_and(|path| {
            !hoh_util_workspace_path::node_path::is_absolute(path)
                || hoh_util_workspace_path::node_path::basename(path) != "control.sock"
        })
    {
        return Err(error(
            "subprocess runner received an invalid Linux launch request",
        ));
    }
    Ok(request)
}
pub fn write_linux_startup_error(
    files: &LinuxLaunchFiles,
    error: &LinuxStartupError,
) -> Result<(), BoxError> {
    write_new(&files.startup_error_path, error)
}
pub fn read_linux_startup_error(
    path: &std::path::Path,
) -> Result<Option<LinuxStartupError>, BoxError> {
    if !path.exists() {
        return Ok(None);
    }
    let value = hoh_util_values::js_json_parse(
        &crate::read_utf8(path).map_err(|error| read_error(error, path))?,
    )?;
    if !value.is_object() {
        return Err(error("subprocess runner emitted an invalid startup error"));
    }
    let object = value.as_object().expect("object checked above");
    let record = value.get("error").and_then(serde_json::Value::as_object);
    if object.len() != 2
        || !object.contains_key("type")
        || record.is_none_or(|record| {
            !record.get("name").is_some_and(serde_json::Value::is_string)
                || !record
                    .get("message")
                    .is_some_and(serde_json::Value::is_string)
                || record.iter().any(|(key, value)| {
                    !matches!(
                        key.as_str(),
                        "name" | "message" | "code" | "syscall" | "path"
                    ) || !value.is_string()
                })
        })
    {
        return Err(error("subprocess runner emitted an invalid error result"));
    }
    if value.get("type").and_then(|value| value.as_str()) != Some("error") {
        return Err(error("subprocess runner emitted an unknown error result"));
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|_| error("subprocess runner emitted an invalid error result"))
}
fn read_error(error: std::io::Error, path: &std::path::Path) -> BoxError {
    if hoh_error::node_error_code(&error) == Some("EISDIR") {
        crate::linux_execve::system_error(error, "read", None)
    } else {
        crate::linux_execve::system_error(error, "open", Some(&path.to_string_lossy()))
    }
}
pub fn serialize_runner_error(value: &(dyn std::error::Error + 'static)) -> SerializedRunnerError {
    let value = hoh_error::SharedError::of(value).map_or(value, |shared| shared.get_ref());
    if let Some(value) = value.downcast_ref::<RunnerFailure>() {
        return value.diagnostic().clone();
    }
    let native = value.downcast_ref::<std::io::Error>();
    let code = native
        .and_then(hoh_error::node_error_code)
        .or_else(|| {
            value
                .downcast_ref::<crate::LocalError>()
                .and_then(crate::LocalError::code)
        })
        .or_else(|| hoh_error::HarnessError::of(value).map(|error| error.code()))
        .or_else(
            || match value.downcast_ref::<hoh_subprocess::SubprocessForeignError>() {
                Some(hoh_subprocess::SubprocessForeignError::Coded { code, .. }) => code.as_str(),
                _ => None,
            },
        );
    SerializedRunnerError {
        name: crate::error_name(value).into(),
        message: value.to_string(),
        code: code.map(Into::into),
        syscall: None,
        path: None,
    }
}
pub fn deserialize_runner_error(value: SerializedRunnerError) -> BoxError {
    Box::new(RunnerFailure::Error(value))
}
pub fn cleanup_linux_launch_files(files: &LinuxLaunchFiles) {
    if let Some(path) = &files.control_path {
        let _ = std::fs::remove_file(path);
    }
    if let Some(directory) = &files.control_directory {
        let _ = std::fs::remove_dir(directory);
    }
    if std::fs::symlink_metadata(&files.directory).is_ok_and(|m| m.file_type().is_symlink()) {
        let _ = std::fs::remove_file(&files.directory);
        return;
    }
    for path in [&files.request_path, &files.startup_error_path] {
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return;
        }
    }
    let _ = std::fs::remove_file(files.directory.join(EXEC_STATUS_FIFO));
    let _ = std::fs::remove_file(files.directory.join(DETACHED_EXEC_STATUS_FIFO));
    let _ = std::fs::remove_dir(&files.directory);
}
pub(crate) fn invalid_state(unit: &str, kind: &str, output: &str) -> BoxError {
    error(format!(
        "systemctl returned {kind} for {unit}: {}",
        quoted(hoh_util_values::js_trim(output))
    ))
}
